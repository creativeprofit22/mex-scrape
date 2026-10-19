//! Manual live check: does Bright Data's "Amazon products global dataset"
//! (`gd_lwhideng15g8jg63s7`) return Amazon.com.mx records priced in MXN?
//! Ignored by default.
//!
//! ```text
//! BRIGHTDATA_API_KEY=... BRIGHTDATA_UNLOCKER_ZONE=... \
//!   cargo test -p amazon --test live_brightdata_mx -- --ignored --nocapture
//! ```
//!
//! Hard cap: one discovery trigger with `limit_per_input = 1` (at most one
//! billed record), then progress polls (free) and one download. Retries are
//! off. Prints only statuses, the record count, each record's domain,
//! currency, whether it has an ASIN/UPC/price, and redacted errors; never
//! the key, zone or record contents.

use std::time::Duration;

use amazon::{AMAZON_GLOBAL_DATASET_ID, AMAZON_MX_ORIGIN};
use brightdata::{
    BrightDataClient, BrightDataConfig, DatasetId, DatasetTrigger, DiscoverBy, Download,
    SnapshotStatus, Transport,
};
use http_guard::Secret;
use serde_json::{Value, json};

/// Progress polls (every 10 s, up to 10 minutes).
const MAX_POLLS: u32 = 60;
const POLL_EVERY: Duration = Duration::from_secs(10);
/// A common product on Amazon Mexico.
const KEYWORDS: &str = "audifonos bluetooth";

fn secret(name: &str) -> Secret {
    Secret::from_env(name)
        .unwrap_or_else(|error| panic!("{error}"))
        .unwrap_or_else(|| panic!("set {name} to run the live check"))
}

fn field<'a>(record: &'a Value, name: &str) -> &'a str {
    record.get(name).and_then(Value::as_str).unwrap_or("<none>")
}

#[tokio::test]
#[ignore = "live: needs BRIGHTDATA_API_KEY and BRIGHTDATA_UNLOCKER_ZONE; bills at most one dataset record"]
async fn live_brightdata_amazon_mx() {
    let mut config = BrightDataConfig::new(
        secret("BRIGHTDATA_API_KEY"),
        secret("BRIGHTDATA_UNLOCKER_ZONE"),
    );
    config.retry.max_retries = 0;
    let client = BrightDataClient::new(config).expect("client");

    let trigger = DatasetTrigger {
        dataset_id: DatasetId::parse(AMAZON_GLOBAL_DATASET_ID).unwrap(),
        inputs: vec![json!({
            "keywords": KEYWORDS,
            "domain": AMAZON_MX_ORIGIN,
            "pages_to_search": 1,
        })],
        discover_by: Some(DiscoverBy::parse("keywords").unwrap()),
        limit_per_input: Some(1),
    };
    let snapshot = match client.trigger(&trigger).await {
        Ok(snapshot) => snapshot,
        Err(error) => panic!("trigger failed: {error}"),
    };
    println!("trigger: ok");

    let mut ready = false;
    for poll in 1..=MAX_POLLS {
        tokio::time::sleep(POLL_EVERY).await;
        match client.progress(&snapshot).await {
            Ok(SnapshotStatus::Ready) => {
                println!("progress {poll}: ready");
                ready = true;
                break;
            }
            Ok(SnapshotStatus::Running) => println!("progress {poll}: running"),
            Ok(other) => panic!("collection ended: {other:?}"),
            Err(error) => println!("progress {poll}: error {error}"),
        }
    }
    if !ready {
        let _ = client.cancel(&snapshot).await;
        panic!("not ready after {MAX_POLLS} polls; cancelled");
    }

    let records = loop {
        match client.download(&snapshot).await {
            Ok(Download::Records(records)) => break records,
            Ok(Download::NotReady) => tokio::time::sleep(POLL_EVERY).await,
            Err(error) => panic!("download failed: {error}"),
        }
    };
    println!("records: {}", records.len());
    for (n, record) in records.iter().enumerate() {
        println!(
            "record {n}: domain={} currency={} has_asin={} has_upc={} has_final_price={} error={}",
            field(record, "domain"),
            field(record, "currency"),
            record.get("asin").is_some_and(Value::is_string),
            record
                .get("upc")
                .is_some_and(|v| v.as_str().is_some_and(|s| !s.is_empty())),
            record.get("final_price").is_some_and(Value::is_number),
            record.get("error").is_some() || record.get("error_code").is_some(),
        );
    }
}
