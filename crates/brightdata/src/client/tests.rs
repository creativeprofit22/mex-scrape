//! Fixture tests against loopback fake servers (as linkgo `direct.rs`).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use http_guard::Secret;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

use super::{BrightDataClient, DatasetTrigger, Download, SnapshotStatus, UnlockRequest};
use crate::config::BrightDataConfig;
use crate::error::BrightDataError;
use crate::ids::{CountryCode, DatasetId, DiscoverBy, SnapshotId, TargetUrl};
use crate::rate::RateLimits;
use crate::retry::RetryPolicy;
use crate::transport::Transport;

const KEY: &str = "bd-test-key-0000000000";
const ZONE: &str = "zone-unlocker-test-0001";
const SNAPSHOT: &str = "sd_synthetic0000000001";

struct Reply {
    status: &'static str,
    headers: Vec<(&'static str, String)>,
    body: String,
}

fn reply(status: &'static str, body: &str) -> Reply {
    Reply {
        status,
        headers: Vec::new(),
        body: body.to_owned(),
    }
}

fn json_reply(status: &'static str, body: &Value) -> Reply {
    Reply {
        status,
        headers: vec![("Content-Type", "application/json".to_owned())],
        body: body.to_string(),
    }
}

/// A fake server answering one connection per reply, in order; extra
/// connections get `599` and are still recorded.
struct FakeServer {
    base: String,
    requests: Arc<Mutex<Vec<String>>>,
    task: JoinHandle<()>,
}

impl FakeServer {
    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for FakeServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

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

async fn serve(replies: Vec<Reply>) -> FakeServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&requests);
    let task = tokio::spawn(async move {
        let mut replies = replies.into_iter();
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_request(&mut stream).await;
            recorded.lock().unwrap().push(request);
            let reply = replies
                .next()
                .unwrap_or_else(|| reply("599 Unexpected", "unexpected request"));
            let mut head = format!("HTTP/1.1 {}\r\n", reply.status);
            for (name, value) in &reply.headers {
                head.push_str(&format!("{name}: {value}\r\n"));
            }
            head.push_str(&format!(
                "Content-Length: {}\r\nConnection: close\r\n\r\n",
                reply.body.len()
            ));
            stream.write_all(head.as_bytes()).await.unwrap();
            stream.write_all(reply.body.as_bytes()).await.unwrap();
            stream.flush().await.unwrap();
        }
    });
    FakeServer {
        base,
        requests,
        task,
    }
}

fn config() -> BrightDataConfig {
    let mut config = BrightDataConfig::new(
        Secret::new(KEY.to_owned()).unwrap(),
        Secret::new(ZONE.to_owned()).unwrap(),
    );
    config.unlocker_timeout = Duration::from_secs(5);
    config.datasets_timeout = Duration::from_secs(5);
    config.retry = RetryPolicy {
        max_retries: 3,
        base_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(5),
    };
    config.rate = RateLimits {
        unlocker_interval: Duration::ZERO,
        datasets_interval: Duration::ZERO,
    };
    config
}

fn client(server: &FakeServer) -> BrightDataClient {
    BrightDataClient::for_test(&server.base, config())
}

fn snapshot() -> SnapshotId {
    SnapshotId::parse(SNAPSHOT).unwrap()
}

fn trigger() -> DatasetTrigger {
    DatasetTrigger {
        dataset_id: DatasetId::parse("gd_l7q7dkf244hwjntr0").unwrap(),
        inputs: vec![json!({ "keyword": "audifonos" })],
        discover_by: Some(DiscoverBy::parse("keyword").unwrap()),
        limit_per_input: Some(20),
    }
}

fn body_of(request: &str) -> Value {
    let (_, body) = request.split_once("\r\n\r\n").unwrap();
    serde_json::from_str(body).unwrap()
}

#[test]
fn production_client_uses_the_fixed_host() {
    let client = BrightDataClient::new(config()).unwrap();
    assert_eq!(client.base.as_str(), "https://api.brightdata.com/");
}

#[tokio::test]
async fn unlocker_request_shape() {
    let server = serve(vec![reply("200 OK", "<html>precio</html>")]).await;
    let page = client(&server)
        .unlock(&UnlockRequest {
            url: TargetUrl::parse("https://www.liverpool.com.mx/tienda/pdp/1").unwrap(),
            country: Some(CountryCode::MX),
        })
        .await
        .unwrap();
    assert_eq!(page.status, 200);
    assert_eq!(page.body, "<html>precio</html>");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].starts_with("POST /request "), "{}", requests[0]);
    let lower = requests[0].to_ascii_lowercase();
    assert!(
        lower.contains(&format!(
            "authorization: bearer {}",
            KEY.to_ascii_lowercase()
        )),
        "{lower}"
    );
    assert!(lower.contains("content-type: application/json"), "{lower}");
    assert_eq!(
        body_of(&requests[0]),
        json!({
            "zone": ZONE,
            "url": "https://www.liverpool.com.mx/tienda/pdp/1",
            "format": "raw",
            "country": "mx"
        })
    );
}

#[tokio::test]
async fn trigger_running_ready_download_flow() {
    let server = serve(vec![
        json_reply("200 OK", &json!({ "snapshot_id": SNAPSHOT })),
        json_reply("200 OK", &json!({ "status": "running" })),
        reply("409 Conflict", r#"{"status":"building"}"#),
        reply("202 Accepted", r#"{"status":"building"}"#),
        json_reply("200 OK", &json!({ "status": "ready" })),
        json_reply("200 OK", &json!([{ "asin": "B0001" }, { "asin": "B0002" }])),
    ])
    .await;
    let client = client(&server);
    let id = client.trigger(&trigger()).await.unwrap();
    assert_eq!(id.as_str(), SNAPSHOT);
    assert_eq!(client.progress(&id).await.unwrap(), SnapshotStatus::Running);
    assert_eq!(client.download(&id).await.unwrap(), Download::NotReady);
    assert_eq!(client.download(&id).await.unwrap(), Download::NotReady);
    assert_eq!(client.progress(&id).await.unwrap(), SnapshotStatus::Ready);
    let Download::Records(records) = client.download(&id).await.unwrap() else {
        panic!("expected records");
    };
    assert_eq!(records.len(), 2);

    let requests = server.requests();
    assert_eq!(requests.len(), 6);
    assert!(
        requests[0].starts_with(
            "POST /datasets/v3/trigger?dataset_id=gd_l7q7dkf244hwjntr0&include_errors=true\
             &type=discover_new&discover_by=keyword&limit_per_input=20 "
        ),
        "{}",
        requests[0]
    );
    assert_eq!(body_of(&requests[0]), json!([{ "keyword": "audifonos" }]));
    assert!(requests[1].starts_with(&format!("GET /datasets/v3/progress/{SNAPSHOT} ")));
    assert!(requests[2].starts_with(&format!(
        "GET /datasets/v3/snapshot/{SNAPSHOT}?format=json "
    )));
}

#[tokio::test]
async fn cancel_posts_to_the_snapshot() {
    let server = serve(vec![reply("200 OK", "OK")]).await;
    client(&server).cancel(&snapshot()).await.unwrap();
    let requests = server.requests();
    assert!(requests[0].starts_with(&format!("POST /datasets/v3/snapshot/{SNAPSHOT}/cancel ")));
}

#[tokio::test]
async fn status_400_is_terminal_and_not_retried() {
    let server = serve(vec![json_reply(
        "400 Bad Request",
        &json!({ "error": "dataset_id is invalid" }),
    )])
    .await;
    let error = client(&server).download(&snapshot()).await.unwrap_err();
    assert_eq!(
        error,
        BrightDataError::Terminal(
            "Bright Data request failed with HTTP 400: dataset_id is invalid".to_owned()
        )
    );
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn server_errors_are_retried_then_succeed() {
    let server = serve(vec![
        reply("500 Internal Server Error", "boom"),
        reply("503 Service Unavailable", "busy"),
        json_reply("200 OK", &json!({ "status": "ready" })),
    ])
    .await;
    let status = client(&server).progress(&snapshot()).await.unwrap();
    assert_eq!(status, SnapshotStatus::Ready);
    assert_eq!(server.requests().len(), 3);
}

#[tokio::test]
async fn persistent_server_errors_end_transient_after_max_retries() {
    let replies = (0..4)
        .map(|_| reply("502 Bad Gateway", "upstream"))
        .collect();
    let server = serve(replies).await;
    let error = client(&server).progress(&snapshot()).await.unwrap_err();
    assert!(error.is_transient(), "{error:?}");
    assert!(error.message().contains("HTTP 502: upstream"), "{error:?}");
    assert_eq!(server.requests().len(), 4);
}

#[tokio::test]
async fn trigger_is_not_retried_on_server_error_but_is_on_429() {
    let server = serve(vec![reply("500 Internal Server Error", "boom")]).await;
    let error = client(&server).trigger(&trigger()).await.unwrap_err();
    assert!(error.is_transient(), "{error:?}");
    assert_eq!(server.requests().len(), 1);

    let mut limited = reply("429 Too Many Requests", "slow down");
    limited.headers.push(("Retry-After", "0".to_owned()));
    let server = serve(vec![
        limited,
        json_reply("200 OK", &json!({ "snapshot_id": SNAPSHOT })),
    ])
    .await;
    let id = client(&server).trigger(&trigger()).await.unwrap();
    assert_eq!(id.as_str(), SNAPSHOT);
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn key_and_zone_are_redacted_from_errors() {
    let mut unauthorized = json_reply(
        "401 Unauthorized",
        &json!({ "error": format!("bad key {KEY} for zone {ZONE}") }),
    );
    unauthorized
        .headers
        .push(("x-brd-error", format!("zone {ZONE} not active")));
    let server = serve(vec![unauthorized]).await;
    let error = client(&server)
        .unlock(&UnlockRequest {
            url: TargetUrl::parse("https://www.walmart.com.mx/ip/1").unwrap(),
            country: None,
        })
        .await
        .unwrap_err();
    let message = error.message();
    assert!(matches!(error, BrightDataError::Terminal(_)), "{error:?}");
    assert!(!message.contains(KEY), "{message}");
    assert!(!message.contains(ZONE), "{message}");
    assert!(!message.contains("walmart"), "{message}");
    assert!(
        message.contains("zone [redacted] not active; bad key [redacted] for zone [redacted]"),
        "{message}"
    );
    // The secrets did go out on the wire, only to the fake server.
    assert!(server.requests()[0].contains(KEY));
}

#[tokio::test]
async fn unsafe_snapshot_ids_from_the_server_are_rejected() {
    let server = serve(vec![json_reply(
        "200 OK",
        &json!({ "snapshot_id": "../../account" }),
    )])
    .await;
    let error = client(&server).trigger(&trigger()).await.unwrap_err();
    assert_eq!(
        error,
        BrightDataError::Terminal("Bright Data returned an invalid snapshot id".to_owned())
    );
    // Nothing followed the bad id.
    assert_eq!(server.requests().len(), 1);
    for bad in ["", "../x", "a/b", "a?b", "a b", &"a".repeat(201)] {
        assert!(SnapshotId::parse(bad).is_err(), "{bad:?}");
    }
}

#[tokio::test]
async fn unknown_progress_and_bad_shapes_are_terminal() {
    let server = serve(vec![
        json_reply("200 OK", &json!({ "status": "mystery" })),
        json_reply("200 OK", &json!({ "records": [] })),
        reply("200 OK", "not json"),
    ])
    .await;
    let client = client(&server);
    let id = snapshot();
    assert_eq!(
        client.progress(&id).await.unwrap_err().message(),
        "Bright Data returned an unknown snapshot status"
    );
    assert_eq!(
        client.download(&id).await.unwrap_err().message(),
        "Bright Data returned an unexpected snapshot shape"
    );
    assert_eq!(
        client.progress(&id).await.unwrap_err().message(),
        "Bright Data returned a response that was not JSON"
    );
    assert_eq!(server.requests().len(), 3);
}

#[tokio::test]
async fn invalid_trigger_inputs_fail_before_any_request() {
    let server = serve(Vec::new()).await;
    let client = client(&server);
    let mut empty = trigger();
    empty.inputs.clear();
    assert!(client.trigger(&empty).await.is_err());
    let mut not_object = trigger();
    not_object.inputs = vec![json!("https://example.com")];
    assert!(client.trigger(&not_object).await.is_err());
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn connection_failures_are_transient() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    // Refused connections are slow on some platforms; one attempt is enough.
    let mut config = config();
    config.retry.max_retries = 0;
    let error = BrightDataClient::for_test(&base, config)
        .progress(&snapshot())
        .await
        .unwrap_err();
    assert!(error.is_transient(), "{error:?}");
    assert!(!error.message().contains(SNAPSHOT), "{error:?}");
}
