//! The guarded WebSocket client (ADR 0007): the only WebSocket client in
//! the workspace, used for the Bright Data Browser API (CDP).
//!
//! Same rules as [`crate::GuardedClient`] where they apply: `wss` only, to
//! an exact allowlisted host on one explicitly allowed port, no userinfo in
//! the URL (credentials go in a sensitive `Authorization` header), no
//! proxy, public-only DNS answers, a connect timeout, a per-message byte cap
//! and redacted, URL-free errors.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures_util::{SinkExt as _, StreamExt as _};
use reqwest::Url;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, crypto};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::tungstenite::http::{HeaderValue, header};
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::{Error as WsError, Message, error::CapacityError};

use crate::client::{DEFAULT_CONNECT_TIMEOUT, DEFAULT_MAX_RESPONSE_BYTES};
use crate::error::GuardError;
use crate::ip::is_non_public_ip;
use crate::policy::{HostAllowlist, TransportPolicy};
use secrets::{Redactor, Secret};

/// The byte stream under a WebSocket: plain TCP (loopback tests) or TLS.
trait Io: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> Io for T {}

/// Configures a [`GuardedWebSocket`]; see [`GuardedWebSocket::builder`].
#[derive(Debug, Clone)]
pub struct GuardedWebSocketBuilder {
    allowlist: HostAllowlist,
    port: u16,
    policy: TransportPolicy,
    connect_timeout: Duration,
    max_message_bytes: usize,
    redactor: Redactor,
}

impl GuardedWebSocketBuilder {
    /// Sets the transport policy (default [`TransportPolicy::PUBLIC_HTTPS`]).
    #[must_use]
    pub fn policy(mut self, policy: TransportPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Sets the time allowed for DNS, TCP, TLS and the WebSocket handshake
    /// (default [`DEFAULT_CONNECT_TIMEOUT`]).
    #[must_use]
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// Sets the largest message that may be received (default
    /// [`DEFAULT_MAX_RESPONSE_BYTES`]); larger ones fail with
    /// [`GuardError::BodyTooLarge`].
    #[must_use]
    pub fn max_message_bytes(mut self, bytes: usize) -> Self {
        self.max_message_bytes = bytes;
        self
    }

    /// Sets the redactor applied to every error message.
    #[must_use]
    pub fn redactor(mut self, redactor: Redactor) -> Self {
        self.redactor = redactor;
        self
    }

    /// Opens the WebSocket. `basic_auth` is sent as
    /// `Authorization: Basic …`, marked sensitive; both parts are redacted
    /// from errors.
    ///
    /// # Errors
    ///
    /// URL policy errors as [`crate::GuardedClient::request`] (`wss` only,
    /// allowlisted host, the configured port); [`GuardError::Timeout`],
    /// [`GuardError::Connect`] or [`GuardError::Transport`] with redacted,
    /// URL-free messages.
    pub async fn connect(
        self,
        url: &Url,
        basic_auth: Option<(&Secret, &Secret)>,
    ) -> Result<GuardedWebSocket, GuardError> {
        let host = self.check_url(url)?;
        let redactor = match basic_auth {
            Some((user, password)) => self.redactor.with(user).with(password),
            None => self.redactor.clone(),
        };
        let port = url.port().unwrap_or(self.port);
        let connect = self.open(url, &host, port, basic_auth, &redactor);
        match tokio::time::timeout(self.connect_timeout, connect).await {
            Ok(result) => result,
            Err(_) => Err(GuardError::Timeout),
        }
    }

    async fn open(
        &self,
        url: &Url,
        host: &str,
        port: u16,
        basic_auth: Option<(&Secret, &Secret)>,
        redactor: &Redactor,
    ) -> Result<GuardedWebSocket, GuardError> {
        let connect_error = |message: String| GuardError::Connect(redactor.sanitize(&message));
        let tcp = self.tcp(host, port, redactor).await?;
        let stream: Box<dyn Io> = if url.scheme() == "wss" {
            let server_name = ServerName::try_from(host.to_owned())
                .map_err(|_| GuardError::InvalidUrl("invalid TLS server name".to_owned()))?;
            let tls = TlsConnector::from(Arc::new(tls_config()?))
                .connect(server_name, tcp)
                .await
                .map_err(|error| connect_error(format!("TLS handshake failed: {error}")))?;
            Box::new(tls)
        } else {
            Box::new(tcp)
        };
        let mut request = url
            .as_str()
            .into_client_request()
            .map_err(|error| connect_error(format!("invalid WebSocket request: {error}")))?;
        if let Some((user, password)) = basic_auth {
            let token = BASE64.encode(format!(
                "{}:{}",
                user.expose_secret(),
                password.expose_secret()
            ));
            let mut value = HeaderValue::from_str(&format!("Basic {token}")).map_err(|_| {
                GuardError::InvalidHeader {
                    name: header::AUTHORIZATION.as_str().to_owned(),
                }
            })?;
            value.set_sensitive(true);
            request.headers_mut().insert(header::AUTHORIZATION, value);
        }
        let config = WebSocketConfig::default()
            .max_message_size(Some(self.max_message_bytes))
            .max_frame_size(Some(self.max_message_bytes));
        let (inner, _response) =
            tokio_tungstenite::client_async_with_config(request, stream, Some(config))
                .await
                .map_err(|error| ws_error(error, redactor, self.max_message_bytes, true))?;
        Ok(GuardedWebSocket {
            inner,
            redactor: redactor.clone(),
            max_message_bytes: self.max_message_bytes,
        })
    }

    /// TCP to the first reachable public address of `host`.
    async fn tcp(
        &self,
        host: &str,
        port: u16,
        redactor: &Redactor,
    ) -> Result<TcpStream, GuardError> {
        let literal = host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>();
        let addresses: Vec<SocketAddr> = match literal {
            Ok(ip) => vec![SocketAddr::new(ip, port)],
            Err(_) => tokio::net::lookup_host((host, port))
                .await
                .map_err(|error| GuardError::Connect(redactor.sanitize(&error.to_string())))?
                .filter(|address| self.policy.loopback_test || !is_non_public_ip(address.ip()))
                .collect(),
        };
        if addresses.is_empty() {
            return Err(GuardError::Connect(
                "destination host resolves only to local or private network addresses".to_owned(),
            ));
        }
        let mut last = String::new();
        for address in addresses {
            match TcpStream::connect(address).await {
                Ok(stream) => return Ok(stream),
                Err(error) => last = error.to_string(),
            }
        }
        Err(GuardError::Connect(redactor.sanitize(&last)))
    }

    /// Validates `url`; returns its host.
    fn check_url(&self, url: &Url) -> Result<String, GuardError> {
        let loopback_test = self.policy.loopback_test;
        match url.scheme() {
            "wss" => {}
            "ws" if loopback_test => {}
            other => {
                return Err(GuardError::InsecureScheme {
                    scheme: other.to_owned(),
                });
            }
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(GuardError::InvalidUrl(
                "URLs must not carry credentials".to_owned(),
            ));
        }
        let host = url
            .host_str()
            .ok_or_else(|| GuardError::InvalidUrl("URL has no host".to_owned()))?;
        let literal = host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>();
        match literal {
            Ok(ip) if loopback_test && ip.is_loopback() => {}
            Ok(_) => {
                return Err(GuardError::HostNotAllowed {
                    host: host.to_owned(),
                });
            }
            Err(_) if self.allowlist.contains(host) => {}
            Err(_) => {
                return Err(GuardError::HostNotAllowed {
                    host: host.to_owned(),
                });
            }
        }
        if !loopback_test && url.port_or_known_default() != Some(self.port) {
            return Err(GuardError::InvalidUrl(
                "only the configured WebSocket port is allowed".to_owned(),
            ));
        }
        Ok(host.to_owned())
    }
}

/// TLS with the platform's certificate verifier and aws-lc-rs (the
/// provider reqwest already uses), chosen explicitly so no process-wide
/// default provider is needed.
fn tls_config() -> Result<ClientConfig, GuardError> {
    use rustls_platform_verifier::BuilderVerifierExt as _;
    let build = |message: String| GuardError::Build(message);
    Ok(
        ClientConfig::builder_with_provider(Arc::new(crypto::aws_lc_rs::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|error| build(error.to_string()))?
            .with_platform_verifier()
            .map_err(|error| build(error.to_string()))?
            .with_no_client_auth(),
    )
}

fn ws_error(error: WsError, redactor: &Redactor, limit: usize, connecting: bool) -> GuardError {
    match error {
        WsError::Capacity(CapacityError::MessageTooLong { .. }) => GuardError::BodyTooLarge {
            limit: u64::try_from(limit).unwrap_or(u64::MAX),
        },
        WsError::Http(response) => GuardError::Connect(format!(
            "WebSocket upgrade refused with HTTP {}",
            response.status().as_u16()
        )),
        other if connecting => GuardError::Connect(redactor.sanitize(&other.to_string())),
        other => GuardError::Transport(redactor.sanitize(&other.to_string())),
    }
}

/// An open guarded WebSocket carrying text messages.
pub struct GuardedWebSocket {
    inner: WebSocketStream<Box<dyn Io>>,
    redactor: Redactor,
    max_message_bytes: usize,
}

impl std::fmt::Debug for GuardedWebSocket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GuardedWebSocket").finish_non_exhaustive()
    }
}

impl GuardedWebSocket {
    /// Starts a WebSocket that may only reach `allowlist` on `port` (the
    /// URL may omit it only when it is the scheme's default).
    #[must_use]
    pub fn builder(allowlist: HostAllowlist, port: u16) -> GuardedWebSocketBuilder {
        GuardedWebSocketBuilder {
            allowlist,
            port,
            policy: TransportPolicy::PUBLIC_HTTPS,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            max_message_bytes: usize::try_from(DEFAULT_MAX_RESPONSE_BYTES).unwrap_or(usize::MAX),
            redactor: Redactor::default(),
        }
    }

    /// Sends one text message.
    ///
    /// # Errors
    ///
    /// [`GuardError::Transport`] with a redacted message.
    pub async fn send_text(&mut self, text: &str) -> Result<(), GuardError> {
        self.inner
            .send(Message::text(text))
            .await
            .map_err(|error| ws_error(error, &self.redactor, self.max_message_bytes, false))
    }

    /// The next text message; `None` once the peer closed. Pings are
    /// answered by the library; binary messages are an error.
    ///
    /// # Errors
    ///
    /// [`GuardError::BodyTooLarge`] over the message cap, otherwise
    /// [`GuardError::Transport`] with a redacted message.
    pub async fn recv_text(&mut self) -> Result<Option<String>, GuardError> {
        loop {
            match self.inner.next().await {
                None => return Ok(None),
                Some(Err(WsError::ConnectionClosed | WsError::AlreadyClosed)) => return Ok(None),
                Some(Err(error)) => {
                    return Err(ws_error(
                        error,
                        &self.redactor,
                        self.max_message_bytes,
                        false,
                    ));
                }
                Some(Ok(Message::Text(text))) => return Ok(Some(text.as_str().to_owned())),
                Some(Ok(Message::Close(_))) => return Ok(None),
                Some(Ok(Message::Binary(_))) => {
                    return Err(GuardError::Transport(
                        "unexpected binary WebSocket message".to_owned(),
                    ));
                }
                Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => {}
            }
        }
    }

    /// Closes the connection (best effort).
    pub async fn close(mut self) {
        let _ = self.inner.close(None).await;
    }
}

/// Loopback fake WebSocket servers for tests (feature `test-support`), so
/// crates that may not depend on `tungstenite` can script a peer.
#[cfg(any(test, feature = "test-support"))]
pub mod test_support {
    use std::future::Future;
    use std::sync::{Arc, Mutex};

    use futures_util::{SinkExt as _, StreamExt as _};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::task::JoinHandle;
    use tokio_tungstenite::WebSocketStream;
    use tokio_tungstenite::tungstenite::Message;
    use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};

    /// The server side of one fake WebSocket connection.
    #[derive(Debug)]
    pub struct FakePeer {
        inner: WebSocketStream<TcpStream>,
    }

    impl FakePeer {
        /// The next text message from the client (`None` once closed).
        pub async fn recv(&mut self) -> Option<String> {
            loop {
                match self.inner.next().await? {
                    Ok(Message::Text(text)) => return Some(text.as_str().to_owned()),
                    Ok(Message::Close(_)) | Err(_) => return None,
                    Ok(_) => {}
                }
            }
        }

        /// Sends a text message to the client (errors ignored).
        pub async fn send(&mut self, text: &str) {
            let _ = self.inner.send(Message::text(text)).await;
        }
    }

    /// A fake server accepting one connection; dropping it stops the task.
    #[derive(Debug)]
    pub struct FakeWebSocketServer {
        /// `ws://127.0.0.1:<port>`.
        pub url: String,
        headers: Arc<Mutex<Vec<(String, String)>>>,
        task: JoinHandle<()>,
    }

    impl FakeWebSocketServer {
        /// Handshake request headers `(lowercase name, value)` of the
        /// accepted connection.
        #[must_use]
        pub fn headers(&self) -> Vec<(String, String)> {
            self.headers.lock().map(|h| h.clone()).unwrap_or_default()
        }

        /// Waits for the script to finish.
        pub async fn finished(&mut self) {
            let _ = (&mut self.task).await;
        }
    }

    impl Drop for FakeWebSocketServer {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    /// Accepts one connection on a loopback port and runs `script` on it.
    ///
    /// # Panics
    ///
    /// When no loopback port can be bound (test helper).
    pub async fn serve<F, Fut>(script: F) -> FakeWebSocketServer
    where
        F: FnOnce(FakePeer) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send,
    {
        #[allow(clippy::expect_used)]
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a loopback port");
        #[allow(clippy::expect_used)]
        let url = format!("ws://{}", listener.local_addr().expect("local address"));
        let headers = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&headers);
        let task = tokio::spawn(async move {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            // The error type is fixed by tungstenite's `Callback` signature.
            #[allow(clippy::result_large_err)]
            let callback = move |request: &Request, response: Response| {
                if let Ok(mut out) = recorded.lock() {
                    for (name, value) in request.headers() {
                        out.push((
                            name.as_str().to_owned(),
                            value.to_str().unwrap_or_default().to_owned(),
                        ));
                    }
                }
                Ok(response)
            };
            let Ok(inner) = tokio_tungstenite::accept_hdr_async(stream, callback).await else {
                return;
            };
            script(FakePeer { inner }).await;
        });
        FakeWebSocketServer { url, headers, task }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::serve;
    use super::*;

    fn secret(value: &str) -> Secret {
        Secret::new(value.to_owned()).unwrap()
    }

    fn builder() -> GuardedWebSocketBuilder {
        GuardedWebSocket::builder(HostAllowlist::new(["brd.superproxy.io"]).unwrap(), 9222)
    }

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap()
    }

    #[test]
    fn production_policy_refuses_bad_urls() {
        let b = builder();
        assert!(b.check_url(&url("wss://brd.superproxy.io:9222")).is_ok());
        for bad in [
            "ws://brd.superproxy.io:9222",
            "wss://brd.superproxy.io",
            "wss://brd.superproxy.io:443",
            "wss://brd.superproxy.io:9223",
            "wss://evil.superproxy.io:9222",
            "wss://user:pw@brd.superproxy.io:9222",
            "wss://127.0.0.1:9222",
            "https://brd.superproxy.io:9222",
        ] {
            assert!(b.check_url(&url(bad)).is_err(), "{bad} must be refused");
        }
    }

    #[test]
    fn port_rule_applies_only_to_this_host_list() {
        let other = GuardedWebSocket::builder(HostAllowlist::new(["example.com"]).unwrap(), 443);
        assert!(other.check_url(&url("wss://example.com")).is_ok());
        assert!(other.check_url(&url("wss://example.com:9222")).is_err());
    }

    #[tokio::test]
    async fn round_trip_with_sensitive_basic_auth() {
        let mut server = serve(|mut peer| async move {
            while let Some(text) = peer.recv().await {
                peer.send(&format!("echo:{text}")).await;
            }
        })
        .await;
        let user = secret("brd-customer-c-zone-z-country-mx");
        let password = secret("pw-secret-0001");
        let mut socket = builder()
            .policy(TransportPolicy::LOOPBACK_TEST)
            .connect(&url(&server.url), Some((&user, &password)))
            .await
            .unwrap();
        socket.send_text("hello").await.unwrap();
        assert_eq!(
            socket.recv_text().await.unwrap().as_deref(),
            Some("echo:hello")
        );
        socket.close().await;
        server.finished().await;
        let expected = format!(
            "Basic {}",
            BASE64.encode("brd-customer-c-zone-z-country-mx:pw-secret-0001")
        );
        assert!(
            server
                .headers()
                .iter()
                .any(|(name, value)| name == "authorization" && *value == expected)
        );
    }

    #[tokio::test]
    async fn oversized_messages_are_refused() {
        let _server = serve(|mut peer| async move {
            peer.send(&"x".repeat(2048)).await;
            let _ = peer.recv().await;
        })
        .await;
        let mut socket = builder()
            .policy(TransportPolicy::LOOPBACK_TEST)
            .max_message_bytes(1024)
            .connect(&url(&_server.url), None)
            .await
            .unwrap();
        assert!(matches!(
            socket.recv_text().await,
            Err(GuardError::BodyTooLarge { limit: 1024 })
        ));
    }

    #[tokio::test]
    async fn refused_upgrade_is_a_connect_error_without_the_url() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buffer = [0_u8; 2048];
            let _ = stream.read(&mut buffer).await;
            let _ = stream
                .write_all(b"HTTP/1.1 407 Proxy Auth\r\nContent-Length: 0\r\n\r\n")
                .await;
        });
        let target = format!("ws://{address}/secret-path");
        let error = builder()
            .policy(TransportPolicy::LOOPBACK_TEST)
            .connect(&url(&target), None)
            .await
            .unwrap_err();
        task.abort();
        let text = error.to_string();
        assert!(text.contains("407"), "{text}");
        assert!(!text.contains("secret-path"), "{text}");
    }
}
