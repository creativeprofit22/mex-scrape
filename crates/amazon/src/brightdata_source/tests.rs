//! Fallback tests: a scripted [`Transport`], an in-memory [`RunStore`] and a
//! fixed clock. Records are synthetic.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use app::{AmazonDataSource, AmazonSource, AmazonSourceError, Fulfillment};
use brightdata::runs::{
    AccessOutcome, Caps, Clock, GateState, NewRun, RunId, RunRecord, RunScope, RunStatus, RunStore,
    RunUpdate, StoreError, Timing,
};
use brightdata::{
    BrightDataError, DatasetTrigger, Download, SnapshotId, SnapshotStatus, Transport,
    UnlockRequest, UnlockedPage,
};
use domain::{Asin, Gtin, PriceMxn};
use serde_json::{Value, json};

use super::*;

const NOW: i64 = 1_790_000_000_000;

struct FixedClock;

impl Clock for FixedClock {
    fn now_ms(&self) -> i64 {
        NOW
    }
}

#[derive(Default)]
struct StoreState {
    runs: BTreeMap<i64, RunRecord>,
    enabled: bool,
}

/// In-memory store: gates from `enabled`; one active run per scope.
#[derive(Default)]
struct MemoryStore {
    state: Mutex<StoreState>,
}

impl MemoryStore {
    fn enabled() -> Self {
        let store = Self::default();
        store.state.lock().unwrap().enabled = true;
        store
    }

    fn statuses(&self) -> Vec<RunStatus> {
        self.state
            .lock()
            .unwrap()
            .runs
            .values()
            .map(|r| r.status)
            .collect()
    }
}

impl RunStore for MemoryStore {
    async fn gate_state(&self, scope: &RunScope, _day: i64) -> Result<GateState, StoreError> {
        let state = self.state.lock().unwrap();
        Ok(GateState {
            kill_switch_on: !state.enabled,
            connector_enabled: state.enabled,
            runs_today: 0,
            items_today: 0,
            active_run: state
                .runs
                .values()
                .any(|run| run.scope == *scope && run.status.is_active()),
        })
    }

    async fn insert_run_capped(
        &self,
        run: &NewRun,
        _day: i64,
        _caps: &Caps,
    ) -> Result<RunId, StoreError> {
        let mut state = self.state.lock().unwrap();
        if state
            .runs
            .values()
            .any(|r| r.scope == run.scope && r.status.is_active())
        {
            return Err(StoreError::ActiveRun);
        }
        let id = RunId(i64::try_from(state.runs.len()).unwrap() + 1);
        state.runs.insert(
            id.0,
            RunRecord {
                id,
                scope: run.scope.clone(),
                mode: run.mode,
                input_json: run.input_json.clone(),
                snapshot_id: None,
                status: RunStatus::Starting,
                requested_count: run.requested_count,
                row_count: 0,
                failed_count: 0,
                error_message: String::new(),
                created_at_ms: run.now_ms,
                updated_at_ms: run.now_ms,
            },
        );
        Ok(id)
    }

    async fn update_if_active(
        &self,
        id: RunId,
        update: &RunUpdate,
        now_ms: i64,
    ) -> Result<bool, StoreError> {
        let mut state = self.state.lock().unwrap();
        let Some(run) = state.runs.get_mut(&id.0) else {
            return Ok(false);
        };
        if !run.status.is_active() {
            return Ok(false);
        }
        if let Some(status) = update.status {
            run.status = status;
        }
        if let Some(snapshot) = &update.snapshot_id {
            run.snapshot_id = Some(snapshot.as_str().to_owned());
        }
        if let Some(rows) = update.row_count {
            run.row_count = rows;
        }
        if let Some(message) = &update.error_message {
            run.error_message.clone_from(message);
        }
        run.updated_at_ms = now_ms;
        Ok(true)
    }

    async fn load_run(&self, id: RunId) -> Result<Option<RunRecord>, StoreError> {
        Ok(self.state.lock().unwrap().runs.get(&id.0).cloned())
    }

    async fn active_runs(&self) -> Result<Vec<RunRecord>, StoreError> {
        Ok(self
            .state
            .lock()
            .unwrap()
            .runs
            .values()
            .filter(|r| r.status.is_active())
            .cloned()
            .collect())
    }

    async fn kill_switch_on(&self) -> Result<bool, StoreError> {
        Ok(!self.state.lock().unwrap().enabled)
    }

    async fn record_access(
        &self,
        _run: RunId,
        _host: &str,
        _outcome: AccessOutcome,
    ) -> Result<(), StoreError> {
        Ok(())
    }
}

/// Scripted transport: every trigger succeeds; progress answers come from
/// `progress` (then `Ready`); downloads return `records`.
#[derive(Default)]
struct FakeTransport {
    triggers: Mutex<Vec<DatasetTrigger>>,
    progress: Mutex<VecDeque<SnapshotStatus>>,
    records: Mutex<Vec<Value>>,
    cancels: Mutex<u32>,
}

impl FakeTransport {
    fn with_records(records: Vec<Value>) -> Self {
        let fake = Self::default();
        *fake.records.lock().unwrap() = records;
        fake
    }

    fn triggers(&self) -> Vec<DatasetTrigger> {
        self.triggers.lock().unwrap().clone()
    }
}

impl Transport for FakeTransport {
    async fn unlock(&self, _request: &UnlockRequest) -> Result<UnlockedPage, BrightDataError> {
        Err(BrightDataError::Terminal("unlock not expected".to_owned()))
    }

    async fn trigger(&self, trigger: &DatasetTrigger) -> Result<SnapshotId, BrightDataError> {
        self.triggers.lock().unwrap().push(trigger.clone());
        SnapshotId::parse("sd_synthetic_amazon_01")
    }

    async fn progress(&self, _id: &SnapshotId) -> Result<SnapshotStatus, BrightDataError> {
        Ok(self
            .progress
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(SnapshotStatus::Ready))
    }

    async fn download(&self, _id: &SnapshotId) -> Result<Download, BrightDataError> {
        Ok(Download::Records(self.records.lock().unwrap().clone()))
    }

    async fn cancel(&self, _id: &SnapshotId) -> Result<(), BrightDataError> {
        *self.cancels.lock().unwrap() += 1;
        Ok(())
    }
}

type Source = BrightDataAmazonSource<FakeTransport, MemoryStore, FixedClock>;

fn source(transport: &Arc<FakeTransport>, store: &Arc<MemoryStore>) -> Source {
    BrightDataAmazonSource::new(Arc::clone(transport), Arc::clone(store), FixedClock)
        .unwrap()
        .with_timing(Timing {
            poll_interval: Duration::from_millis(10),
            poll_budget: Duration::from_millis(50),
        })
}

fn record(asin: &str, domain: &str, currency: &str, upc: &str) -> Value {
    json!({
        "asin": asin,
        "title": format!("Producto  sintético {asin}"),
        "brand": "MarcaPrueba",
        "domain": domain,
        "url": format!("{domain}dp/{asin}"),
        "currency": currency,
        "final_price": 1299.5,
        "number_of_sellers": 3,
        "upc": upc,
        "image_url": "https://m.media-amazon.com/images/I/synthetic-main.jpg",
        "images": [
            "https://m.media-amazon.com/images/I/synthetic-main.jpg",
            "https://m.media-amazon.com/images/I/synthetic-pt01.jpg"
        ],
        "bs_rank": 87,
        "bs_category": "Taladros",
        "root_bs_rank": 1520,
        "root_bs_category": "Herramientas y Mejoras del Hogar"
    })
}

const MX: &str = "https://www.amazon.com.mx/";
const US: &str = "https://www.amazon.com/";

#[tokio::test(start_paused = true)]
async fn barcode_lookup_searches_mx_and_keeps_exact_upc_matches_only() {
    let transport = Arc::new(FakeTransport::with_records(vec![
        record("B0SYNTH001", MX, "MXN", "012345678905"),
        record("B0SYNTH002", MX, "MXN", "999999999993"),
        record("B0SYNTH003", US, "USD", "012345678905"),
        record("B0SYNTH004", MX, "MXN", ""),
    ]));
    let store = Arc::new(MemoryStore::enabled());
    let gtin = Gtin::new("0012345678905").unwrap();
    let result = source(&transport, &store)
        .catalog_by_gtin(&gtin)
        .await
        .unwrap();
    assert_eq!(result.source, AmazonDataSource::BrightData);
    assert_eq!(result.fetched_at.0, NOW);
    let asins: Vec<&str> = result.items.iter().map(|i| i.asin.as_str()).collect();
    assert_eq!(asins, ["B0SYNTH001"]);
    let item = &result.items[0];
    assert_eq!(item.title, "Producto sintético B0SYNTH001");
    assert_eq!(item.images.len(), 2);
    assert_eq!(item.sales_ranks[0].rank, 87);

    let triggers = transport.triggers();
    assert_eq!(triggers.len(), 1);
    let trigger = &triggers[0];
    assert_eq!(trigger.dataset_id.as_str(), "gd_lwhideng15g8jg63s7");
    assert_eq!(trigger.discover_by.as_ref().unwrap().as_str(), "keywords");
    assert_eq!(trigger.limit_per_input, Some(5));
    assert_eq!(
        trigger.inputs,
        [
            json!({"keywords": "0012345678905", "domain": "https://www.amazon.com.mx", "pages_to_search": 1})
        ]
    );
    assert_eq!(store.statuses(), [RunStatus::Imported]);
}

#[tokio::test(start_paused = true)]
async fn keyword_lookup_drops_non_mx_and_non_mxn_records() {
    let transport = Arc::new(FakeTransport::with_records(vec![
        record("B0SYNTH001", MX, "MXN", ""),
        record("B0SYNTH002", MX, "EUR", ""),
        record("B0SYNTH003", US, "USD", ""),
        record("B0SYNTH001", MX, "MXN", ""),
        record("not-an-asin", MX, "MXN", ""),
        json!({"asin": "B0SYNTH005", "title": "sin dominio"}),
    ]));
    let store = Arc::new(MemoryStore::enabled());
    let result = source(&transport, &store)
        .catalog_by_keyword(" taladro  20v ")
        .await
        .unwrap();
    let asins: Vec<&str> = result.items.iter().map(|i| i.asin.as_str()).collect();
    assert_eq!(asins, ["B0SYNTH001"]);
    assert_eq!(transport.triggers()[0].inputs[0]["keywords"], "taladro 20v");
}

#[tokio::test(start_paused = true)]
async fn offers_collect_the_mx_product_page() {
    let transport = Arc::new(FakeTransport::with_records(vec![record(
        "B0SYNTH001",
        MX,
        "MXN",
        "",
    )]));
    let store = Arc::new(MemoryStore::enabled());
    let asin = Asin::new("B0SYNTH001").unwrap();
    let summary = source(&transport, &store).offers(&asin).await.unwrap();
    assert_eq!(
        summary.buy_box_mxn,
        Some(PriceMxn::from_centavos(129_950).unwrap())
    );
    assert_eq!(summary.lowest_new_mxn, None);
    assert_eq!(summary.offer_count, 3);
    let trigger = &transport.triggers()[0];
    assert!(trigger.discover_by.is_none());
    assert_eq!(trigger.limit_per_input, None);
    assert_eq!(
        trigger.inputs,
        [json!({"url": "https://www.amazon.com.mx/dp/B0SYNTH001"})]
    );
}

/// Regression: the live check returned amazon.com.mx records labelled
/// `MXD`; they are pesos and must not be dropped.
#[tokio::test(start_paused = true)]
async fn mx_records_labelled_mxd_count_as_pesos() {
    let asin = Asin::new("B0SYNTH001").unwrap();
    let transport = Arc::new(FakeTransport::with_records(vec![record(
        "B0SYNTH001",
        MX,
        "MXD",
        "",
    )]));
    let store = Arc::new(MemoryStore::enabled());
    let summary = source(&transport, &store).offers(&asin).await.unwrap();
    assert_eq!(
        summary.buy_box_mxn,
        Some(PriceMxn::from_centavos(129_950).unwrap())
    );
    let transport = Arc::new(FakeTransport::with_records(vec![
        record("B0SYNTH001", MX, "MXD", ""),
        record("B0SYNTH002", US, "MXD", ""),
    ]));
    let store = Arc::new(MemoryStore::enabled());
    let result = source(&transport, &store)
        .catalog_by_keyword("taladro")
        .await
        .unwrap();
    let asins: Vec<&str> = result.items.iter().map(|i| i.asin.as_str()).collect();
    assert_eq!(asins, ["B0SYNTH001"]);
}

#[tokio::test(start_paused = true)]
async fn offers_from_another_store_or_currency_fail_closed() {
    let asin = Asin::new("B0SYNTH001").unwrap();
    for bad in [
        record("B0SYNTH001", US, "USD", ""),
        record("B0SYNTH001", MX, "USD", ""),
        json!({"asin": "B0SYNTH001", "domain": MX, "final_price": 10}),
    ] {
        let transport = Arc::new(FakeTransport::with_records(vec![bad]));
        let store = Arc::new(MemoryStore::enabled());
        let error = source(&transport, &store).offers(&asin).await.unwrap_err();
        assert_eq!(error, AmazonSourceError::ParseFailed);
    }
    // No record at all: Amazon Mexico has no such product page.
    let transport = Arc::new(FakeTransport::default());
    let store = Arc::new(MemoryStore::enabled());
    let summary = source(&transport, &store).offers(&asin).await.unwrap();
    assert_eq!(summary.offer_count, 0);
}

#[tokio::test(start_paused = true)]
async fn fees_are_unsupported_without_a_run() {
    let transport = Arc::new(FakeTransport::default());
    let store = Arc::new(MemoryStore::enabled());
    let asin = Asin::new("B0SYNTH001").unwrap();
    let price = PriceMxn::from_centavos(100).unwrap();
    let error = source(&transport, &store)
        .fees(&asin, price, Fulfillment::Fba)
        .await
        .unwrap_err();
    assert_eq!(error, AmazonSourceError::Unsupported);
    assert!(transport.triggers().is_empty());
}

#[tokio::test(start_paused = true)]
async fn closed_gates_are_unavailable_and_trigger_nothing() {
    let transport = Arc::new(FakeTransport::default());
    let store = Arc::new(MemoryStore::default());
    let error = source(&transport, &store)
        .catalog_by_keyword("taladro")
        .await
        .unwrap_err();
    assert_eq!(error, AmazonSourceError::Unavailable);
    assert!(transport.triggers().is_empty());
    assert!(store.statuses().is_empty());
}

#[tokio::test(start_paused = true)]
async fn blank_keywords_are_invalid_input() {
    let transport = Arc::new(FakeTransport::default());
    let store = Arc::new(MemoryStore::enabled());
    let error = source(&transport, &store)
        .catalog_by_keyword("   ")
        .await
        .unwrap_err();
    assert_eq!(error, AmazonSourceError::InvalidInput);
    assert!(transport.triggers().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_run_still_collecting_is_cancelled_and_frees_the_scope() {
    let transport = Arc::new(FakeTransport::with_records(vec![record(
        "B0SYNTH001",
        MX,
        "MXN",
        "",
    )]));
    transport
        .progress
        .lock()
        .unwrap()
        .extend([SnapshotStatus::Running; 20]);
    let store = Arc::new(MemoryStore::enabled());
    let source = source(&transport, &store);
    let error = source.catalog_by_keyword("taladro").await.unwrap_err();
    assert_eq!(error, AmazonSourceError::Unavailable);
    assert_eq!(store.statuses(), [RunStatus::Cancelled]);
    assert_eq!(*transport.cancels.lock().unwrap(), 1);
    // The scope is free again: the next lookup runs.
    transport.progress.lock().unwrap().clear();
    assert_eq!(
        source
            .catalog_by_keyword("taladro")
            .await
            .unwrap()
            .items
            .len(),
        1
    );
}

#[test]
fn upc_matching_ignores_leading_zeros_and_lists() {
    let gtin = Gtin::new("012345678905").unwrap();
    for upc in [
        "012345678905",
        "0012345678905",
        "12345678905",
        "111, 012345678905",
    ] {
        assert!(upc_matches(&json!({ "upc": upc }), &gtin), "{upc}");
    }
    for upc in ["", "0123456789050", "x"] {
        assert!(!upc_matches(&json!({ "upc": upc }), &gtin), "{upc}");
    }
    assert!(!upc_matches(&json!({}), &gtin));
}
