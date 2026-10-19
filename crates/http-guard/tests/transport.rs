//! Transport tests ported from linkgo `net/transport.rs`, run against
//! loopback fake servers.

use std::time::Duration;

use http_guard::{
    GuardError, GuardedClient, HostAllowlist, Redactor, Secret, TransportPolicy, Url,
};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

const FAKE_KEY: &str = "test-key-00000000";

fn fake_key() -> Secret {
    Secret::new(FAKE_KEY.to_owned()).unwrap()
}

fn allowlist(hosts: &[&str]) -> HostAllowlist {
    HostAllowlist::new(hosts).unwrap()
}

fn loopback_client() -> GuardedClient {
    GuardedClient::builder(allowlist(&["fake.test"]))
        .policy(TransportPolicy::LOOPBACK_TEST)
        .redactor(Redactor::new([&fake_key()]))
        .build()
        .unwrap()
}

fn public_client(hosts: &[&str]) -> GuardedClient {
    GuardedClient::builder(allowlist(hosts))
        .redactor(Redactor::new([&fake_key()]))
        .build()
        .unwrap()
}

fn url(text: &str) -> Url {
    text.parse().unwrap()
}

/// Reads one HTTP request (headers + Content-Length body) and returns it.
async fn read_request(stream: &mut TcpStream) -> String {
    let mut data = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        let read = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buffer))
            .await
            .unwrap()
            .unwrap();
        if read == 0 {
            break;
        }
        data.extend_from_slice(&buffer[..read]);
        let text = String::from_utf8_lossy(&data).to_string();
        if let Some(header_end) = text.find("\r\n\r\n") {
            let content_length = text[..header_end]
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            if data.len() >= header_end + 4 + content_length {
                return text;
            }
        }
    }
    String::from_utf8_lossy(&data).to_string()
}

/// Serves exactly one response on an ephemeral loopback port.
async fn serve_once(response: Vec<u8>) -> (String, JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let handle = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_request(&mut stream).await;
        stream.write_all(&response).await.unwrap();
        stream.flush().await.unwrap();
        request
    });
    (base, handle)
}

#[tokio::test]
async fn redirects_are_not_followed_and_credentials_do_not_reach_the_target() {
    let target = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target_url = format!("http://{}/steal", target.local_addr().unwrap());
    let client = loopback_client();
    let key = fake_key();
    for status in [
        "302 Found",
        "307 Temporary Redirect",
        "308 Permanent Redirect",
    ] {
        let (origin, server) = serve_once(
            format!(
                "HTTP/1.1 {status}\r\nLocation: {target_url}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .into_bytes(),
        )
        .await;
        let response = client
            .post(&url(&format!("{origin}/v1/messages")))
            .unwrap()
            .secret_header("x-api-key", &key)
            .unwrap()
            .bearer(&key)
            .unwrap()
            .body("{}")
            .send()
            .await
            .unwrap();

        assert!(response.status().is_redirection(), "{status}");
        let origin_request = server.await.unwrap();
        assert!(origin_request.contains("/v1/messages"), "{origin_request}");
        assert!(
            tokio::time::timeout(Duration::from_millis(200), target.accept())
                .await
                .is_err(),
            "{status}: redirect target received a connection"
        );
    }
}

#[tokio::test]
async fn rejects_bodies_larger_than_the_cap() {
    let body = "x".repeat(4096);
    let client = loopback_client();

    // No Content-Length: the cap must hold while streaming.
    let (base, server) =
        serve_once(format!("HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{body}").into_bytes())
            .await;
    let response = client.get(&url(&base)).unwrap().send().await.unwrap();
    let error = response.text(1024).await.unwrap_err();
    assert!(
        matches!(error, GuardError::BodyTooLarge { limit: 1024 }),
        "{error}"
    );
    assert!(error.to_string().contains("1024-byte limit"), "{error}");
    server.await.unwrap();

    // Declared Content-Length above the cap is rejected before reading.
    let (base, server) = serve_once(
        format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .into_bytes(),
    )
    .await;
    let response = client.get(&url(&base)).unwrap().send().await.unwrap();
    let error = response.json::<serde_json::Value>(1024).await.unwrap_err();
    assert!(
        matches!(error, GuardError::BodyTooLarge { limit: 1024 }),
        "{error}"
    );
    server.await.unwrap();
}

#[tokio::test]
async fn reads_bodies_within_the_cap() {
    let body = r#"{"ok":true}"#;
    let (base, server) = serve_once(
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .into_bytes(),
    )
    .await;
    let response = loopback_client()
        .post(&url(&base))
        .unwrap()
        .json(&serde_json::json!({ "q": 1 }))
        .unwrap()
        .send()
        .await
        .unwrap();
    let value: serde_json::Value = response.json(1024).await.unwrap();
    assert_eq!(value, serde_json::json!({ "ok": true }));
    let request = server.await.unwrap();
    assert!(
        request
            .to_ascii_lowercase()
            .contains("content-type: application/json"),
        "{request}"
    );
}

#[tokio::test]
async fn public_policy_refuses_plain_http() {
    let client = public_client(&["api.example.com"]);
    for refused in ["http://api.example.com/", "http://127.0.0.1:9/"] {
        let error = client.get(&url(refused)).err().unwrap();
        assert!(
            matches!(error, GuardError::InsecureScheme { .. }),
            "{refused}: {error}"
        );
    }
}

#[tokio::test]
async fn public_only_resolver_drops_local_answers() {
    let client = public_client(&["localhost"]);
    let error = client
        .get(&url("https://localhost/"))
        .unwrap()
        .send()
        .await
        .expect_err("localhost must not resolve for public destinations");
    assert!(matches!(error, GuardError::Connect(_)), "{error}");
    assert!(error.to_string().contains("private network"), "{error}");
}

#[tokio::test]
async fn request_refuses_hosts_off_the_allowlist_and_userinfo() {
    let client = public_client(&["api.brightdata.com"]);
    for (refused, check) in [
        (
            "https://evil.example.com/",
            (|e: &GuardError| matches!(e, GuardError::HostNotAllowed { .. }))
                as fn(&GuardError) -> bool,
        ),
        ("https://api.brightdata.com.evil.com/", |e| {
            matches!(e, GuardError::HostNotAllowed { .. })
        }),
        ("https://1.1.1.1/", |e| {
            matches!(e, GuardError::HostNotAllowed { .. })
        }),
        ("https://[::1]/", |e| {
            matches!(e, GuardError::HostNotAllowed { .. })
        }),
        ("https://user:pw@api.brightdata.com/", |e| {
            matches!(e, GuardError::InvalidUrl(_))
        }),
        ("https://api.brightdata.com:8443/", |e| {
            matches!(e, GuardError::InvalidUrl(_))
        }),
        ("ftp://api.brightdata.com/", |e| {
            matches!(e, GuardError::InsecureScheme { .. })
        }),
    ] {
        let error = client.get(&url(refused)).err().unwrap();
        assert!(check(&error), "{refused}: {error}");
        assert!(!error.is_transient());
    }
    assert!(client.get(&url("https://API.brightdata.com/x?y=1")).is_ok());
}

#[tokio::test]
async fn host_header_cannot_be_overridden() {
    let client = public_client(&["api.brightdata.com"]);
    let request = client.get(&url("https://api.brightdata.com/")).unwrap();
    let error = request.header("Host", "evil.example.com").err().unwrap();
    assert!(
        matches!(error, GuardError::ForbiddenHeader { .. }),
        "{error}"
    );
}

#[test]
fn truncates_long_error_messages() {
    let long = "e".repeat(http_guard::MAX_ERROR_MESSAGE_CHARS + 50);
    let truncated = http_guard::truncate_error_message(&long);
    assert_eq!(
        truncated.chars().count(),
        http_guard::MAX_ERROR_MESSAGE_CHARS + 1
    );
    assert!(truncated.ends_with('…'));
    let client = public_client(&["api.brightdata.com"]);
    let sanitized = client.sanitize(&format!("bad key {FAKE_KEY}: {long}"));
    assert!(!sanitized.contains(FAKE_KEY), "{sanitized}");
    assert!(sanitized.starts_with("bad key [redacted]"), "{sanitized}");
    assert!(sanitized.ends_with('…'));
}

#[tokio::test]
async fn secrets_sent_on_a_request_are_redacted_without_a_client_redactor() {
    let echoed = format!(
        "{{\"error\":\"invalid key {FAKE_KEY}\",\"pad\":\"{}\"}}",
        "p".repeat(900)
    );
    let (base, server) = serve_once(
        format!(
            "HTTP/1.1 401 Unauthorized\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{echoed}",
            echoed.len()
        )
        .into_bytes(),
    )
    .await;
    // No `.redactor(..)`: the secret is known only from `secret_header`.
    let client = GuardedClient::builder(allowlist(&["fake.test"]))
        .policy(TransportPolicy::LOOPBACK_TEST)
        .build()
        .unwrap();
    let key = fake_key();
    let response = client
        .get(&url(&format!("{base}/v1")))
        .unwrap()
        .secret_header("x-api-key", &key)
        .unwrap()
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 401);
    assert!(!response.sanitize(FAKE_KEY).contains(FAKE_KEY));
    let message = response
        .error_text(http_guard::SMALL_RESPONSE_BYTES)
        .await
        .unwrap();
    assert!(!message.contains(FAKE_KEY), "{message}");
    assert!(message.contains("invalid key [redacted]"), "{message}");
    assert!(message.ends_with('…'), "{message}");
    assert!(server.await.unwrap().contains(FAKE_KEY));
    // The client itself still has no secrets.
    assert_eq!(client.sanitize(FAKE_KEY), FAKE_KEY);
}

#[tokio::test]
async fn transport_errors_are_redacted_and_truncated() {
    // A port that was just freed: the connection is refused.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let key = fake_key();
    let error = loopback_client()
        .get(&url(&format!(
            "http://{address}/v1?key={FAKE_KEY}&marker=query-marker"
        )))
        .unwrap()
        .secret_header("x-api-key", &key)
        .unwrap()
        .send()
        .await
        .expect_err("closed port must fail");
    let message = error.to_string();
    assert!(error.is_transient(), "{message}");
    assert!(!message.contains(FAKE_KEY), "{message}");
    assert!(!message.contains("query-marker"), "{message}");
    assert!(
        message.chars().count() <= http_guard::MAX_ERROR_MESSAGE_CHARS + 30,
        "{message}"
    );
}
