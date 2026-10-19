//! The guarded client, request and response wrappers.

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use reqwest::header::{self, HeaderName, HeaderValue};
use reqwest::redirect::Policy;
use reqwest::{Method, StatusCode, Url};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::GuardError;
use crate::policy::{HostAllowlist, TransportPolicy};
use crate::resolver::PublicOnlyResolver;
use crate::secret::header_value;
use secrets::{Redactor, Secret};

/// General cap for provider response bodies.
pub const DEFAULT_MAX_RESPONSE_BYTES: u64 = 8 * 1024 * 1024;
/// Cap for small responses: OAuth tokens and provider error bodies.
pub const SMALL_RESPONSE_BYTES: u64 = 256 * 1024;
/// Default time allowed to open a connection (DNS + TCP + TLS).
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Default time allowed for a whole request, including the body.
pub const DEFAULT_TOTAL_TIMEOUT: Duration = Duration::from_secs(120);

/// Configures a [`GuardedClient`]; see [`GuardedClient::builder`].
#[derive(Debug, Clone)]
pub struct GuardedClientBuilder {
    allowlist: HostAllowlist,
    policy: TransportPolicy,
    connect_timeout: Duration,
    total_timeout: Duration,
    redactor: Redactor,
}

impl GuardedClientBuilder {
    /// Sets the transport policy (default [`TransportPolicy::PUBLIC_HTTPS`]).
    #[must_use]
    pub fn policy(mut self, policy: TransportPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Sets the connect timeout (default [`DEFAULT_CONNECT_TIMEOUT`]).
    #[must_use]
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// Sets the total request timeout (default [`DEFAULT_TOTAL_TIMEOUT`]).
    #[must_use]
    pub fn total_timeout(mut self, timeout: Duration) -> Self {
        self.total_timeout = timeout;
        self
    }

    /// Sets the redactor applied to every error message; give it every
    /// secret this client sends.
    #[must_use]
    pub fn redactor(mut self, redactor: Redactor) -> Self {
        self.redactor = redactor;
        self
    }

    /// Builds the client.
    ///
    /// # Errors
    ///
    /// [`GuardError::Build`] when the TLS backend cannot be initialised.
    pub fn build(self) -> Result<GuardedClient, GuardError> {
        let allowlist = Arc::new(self.allowlist);
        // `no_proxy`: an environment proxy would resolve the destination
        // itself, bypassing `PublicOnlyResolver`. Redirects are never
        // followed: reqwest strips only a fixed set of auth headers on
        // cross-host redirects, so `x-api-key` style headers (and replayed
        // 307/308 bodies) would otherwise follow.
        let builder = reqwest::Client::builder()
            .no_proxy()
            .redirect(Policy::none())
            .connect_timeout(self.connect_timeout)
            .timeout(self.total_timeout)
            .https_only(!self.policy.loopback_test);
        let builder = if self.policy.loopback_test {
            builder
        } else {
            builder.dns_resolver(Arc::new(PublicOnlyResolver::new(Arc::clone(&allowlist))))
        };
        let inner = builder.build().map_err(|error| {
            match GuardError::from_reqwest(error, &self.redactor) {
                GuardError::Build(message)
                | GuardError::Connect(message)
                | GuardError::Transport(message) => GuardError::Build(message),
                other => other,
            }
        })?;
        Ok(GuardedClient {
            inner,
            allowlist,
            policy: self.policy,
            redactor: self.redactor,
        })
    }
}

/// The only HTTP client in the workspace: https only, allowlisted hosts, no
/// redirects, no proxy, timeouts, public-only DNS, bounded bodies and
/// redacted errors. Cheap to clone; build one per provider.
#[derive(Debug, Clone)]
pub struct GuardedClient {
    inner: reqwest::Client,
    allowlist: Arc<HostAllowlist>,
    policy: TransportPolicy,
    redactor: Redactor,
}

impl GuardedClient {
    /// Starts a client that may only reach `allowlist`.
    #[must_use]
    pub fn builder(allowlist: HostAllowlist) -> GuardedClientBuilder {
        GuardedClientBuilder {
            allowlist,
            policy: TransportPolicy::PUBLIC_HTTPS,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            total_timeout: DEFAULT_TOTAL_TIMEOUT,
            redactor: Redactor::default(),
        }
    }

    /// Starts a request after validating `url` (no I/O happens on error).
    ///
    /// # Errors
    ///
    /// [`GuardError::InsecureScheme`], [`GuardError::HostNotAllowed`] or
    /// [`GuardError::InvalidUrl`] (userinfo, non-default port, no host).
    pub fn request(&self, method: Method, url: &Url) -> Result<GuardedRequest, GuardError> {
        self.check_url(url)?;
        Ok(GuardedRequest {
            inner: self.inner.request(method, url.clone()),
            redactor: self.redactor.clone(),
        })
    }

    /// `GET url`; see [`GuardedClient::request`].
    ///
    /// # Errors
    ///
    /// As [`GuardedClient::request`].
    pub fn get(&self, url: &Url) -> Result<GuardedRequest, GuardError> {
        self.request(Method::GET, url)
    }

    /// `POST url`; see [`GuardedClient::request`].
    ///
    /// # Errors
    ///
    /// As [`GuardedClient::request`].
    pub fn post(&self, url: &Url) -> Result<GuardedRequest, GuardError> {
        self.request(Method::POST, url)
    }

    /// Redacts this client's secrets from `text` and truncates it; use it on
    /// provider error bodies before building messages.
    #[must_use]
    pub fn sanitize(&self, text: &str) -> String {
        self.redactor.sanitize(text)
    }

    /// The redactor this client applies to errors.
    #[must_use]
    pub fn redactor(&self) -> &Redactor {
        &self.redactor
    }

    fn check_url(&self, url: &Url) -> Result<(), GuardError> {
        let loopback_test = self.policy.loopback_test;
        match url.scheme() {
            "https" => {}
            "http" if loopback_test => {}
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
        if url.port().is_some() && !loopback_test {
            return Err(GuardError::InvalidUrl(
                "only the default port is allowed".to_owned(),
            ));
        }
        Ok(())
    }
}

/// A request being built; every header is validated as it is added.
#[derive(Debug)]
pub struct GuardedRequest {
    inner: reqwest::RequestBuilder,
    redactor: Redactor,
}

impl GuardedRequest {
    /// Adds a non-secret header.
    ///
    /// # Errors
    ///
    /// [`GuardError::ForbiddenHeader`] for `Host`;
    /// [`GuardError::InvalidHeader`] for an invalid name or value.
    pub fn header(self, name: &str, value: &str) -> Result<Self, GuardError> {
        let header_name = header_name(name)?;
        let value = HeaderValue::from_str(value).map_err(|_| GuardError::InvalidHeader {
            name: name.to_owned(),
        })?;
        Ok(self.with(header_name, value))
    }

    /// Adds a header carrying a secret (e.g. `x-api-key`); the value is
    /// marked sensitive and redacted from this request's errors and from
    /// [`GuardedResponse::error_text`], even if the client's redactor lacks it.
    ///
    /// # Errors
    ///
    /// As [`GuardedRequest::header`]; the error never contains the value.
    pub fn secret_header(self, name: &str, secret: &Secret) -> Result<Self, GuardError> {
        let header_name = header_name(name)?;
        let value = header_value(secret, "").map_err(|_| GuardError::InvalidHeader {
            name: name.to_owned(),
        })?;
        Ok(self.with_secret(secret).with(header_name, value))
    }

    /// Adds `Authorization: Bearer <secret>`, marked sensitive and redacted
    /// like [`GuardedRequest::secret_header`].
    ///
    /// # Errors
    ///
    /// [`GuardError::InvalidHeader`]; the error never contains the value.
    pub fn bearer(self, secret: &Secret) -> Result<Self, GuardError> {
        let value = header_value(secret, "Bearer ").map_err(|_| GuardError::InvalidHeader {
            name: header::AUTHORIZATION.as_str().to_owned(),
        })?;
        Ok(self.with_secret(secret).with(header::AUTHORIZATION, value))
    }

    /// Sets a JSON body and `Content-Type: application/json`.
    ///
    /// # Errors
    ///
    /// [`GuardError::InvalidJson`] when `value` cannot be serialized.
    pub fn json<T: Serialize + ?Sized>(self, value: &T) -> Result<Self, GuardError> {
        let body = serde_json::to_vec(value)
            .map_err(|error| GuardError::InvalidJson(self.redactor.sanitize(&error.to_string())))?;
        Ok(self
            .with(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            )
            .body(body))
    }

    /// Sets an `application/x-www-form-urlencoded` body. Every
    /// [`FormValue::Secret`] is added to this request's redactor, like
    /// [`GuardedRequest::secret_header`].
    ///
    /// # Errors
    ///
    /// [`GuardError::InvalidUrl`] if the form cannot be encoded; the error
    /// never contains a value.
    pub fn form(self, fields: &[(&str, FormValue<'_>)]) -> Result<Self, GuardError> {
        let pairs: Vec<(&str, &str)> = fields
            .iter()
            .map(|(name, value)| (*name, value.expose()))
            .collect();
        let body = serde_urlencoded::to_string(&pairs)
            .map_err(|_| GuardError::InvalidUrl("form body could not be encoded".to_owned()))?;
        let mut request = self;
        for (_, value) in fields {
            if let FormValue::Secret(secret) = value {
                request = request.with_secret(secret);
            }
        }
        Ok(request
            .with(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/x-www-form-urlencoded"),
            )
            .body(body))
    }

    /// Sets a raw body.
    #[must_use]
    pub fn body(self, body: impl Into<Vec<u8>>) -> Self {
        Self {
            inner: self.inner.body(body.into()),
            redactor: self.redactor,
        }
    }

    /// Sends the request. Redirect responses are returned as-is.
    ///
    /// # Errors
    ///
    /// [`GuardError::Timeout`], [`GuardError::Connect`] or
    /// [`GuardError::Transport`], with redacted, URL-free messages.
    pub async fn send(self) -> Result<GuardedResponse, GuardError> {
        let redactor = self.redactor;
        match self.inner.send().await {
            Ok(inner) => Ok(GuardedResponse { inner, redactor }),
            Err(error) => Err(GuardError::from_reqwest(error, &redactor)),
        }
    }

    fn with(self, name: HeaderName, value: HeaderValue) -> Self {
        Self {
            inner: self.inner.header(name, value),
            redactor: self.redactor,
        }
    }

    fn with_secret(self, secret: &Secret) -> Self {
        Self {
            redactor: self.redactor.with(secret),
            inner: self.inner,
        }
    }
}

fn header_name(name: &str) -> Result<HeaderName, GuardError> {
    let parsed =
        HeaderName::from_bytes(name.as_bytes()).map_err(|_| GuardError::InvalidHeader {
            name: name.to_owned(),
        })?;
    if parsed == header::HOST {
        return Err(GuardError::ForbiddenHeader {
            name: name.to_owned(),
        });
    }
    Ok(parsed)
}

/// One value of a [`GuardedRequest::form`] body.
#[derive(Clone, Copy)]
pub enum FormValue<'a> {
    /// A non-secret value.
    Plain(&'a str),
    /// A secret value, redacted from this request's errors.
    Secret(&'a Secret),
}

impl FormValue<'_> {
    fn expose(&self) -> &str {
        match self {
            Self::Plain(text) => text,
            Self::Secret(secret) => secret.expose_secret(),
        }
    }
}

impl std::fmt::Debug for FormValue<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Plain(text) => f.debug_tuple("Plain").field(text).finish(),
            Self::Secret(_) => f.write_str("Secret([redacted])"),
        }
    }
}

/// A response whose body can only be read through a byte cap.
#[derive(Debug)]
pub struct GuardedResponse {
    inner: reqwest::Response,
    redactor: Redactor,
}

impl GuardedResponse {
    /// The HTTP status.
    #[must_use]
    pub fn status(&self) -> StatusCode {
        self.inner.status()
    }

    /// The response headers.
    #[must_use]
    pub fn headers(&self) -> &header::HeaderMap {
        self.inner.headers()
    }

    /// Reads at most `max_bytes` of the body; larger bodies are an error.
    ///
    /// # Errors
    ///
    /// [`GuardError::BodyTooLarge`], or a transport error while reading.
    pub async fn bytes(mut self, max_bytes: u64) -> Result<Vec<u8>, GuardError> {
        let too_large = GuardError::BodyTooLarge { limit: max_bytes };
        if self
            .inner
            .content_length()
            .is_some_and(|length| length > max_bytes)
        {
            return Err(too_large);
        }
        let mut body = Vec::new();
        while let Some(chunk) = self
            .inner
            .chunk()
            .await
            .map_err(|error| GuardError::from_reqwest(error, &self.redactor))?
        {
            let total = u64::try_from(body.len().saturating_add(chunk.len())).unwrap_or(u64::MAX);
            if total > max_bytes {
                return Err(too_large);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }

    /// Reads at most `max_bytes` as text (invalid UTF-8 is replaced).
    ///
    /// # Errors
    ///
    /// As [`GuardedResponse::bytes`].
    pub async fn text(self, max_bytes: u64) -> Result<String, GuardError> {
        let body = self.bytes(max_bytes).await?;
        Ok(String::from_utf8_lossy(&body).into_owned())
    }

    /// Reads at most `max_bytes` of a provider error body as redacted,
    /// truncated text, safe to put in an error, log line or API response.
    /// Covers the client's secrets and every secret sent on this request.
    ///
    /// # Errors
    ///
    /// As [`GuardedResponse::bytes`].
    pub async fn error_text(self, max_bytes: u64) -> Result<String, GuardError> {
        let redactor = self.redactor.clone();
        let text = self.text(max_bytes).await?;
        Ok(redactor.sanitize(&text))
    }

    /// Redacts this request's secrets from `text` and truncates it.
    #[must_use]
    pub fn sanitize(&self, text: &str) -> String {
        self.redactor.sanitize(text)
    }

    /// Reads at most `max_bytes` and parses it as JSON.
    ///
    /// # Errors
    ///
    /// As [`GuardedResponse::bytes`], or [`GuardError::InvalidJson`].
    pub async fn json<T: DeserializeOwned>(self, max_bytes: u64) -> Result<T, GuardError> {
        let redactor = self.redactor.clone();
        let body = self.bytes(max_bytes).await?;
        serde_json::from_slice(&body)
            .map_err(|error| GuardError::InvalidJson(redactor.sanitize(&error.to_string())))
    }
}
