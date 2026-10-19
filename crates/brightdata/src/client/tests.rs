//! Fixture tests against loopback fake servers (as linkgo `direct.rs`).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use http_guard::Secret;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

use super::{BrightDataClient, DatasetTrigger, Download, SnapshotStatus, UnlockRequest};
use crate::config::{BrightDataConfig, BrowserSettle};
use crate::error::BrightDataError;
use crate::ids::{CountryCode, DatasetId, DiscoverBy, SnapshotId, TargetUrl};
use crate::rate::RateLimits;
use crate::refusal::RefusalReason;
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
        browser_interval: Duration::ZERO,
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

#[tokio::test]
async fn kyc_refusals_are_blocked_and_never_retried() {
    for (status, code, reason) in [
        (
            "400 Bad Request",
            "ub_bad_endpoint_robots",
            RefusalReason::RobotsDisallowed,
        ),
        ("403 Forbidden", "policy_20050", RefusalReason::Policy),
    ] {
        let mut refusal = reply(status, "refused");
        refusal.headers.push(("X-Brd-Error-Code", code.to_owned()));
        let server = serve(vec![refusal]).await;
        let error = client(&server)
            .unlock(&UnlockRequest {
                url: TargetUrl::parse("https://www.walmart.com.mx/ip/1").unwrap(),
                country: None,
            })
            .await
            .unwrap_err();
        assert!(error.is_blocked_needs_kyc(), "{error:?}");
        assert_eq!(error.refusal_reason(), Some(reason), "{error:?}");
        assert!(!error.is_transient());
        assert!(error.message().contains(code), "{error:?}");
        assert!(!error.message().contains("walmart"), "{error:?}");
        assert_eq!(server.requests().len(), 1);
    }
}

#[tokio::test]
async fn plain_403_without_policy_code_stays_terminal() {
    let server = serve(vec![reply("403 Forbidden", "nope")]).await;
    let error = client(&server).progress(&snapshot()).await.unwrap_err();
    assert!(matches!(error, BrightDataError::Terminal(_)), "{error:?}");
}

mod browser {
    use super::*;
    use http_guard::test_support::{FakePeer, FakeWebSocketServer, serve as serve_ws};

    const BROWSER_ZONE: &str = "zone-browser-test-0001";
    const PASSWORD: &str = "pw-browser-0001";
    /// base64("brd-customer-hl_test01-zone-zone-browser-test-0001-country-mx:pw-browser-0001")
    const BASIC: &str = "Basic YnJkLWN1c3RvbWVyLWhsX3Rlc3QwMS16b25lLXpvbmUtYnJvd3Nlci10ZXN0LTAwMDEtY291bnRyeS1teDpwdy1icm93c2VyLTAwMDE=";

    /// The fake page's HTML: its current element count and how many settle
    /// checks the client made before reading it.
    fn page_html(elements: u64, probes: usize) -> String {
        format!("<html><body data-elements=\"{elements}\" data-probes=\"{probes}\"></body></html>")
    }

    fn credential_replies() -> Vec<Reply> {
        vec![
            json_reply(
                "200 OK",
                &json!({ "status": "active", "customer": "hl_test01" }),
            ),
            json_reply("200 OK", &json!({ "passwords": [PASSWORD] })),
        ]
    }

    fn browser_client(api: &FakeServer, ws: &FakeWebSocketServer) -> BrightDataClient {
        browser_client_settling(api, ws, 60)
    }

    fn browser_client_settling(
        api: &FakeServer,
        ws: &FakeWebSocketServer,
        max_polls: u32,
    ) -> BrightDataClient {
        let mut config =
            config().with_browser_zone(Some(Secret::new(BROWSER_ZONE.to_owned()).unwrap()));
        config.browser_settle = BrowserSettle {
            poll_interval: Duration::ZERO,
            quiet_polls: 3,
            max_polls,
        };
        BrightDataClient::for_test_browser(&api.base, &ws.url, config)
    }

    fn request() -> UnlockRequest {
        UnlockRequest {
            url: TargetUrl::parse("https://www.sears.com.mx/categoria/4714/licuadoras/").unwrap(),
            country: Some(CountryCode::MX),
        }
    }

    /// How the fake page behaves.
    #[derive(Clone, Copy)]
    struct Script {
        /// `Page.navigate` fails with this CDP error message.
        navigate_error: Option<&'static str>,
        /// Element count seen by settle check `n` (1-based) is
        /// `counts[n - 1]`; the last entry repeats.
        counts: &'static [u64],
        /// A request starts at `load` and finishes just before this settle
        /// check (`usize::MAX`: never).
        request_until_probe: Option<usize>,
        /// The client should send `Network.setBlockedURLs`.
        expect_blocking: bool,
    }

    const STEADY: Script = Script {
        navigate_error: None,
        counts: &[10],
        request_until_probe: None,
        expect_blocking: true,
    };

    fn failing(error: &'static str) -> Script {
        Script {
            navigate_error: Some(error),
            ..STEADY
        }
    }

    fn event(method: &str, params: Value) -> String {
        json!({ "method": method, "sessionId": "S1", "params": params }).to_string()
    }

    /// A scripted Chrome speaking CDP over the fake WebSocket.
    async fn chrome(mut peer: FakePeer, script: Script) {
        let mut network_enabled = false;
        let mut blocked = false;
        let mut probes = 0usize;
        let count_at = |probes: usize| {
            let index = probes.saturating_sub(1).min(script.counts.len() - 1);
            script.counts[index]
        };
        while let Some(text) = peer.recv().await {
            let message: Value = serde_json::from_str(&text).unwrap();
            let id = message["id"].clone();
            let method = message["method"].as_str().unwrap().to_owned();
            let session = message.get("sessionId").cloned();
            let reply = |result: Value| json!({ "id": id, "result": result }).to_string();
            match method.as_str() {
                "Target.createTarget" => peer.send(&reply(json!({ "targetId": "T1" }))).await,
                "Target.attachToTarget" => {
                    assert_eq!(message["params"]["flatten"], json!(true));
                    peer.send(&reply(json!({ "sessionId": "S1" }))).await;
                }
                "Page.enable" => {
                    assert_eq!(session, Some(json!("S1")));
                    peer.send(&reply(json!({}))).await;
                }
                "Network.enable" => {
                    assert_eq!(session, Some(json!("S1")));
                    network_enabled = true;
                    peer.send(&reply(json!({}))).await;
                }
                "Network.setBlockedURLs" => {
                    assert!(script.expect_blocking, "blocking was switched off");
                    assert!(network_enabled, "Network.enable must come first");
                    let urls = message["params"]["urls"].as_array().unwrap();
                    for pattern in ["*.png*", "*.jpg*", "*.woff*", "*.css*", "*.mp4*"] {
                        assert!(urls.contains(&json!(pattern)), "{pattern} not blocked");
                    }
                    blocked = true;
                    peer.send(&reply(json!({}))).await;
                }
                "Page.navigate" => {
                    if let Some(error) = script.navigate_error {
                        let body =
                            json!({ "id": id, "error": { "code": -32000, "message": error } });
                        peer.send(&body.to_string()).await;
                        continue;
                    }
                    assert_eq!(
                        blocked, script.expect_blocking,
                        "downloads must be blocked before navigating"
                    );
                    assert_eq!(
                        message["params"]["url"],
                        json!("https://www.sears.com.mx/categoria/4714/licuadoras/")
                    );
                    // The load event may arrive before the navigate reply.
                    peer.send(&event("Page.loadEventFired", json!({}))).await;
                    if script.request_until_probe.is_some() {
                        let started = json!({ "requestId": "R1" });
                        peer.send(&event("Network.requestWillBeSent", started))
                            .await;
                    }
                    peer.send(&reply(json!({ "frameId": "F1" }))).await;
                }
                "Runtime.evaluate" => {
                    let expression = message["params"]["expression"].as_str().unwrap();
                    if expression == "document.documentElement.outerHTML" {
                        let html = page_html(count_at(probes), probes);
                        let result = json!({ "result": { "type": "string", "value": html } });
                        peer.send(&reply(result)).await;
                    } else {
                        assert_eq!(expression, "document.getElementsByTagName('*').length");
                        probes += 1;
                        if script.request_until_probe == Some(probes) {
                            let finished = json!({ "requestId": "R1" });
                            peer.send(&event("Network.loadingFinished", finished)).await;
                        }
                        let count = count_at(probes);
                        let result = json!({ "result": { "type": "number", "value": count } });
                        peer.send(&reply(result)).await;
                    }
                }
                "Target.closeTarget" => peer.send(&reply(json!({ "success": true }))).await,
                other => panic!("unexpected CDP method {other}"),
            }
        }
    }

    #[tokio::test]
    async fn fetches_credentials_then_renders_over_cdp() {
        let api = serve(credential_replies()).await;
        let mut ws = serve_ws(|peer| chrome(peer, STEADY)).await;
        let page = browser_client(&api, &ws).browse(&request()).await.unwrap();
        assert_eq!(page.status, 200);
        // A steady page: a baseline check plus three quiet ones.
        assert_eq!(page.body, page_html(10, 4));
        ws.finished().await;

        let requests = api.requests();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].starts_with("GET /status "), "{}", requests[0]);
        assert!(
            requests[1].starts_with(&format!("GET /zone/passwords?zone={BROWSER_ZONE} ")),
            "{}",
            requests[1]
        );
        for request in &requests {
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains(&format!("authorization: bearer {KEY}"))
            );
        }
        let headers = ws.headers();
        assert!(
            headers
                .iter()
                .any(|(name, value)| name == "authorization" && value == BASIC),
            "{headers:?}"
        );
        assert!(!ws.url.contains(PASSWORD));
    }

    #[tokio::test]
    async fn waits_for_content_built_after_load_before_reading_html() {
        // The product list appears after `load` (10 → 40 elements); reading
        // the HTML at `load` returned the empty listing.
        let api = serve(credential_replies()).await;
        let script = Script {
            counts: &[10, 40],
            ..STEADY
        };
        let ws = serve_ws(move |peer| chrome(peer, script)).await;
        let page = browser_client(&api, &ws).browse(&request()).await.unwrap();
        assert_eq!(page.body, page_html(40, 5));
    }

    #[tokio::test]
    async fn blocking_can_be_switched_off() {
        let api = serve(credential_replies()).await;
        let script = Script {
            expect_blocking: false,
            ..STEADY
        };
        let mut ws = serve_ws(move |peer| chrome(peer, script)).await;
        let mut config =
            config().with_browser_zone(Some(Secret::new(BROWSER_ZONE.to_owned()).unwrap()));
        config.browser_settle.poll_interval = Duration::ZERO;
        config.browser_block_downloads = false;
        let client = BrightDataClient::for_test_browser(&api.base, &ws.url, config);
        let page = client.browse(&request()).await.unwrap();
        assert_eq!(page.body, page_html(10, 4));
        ws.finished().await;
    }

    #[tokio::test]
    async fn waits_while_a_request_is_in_flight() {
        let api = serve(credential_replies()).await;
        let script = Script {
            request_until_probe: Some(6),
            ..STEADY
        };
        let ws = serve_ws(move |peer| chrome(peer, script)).await;
        let page = browser_client(&api, &ws).browse(&request()).await.unwrap();
        // Checks 1-5 see the request in flight; 6-8 are quiet.
        assert_eq!(page.body, page_html(10, 8));
    }

    #[tokio::test]
    async fn reads_a_page_that_never_settles_after_max_polls() {
        let api = serve(credential_replies()).await;
        let script = Script {
            request_until_probe: Some(usize::MAX),
            ..STEADY
        };
        let ws = serve_ws(move |peer| chrome(peer, script)).await;
        let page = browser_client_settling(&api, &ws, 5)
            .browse(&request())
            .await
            .unwrap();
        assert_eq!(page.body, page_html(10, 5));
    }

    #[tokio::test]
    async fn brob_navigation_error_is_blocked_needs_kyc() {
        let api = serve(credential_replies()).await;
        let ws =
            serve_ws(|peer| chrome(peer, failing("Protocol error (Page.navigate): brob"))).await;
        let error = browser_client(&api, &ws)
            .browse(&request())
            .await
            .unwrap_err();
        assert_eq!(error.refusal_reason(), Some(RefusalReason::BrowserRobots));
        assert!(!error.message().contains(PASSWORD));
        assert!(!error.message().contains(BROWSER_ZONE));
    }

    #[tokio::test]
    async fn other_cdp_errors_are_terminal_and_redacted() {
        let api = serve(credential_replies()).await;
        let ws = serve_ws(|peer| chrome(peer, failing("net::ERR_FAILED pw-browser-0001"))).await;
        let error = browser_client(&api, &ws)
            .browse(&request())
            .await
            .unwrap_err();
        assert!(matches!(error, BrightDataError::Terminal(_)), "{error:?}");
        assert!(!error.message().contains(PASSWORD), "{}", error.message());
    }

    #[tokio::test]
    async fn unknown_zone_is_terminal_without_retry() {
        let api = serve(vec![
            json_reply("200 OK", &json!({ "customer": "hl_test01" })),
            json_reply(
                "422 Unprocessable Entity",
                &json!({ "error": "zone not found" }),
            ),
        ])
        .await;
        let ws = serve_ws(|_peer| async {}).await;
        let error = browser_client(&api, &ws)
            .browse(&request())
            .await
            .unwrap_err();
        assert!(matches!(error, BrightDataError::Terminal(_)), "{error:?}");
        assert!(!error.message().contains(BROWSER_ZONE));
        assert_eq!(api.requests().len(), 2);
    }

    #[tokio::test]
    async fn without_a_browser_zone_nothing_is_sent() {
        let api = serve(Vec::new()).await;
        let ws = serve_ws(|_peer| async {}).await;
        let client = BrightDataClient::for_test_browser(&api.base, &ws.url, config());
        let error = client.browse(&request()).await.unwrap_err();
        assert_eq!(error.message(), "Bright Data Browser API is not configured");
        assert!(api.requests().is_empty());
    }
}
