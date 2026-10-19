//! Loopback fake-server tests with recorded fixtures (synthetic ids and
//! credentials only).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use brightdata::RetryPolicy;
use domain::{Asin, Gtin, PriceMxn};
use http_guard::Secret;
use serde_json::Value;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

use super::SpApiClient;
use crate::config::{Operation, RateLimit, RateTable, SpApiConfig};
use crate::error::SpApiError;

const CLIENT_ID: &str = "amzn1.application-oa2-client.synthetic0001";
const CLIENT_SECRET: &str = "synthetic-client-secret-0001";
const REFRESH: &str = "Atzr|synthetic-refresh-token-0001";
const ACCESS: &str = "Atza|synthetic-access-token-0001";

const TOKEN: &str = include_str!("../../tests/fixtures/lwa_token.json");
const CATALOG_UPC: &str = include_str!("../../tests/fixtures/catalog_by_upc.json");
const CATALOG_KEYWORD: &str = include_str!("../../tests/fixtures/catalog_by_keyword.json");
const OFFERS: &str = include_str!("../../tests/fixtures/item_offers.json");
const FEES: &str = include_str!("../../tests/fixtures/fees_estimate.json");
const ERROR_429: &str = include_str!("../../tests/fixtures/error_429.json");
const ERROR_403: &str = include_str!("../../tests/fixtures/error_403.json");

struct Reply {
    status: &'static str,
    headers: Vec<(&'static str, String)>,
    body: String,
}

fn reply(status: &'static str, body: &str) -> Reply {
    Reply {
        status,
        headers: vec![("Content-Type", "application/json".to_owned())],
        body: body.to_owned(),
    }
}

fn token_reply() -> Reply {
    reply("200 OK", TOKEN)
}

/// Answers one connection per reply, in order; extra connections get 599.
struct FakeServer {
    base: String,
    requests: Arc<Mutex<Vec<String>>>,
    task: JoinHandle<()>,
}

impl FakeServer {
    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }

    fn client(&self, config: SpApiConfig) -> SpApiClient {
        SpApiClient::for_test(&self.base, &format!("{}/auth/o2/token", self.base), config)
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

fn secret(value: &str) -> Option<Secret> {
    Some(Secret::new(value.to_owned()).unwrap())
}

const NO_WAIT: RateLimit = RateLimit {
    per_request: Duration::ZERO,
    burst: 1,
};

fn config() -> SpApiConfig {
    let mut config =
        SpApiConfig::from_secrets(secret(CLIENT_ID), secret(CLIENT_SECRET), secret(REFRESH))
            .unwrap();
    config.timeout = Duration::from_secs(5);
    config.retry = RetryPolicy {
        max_retries: 3,
        base_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(5),
    };
    config.rates = RateTable {
        search_catalog: NO_WAIT,
        item_offers: NO_WAIT,
        fees_estimate: NO_WAIT,
    };
    config
}

fn upc() -> Gtin {
    Gtin::new("012345678905").unwrap()
}

fn asin() -> Asin {
    Asin::new("B0SYNTH001").unwrap()
}

fn lower(request: &str) -> String {
    request.to_ascii_lowercase()
}

fn body_of(request: &str) -> &str {
    request.split_once("\r\n\r\n").map_or("", |(_, body)| body)
}

fn assert_no_secrets(text: &str) {
    for secret in [CLIENT_ID, CLIENT_SECRET, REFRESH, ACCESS] {
        assert!(!text.contains(secret), "{secret} leaked in {text}");
    }
}

#[test]
fn production_client_targets_the_na_endpoint() {
    let client = SpApiClient::new(config()).unwrap();
    assert_eq!(
        client.base.as_str(),
        "https://sellingpartnerapi-na.amazon.com/"
    );
    let sandbox = SpApiClient::new(config().with_endpoint(crate::SpApiEndpoint::Sandbox)).unwrap();
    assert_eq!(
        sandbox.base.as_str(),
        "https://sandbox.sellingpartnerapi-na.amazon.com/"
    );
}

#[tokio::test]
async fn token_is_fetched_once_and_sent_on_every_call() {
    let server = serve(vec![
        token_reply(),
        reply("200 OK", CATALOG_UPC),
        reply("200 OK", CATALOG_KEYWORD),
    ])
    .await;
    let client = server.client(config());
    let by_upc = client
        .search_catalog_by_identifiers(&[upc()])
        .await
        .unwrap();
    assert_eq!(by_upc.len(), 1);
    assert_eq!(by_upc[0].title, "Taladro Inalámbrico 20V Sintético");
    let by_keyword = client
        .search_catalog_by_keyword("  taladro   inalámbrico ")
        .await
        .unwrap();
    assert_eq!(by_keyword.len(), 2);

    let requests = server.requests();
    assert_eq!(requests.len(), 3, "one token refresh, two searches");
    let token = &requests[0];
    assert!(token.starts_with("POST /auth/o2/token "), "{token}");
    assert!(
        lower(token).contains("content-type: application/x-www-form-urlencoded"),
        "{token}"
    );
    let form = body_of(token);
    assert!(
        form.starts_with("grant_type=refresh_token&refresh_token="),
        "{form}"
    );
    assert!(
        form.contains("&client_id=amzn1.application-oa2-client.synthetic0001"),
        "{form}"
    );

    let upc_call = &requests[1];
    assert!(
        upc_call.starts_with("GET /catalog/2022-04-01/items?"),
        "{upc_call}"
    );
    for part in [
        "marketplaceIds=A1AM78C64UM0Y8",
        "identifiers=012345678905",
        "identifiersType=UPC",
        "includedData=summaries%2Cimages%2CsalesRanks%2Cidentifiers",
        "locale=es_MX",
    ] {
        assert!(upc_call.contains(part), "{part} missing: {upc_call}");
    }
    for call in &requests[1..] {
        let l = lower(call);
        assert!(
            l.contains(&format!("x-amz-access-token: {}", lower(ACCESS))),
            "{call}"
        );
        assert!(l.contains("user-agent: mex-scrape/"), "{call}");
        assert!(call.contains("marketplaceIds=A1AM78C64UM0Y8"), "{call}");
    }
    assert!(
        requests[2].contains("keywords=taladro+inal%C3%A1mbrico"),
        "{}",
        requests[2]
    );
}

#[tokio::test]
async fn token_is_refreshed_shortly_before_it_expires() {
    let short = TOKEN.replace("3600", "1");
    let server = serve(vec![
        reply("200 OK", &short),
        reply("200 OK", CATALOG_UPC),
        reply("200 OK", &short),
        reply("200 OK", CATALOG_UPC),
    ])
    .await;
    let client = server.client(config());
    client
        .search_catalog_by_identifiers(&[upc()])
        .await
        .unwrap();
    // 1 s lifetime: refreshed after half of it.
    tokio::time::sleep(Duration::from_millis(600)).await;
    client
        .search_catalog_by_identifiers(&[upc()])
        .await
        .unwrap();
    let paths: Vec<String> = server
        .requests()
        .iter()
        .map(|r| {
            r.split(' ')
                .nth(1)
                .unwrap_or_default()
                .split('?')
                .next()
                .unwrap_or_default()
                .to_owned()
        })
        .collect();
    assert_eq!(
        paths,
        [
            "/auth/o2/token",
            "/catalog/2022-04-01/items",
            "/auth/o2/token",
            "/catalog/2022-04-01/items"
        ]
    );
}

#[tokio::test]
async fn ean_and_gtin14_pick_the_identifier_type() {
    let server = serve(vec![token_reply(), reply("200 OK", r#"{"items":[]}"#)]).await;
    let client = server.client(config());
    let ean = Gtin::new("7501055363063").unwrap();
    assert!(
        client
            .search_catalog_by_identifiers(&[ean])
            .await
            .unwrap()
            .is_empty()
    );
    assert!(server.requests()[1].contains("identifiersType=EAN"));
}

#[tokio::test]
async fn invalid_inputs_fail_before_any_request() {
    let server = serve(vec![]).await;
    let client = server.client(config());
    let ean = Gtin::new("7501055363063").unwrap();
    for gtins in [vec![], vec![upc(), ean], vec![upc(); 21]] {
        assert!(matches!(
            client.search_catalog_by_identifiers(&gtins).await,
            Err(SpApiError::Invalid(_))
        ));
    }
    for bad in ["", "   ", "a\u{7}b", &"x".repeat(201)] {
        assert!(matches!(
            client.search_catalog_by_keyword(bad).await,
            Err(SpApiError::Invalid(_))
        ));
    }
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn offers_use_the_mx_marketplace_and_new_condition() {
    let server = serve(vec![token_reply(), reply("200 OK", OFFERS)]).await;
    let client = server.client(config());
    let prices = client.item_offers(&asin()).await.unwrap();
    assert_eq!(
        prices.buy_box_mxn,
        Some(PriceMxn::from_centavos(129_900).unwrap())
    );
    assert_eq!(prices.offer_count, 3);
    let call = &server.requests()[1];
    assert!(
        call.starts_with(
            "GET /products/pricing/v0/items/B0SYNTH001/offers?MarketplaceId=A1AM78C64UM0Y8&ItemCondition=New "
        ),
        "{call}"
    );
}

#[tokio::test]
async fn not_found_is_empty() {
    let server = serve(vec![
        token_reply(),
        reply(
            "404 Not Found",
            r#"{"errors":[{"code":"NotFound","message":"x"}]}"#,
        ),
        reply("404 Not Found", r#"{"errors":[]}"#),
    ])
    .await;
    let client = server.client(config());
    assert!(
        client
            .search_catalog_by_keyword("nada")
            .await
            .unwrap()
            .is_empty()
    );
    let prices = client.item_offers(&asin()).await.unwrap();
    assert_eq!(prices.offer_count, 0);
    assert!(prices.buy_box_mxn.is_none());
}

#[tokio::test]
async fn fees_request_mxn_and_reject_usd_answers() {
    let server = serve(vec![
        token_reply(),
        reply("200 OK", FEES),
        reply("200 OK", &FEES.replace("MXN", "USD")),
    ])
    .await;
    let client = server.client(config());
    let price = PriceMxn::from_centavos(129_900).unwrap();
    let fees = client.fees_estimate(&asin(), price, true).await.unwrap();
    assert_eq!(fees.total_mxn.centavos(), 25_485);
    let usd = client.fees_estimate(&asin(), price, false).await;
    assert!(matches!(usd, Err(SpApiError::Parse(_))), "{usd:?}");

    let requests = server.requests();
    let call = &requests[1];
    assert!(
        call.starts_with("POST /products/fees/v0/items/B0SYNTH001/feesEstimate "),
        "{call}"
    );
    let body: Value = serde_json::from_str(body_of(call)).unwrap();
    let request = &body["FeesEstimateRequest"];
    assert_eq!(request["MarketplaceId"], "A1AM78C64UM0Y8");
    assert_eq!(request["IsAmazonFulfilled"], true);
    assert_eq!(
        request["PriceToEstimateFees"]["ListingPrice"]["CurrencyCode"],
        "MXN"
    );
    assert_eq!(
        request["PriceToEstimateFees"]["ListingPrice"]["Amount"].to_string(),
        "1299.0"
    );
    let second: Value = serde_json::from_str(body_of(&requests[2])).unwrap();
    assert_eq!(second["FeesEstimateRequest"]["IsAmazonFulfilled"], false);
}

#[tokio::test]
async fn throttled_calls_back_off_then_succeed() {
    let mut throttled = reply("429 Too Many Requests", ERROR_429);
    throttled
        .headers
        .push(("x-amzn-RateLimit-Limit", "20".to_owned()));
    let server = serve(vec![
        token_reply(),
        throttled,
        reply("503 Service Unavailable", "{}"),
        reply("200 OK", CATALOG_UPC),
    ])
    .await;
    let client = server.client(config());
    let items = client
        .search_catalog_by_identifiers(&[upc()])
        .await
        .unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(server.requests().len(), 4);
    // The slower rate Amazon reported was adopted.
    let limit = client
        .limiter
        .limit(Operation::SearchCatalog)
        .await
        .unwrap();
    assert_eq!(limit.per_request, Duration::from_millis(50));
}

#[tokio::test]
async fn persistent_429_ends_rate_limited_after_the_retries() {
    let server = serve(vec![
        token_reply(),
        reply("429 Too Many Requests", ERROR_429),
        reply("429 Too Many Requests", ERROR_429),
        reply("429 Too Many Requests", ERROR_429),
        reply("429 Too Many Requests", ERROR_429),
    ])
    .await;
    let client = server.client(config());
    let error = client.item_offers(&asin()).await.unwrap_err();
    assert!(matches!(error, SpApiError::RateLimited(_)), "{error:?}");
    assert!(error.to_string().contains("QuotaExceeded"), "{error}");
    assert_eq!(server.requests().len(), 5, "token + 4 attempts");
}

#[tokio::test]
async fn unauthorized_refreshes_once_then_fails_not_authorized() {
    let echo =
        format!(r#"{{"errors":[{{"code":"Unauthorized","message":"bad token {ACCESS}"}}]}}"#);
    let server = serve(vec![
        token_reply(),
        reply("401 Unauthorized", &echo),
        token_reply(),
        reply("401 Unauthorized", &echo),
    ])
    .await;
    let client = server.client(config());
    let error = client
        .search_catalog_by_keyword("taladro")
        .await
        .unwrap_err();
    assert!(matches!(error, SpApiError::NotAuthorized(_)), "{error:?}");
    assert_no_secrets(&error.to_string());
    assert_eq!(server.requests().len(), 4);
}

#[tokio::test]
async fn forbidden_is_not_authorized_without_retry() {
    let server = serve(vec![token_reply(), reply("403 Forbidden", ERROR_403)]).await;
    let client = server.client(config());
    let error = client.item_offers(&asin()).await.unwrap_err();
    assert!(matches!(error, SpApiError::NotAuthorized(_)), "{error:?}");
    assert_eq!(
        error.to_source_error(),
        app::AmazonSourceError::NotAuthorized
    );
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn lwa_invalid_grant_is_not_authorized_and_redacted() {
    let echo = format!(
        r#"{{"error":"invalid_grant","error_description":"bad {REFRESH} for {CLIENT_ID} / {CLIENT_SECRET}"}}"#
    );
    let server = serve(vec![reply("400 Bad Request", &echo)]).await;
    let client = server.client(config());
    let error = client
        .search_catalog_by_keyword("taladro")
        .await
        .unwrap_err();
    assert!(matches!(error, SpApiError::NotAuthorized(_)), "{error:?}");
    let message = error.to_string();
    assert!(message.contains("invalid_grant"), "{message}");
    assert_no_secrets(&message);
    assert_eq!(server.requests().len(), 1, "no SP-API call without a token");
}

#[tokio::test]
async fn server_errors_echoing_the_token_stay_redacted() {
    let echo = format!(r#"{{"errors":[{{"code":"InternalFailure","message":"token {ACCESS}"}}]}}"#);
    let mut replies = vec![token_reply()];
    for _ in 0..4 {
        replies.push(reply("500 Internal Server Error", &echo));
    }
    let server = serve(replies).await;
    let client = server.client(config());
    let error = client.item_offers(&asin()).await.unwrap_err();
    assert!(matches!(error, SpApiError::Transient(_)), "{error:?}");
    assert_eq!(error.to_source_error(), app::AmazonSourceError::Unavailable);
    assert_no_secrets(&error.to_string());
}

#[tokio::test]
async fn each_operation_is_spaced_by_its_usage_plan() {
    let server = serve(vec![
        token_reply(),
        reply("200 OK", OFFERS),
        reply("200 OK", r#"{"items":[]}"#),
        reply("200 OK", OFFERS),
    ])
    .await;
    let mut config = config();
    config.rates.item_offers = RateLimit {
        per_request: Duration::from_millis(300),
        burst: 1,
    };
    let client = server.client(config);
    let start = tokio::time::Instant::now();
    client.item_offers(&asin()).await.unwrap();
    client.search_catalog_by_keyword("taladro").await.unwrap();
    let before_second_offers = start.elapsed();
    client.item_offers(&asin()).await.unwrap();
    assert!(before_second_offers < Duration::from_millis(300));
    assert!(
        start.elapsed() >= Duration::from_millis(300),
        "{:?}",
        start.elapsed()
    );
}
