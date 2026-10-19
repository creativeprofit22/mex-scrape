//! Manual live smoke test against the real Bright Data API. Ignored by
//! default; costs at most one billed Web Unlocker request.
//!
//! ```text
//! BRIGHTDATA_API_KEY=... BRIGHTDATA_UNLOCKER_ZONE=... \
//!   cargo test -p brightdata --test live_smoke -- --ignored --nocapture
//! ```
//!
//! Hard cap: 3 requests (retries disabled, every call counted). Prints only
//! statuses, sizes and redacted error text; never the key, zone or body.

use std::sync::atomic::{AtomicU32, Ordering};

use brightdata::{
    BrightDataClient, BrightDataConfig, BrightDataError, CountryCode, SnapshotId, TargetUrl,
    Transport, UnlockRequest,
};
use http_guard::Secret;

const MAX_REQUESTS: u32 = 3;
/// Bright Data's own geo test page (JSON describing the exit IP).
const TEST_PAGE: &str = "https://geo.brdtest.com/mygeo.json";
/// Well-formed but unknown: checks Datasets auth without a paid trigger.
const UNKNOWN_SNAPSHOT: &str = "sd_mexscrape_smoke_unknown_0000";

struct Budget(AtomicU32);

impl Budget {
    fn spend(&self) {
        let used = self.0.fetch_add(1, Ordering::SeqCst) + 1;
        assert!(
            used <= MAX_REQUESTS,
            "live smoke test exceeded {MAX_REQUESTS} requests"
        );
    }
}

fn secret(name: &str) -> Secret {
    Secret::from_env(name)
        .unwrap_or_else(|error| panic!("{error}"))
        .unwrap_or_else(|| panic!("set {name} to run the live smoke test"))
}

#[tokio::test]
#[ignore = "live: needs BRIGHTDATA_API_KEY and BRIGHTDATA_UNLOCKER_ZONE; may bill one request"]
async fn live_smoke() {
    let mut config = BrightDataConfig::new(
        secret("BRIGHTDATA_API_KEY"),
        secret("BRIGHTDATA_UNLOCKER_ZONE"),
    );
    // One HTTP request per call, so the budget counts real requests.
    config.retry.max_retries = 0;
    let client = BrightDataClient::new(config).expect("client");
    let budget = Budget(AtomicU32::new(0));

    budget.spend();
    let page = client
        .unlock(&UnlockRequest {
            url: TargetUrl::parse(TEST_PAGE).unwrap(),
            country: Some(CountryCode::MX),
        })
        .await;
    match &page {
        Ok(page) => println!(
            "unlocker: HTTP {}, {} bytes, exit country {}",
            page.status,
            page.body.len(),
            serde_json::from_str::<serde_json::Value>(&page.body)
                .ok()
                .and_then(|geo| geo["country"].as_str().map(str::to_owned))
                .unwrap_or_else(|| "unknown".to_owned())
        ),
        Err(error) => println!("unlocker: {error:?}"),
    }
    let page = page.expect("Web Unlocker fetch succeeds");
    assert!(!page.body.is_empty());

    budget.spend();
    let id = SnapshotId::parse(UNKNOWN_SNAPSHOT).unwrap();
    let progress = client.progress(&id).await;
    println!("datasets progress (unknown snapshot): {progress:?}");
    match progress {
        Err(BrightDataError::Terminal(message)) => {
            for auth in ["HTTP 401", "HTTP 403"] {
                assert!(!message.contains(auth), "Datasets auth failed: {message}");
            }
        }
        other => panic!("expected a terminal not-found error, got {other:?}"),
    }
    println!(
        "requests used: {} of {MAX_REQUESTS}",
        budget.0.load(Ordering::SeqCst)
    );
}
