//! Run driver tests with a fake transport, an in-memory store and a fixed
//! clock. Each test builds its own fixtures.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use super::{
    AccessOutcome, COLLECTION_FAILED, Caps, Clock, GateError, GateState, KILL_SWITCH_STOP,
    MAX_RUN_ERROR_CHARS, NO_PAGE_FETCHED, NewRun, POLL_BUDGET_EXHAUSTED, PageAccess, RunError,
    RunId, RunMode, RunRecord, RunScope, RunSink, RunStatus, RunStore, RunUpdate, Runner,
    StoreError, Timing, bound_error, check_gates, day_start_ms,
};
use crate::client::{DatasetTrigger, Download, SnapshotStatus, UnlockRequest, UnlockedPage};
use crate::error::BrightDataError;
use crate::ids::{DatasetId, DiscoverBy, SnapshotId, TargetUrl};
use crate::refusal::RefusalReason;
use crate::transport::Transport;

const NOW: i64 = 1_790_000_000_000;
const SNAP: &str = "sd_run0000000001";

pub(crate) fn scope(job: &str) -> RunScope {
    RunScope::new("tenant-1", job).unwrap()
}

pub(crate) struct FixedClock(pub(crate) i64);

impl Clock for FixedClock {
    fn now_ms(&self) -> i64 {
        self.0
    }
}

#[derive(Default)]
struct State {
    runs: BTreeMap<i64, RunRecord>,
    next_id: i64,
    kill_switch: bool,
    enabled: bool,
    access: Vec<(i64, String, AccessOutcome)>,
}

impl State {
    fn gate_state(&self, scope: &RunScope, day_start_ms: i64) -> GateState {
        let tenant_today = self
            .runs
            .values()
            .filter(|run| run.scope.tenant_id() == scope.tenant_id())
            .filter(|run| run.created_at_ms >= day_start_ms);
        let (runs_today, items_today) = tenant_today.fold((0, 0), |(runs, items), run| {
            (runs + 1, items + run.requested_count)
        });
        GateState {
            kill_switch_on: self.kill_switch,
            connector_enabled: self.enabled,
            runs_today,
            items_today,
            active_run: self
                .runs
                .values()
                .any(|run| run.scope == *scope && run.status.is_active()),
        }
    }
}

/// In-memory [`RunStore`] with the same atomic rules as Postgres.
#[derive(Default)]
pub(crate) struct MemoryStore {
    state: Mutex<State>,
}

impl MemoryStore {
    /// Kill switch off, connector enabled.
    pub(crate) fn enabled() -> Self {
        let store = Self::default();
        store.state.lock().unwrap().enabled = true;
        store
    }

    pub(crate) fn set_kill_switch(&self, on: bool) {
        self.state.lock().unwrap().kill_switch = on;
    }

    pub(crate) fn seed(
        &self,
        scope: &RunScope,
        mode: RunMode,
        snapshot: Option<&str>,
        status: RunStatus,
        at_ms: i64,
    ) -> RunId {
        let mut state = self.state.lock().unwrap();
        state.next_id += 1;
        let id = RunId(state.next_id);
        state.runs.insert(
            id.0,
            RunRecord {
                id,
                scope: scope.clone(),
                mode,
                input_json: "{}".to_owned(),
                snapshot_id: snapshot.map(str::to_owned),
                status,
                requested_count: 1,
                row_count: 0,
                failed_count: 0,
                error_message: String::new(),
                created_at_ms: at_ms,
                updated_at_ms: at_ms,
            },
        );
        id
    }

    pub(crate) fn run(&self, id: RunId) -> RunRecord {
        self.state.lock().unwrap().runs[&id.0].clone()
    }

    pub(crate) fn force_status(&self, id: RunId, status: RunStatus) {
        self.state
            .lock()
            .unwrap()
            .runs
            .get_mut(&id.0)
            .unwrap()
            .status = status;
    }

    fn count(&self) -> usize {
        self.state.lock().unwrap().runs.len()
    }
}

impl RunStore for MemoryStore {
    async fn gate_state(
        &self,
        scope: &RunScope,
        day_start_ms: i64,
    ) -> Result<GateState, StoreError> {
        Ok(self.state.lock().unwrap().gate_state(scope, day_start_ms))
    }

    async fn insert_run_capped(
        &self,
        run: &NewRun,
        day_start_ms: i64,
        caps: &Caps,
    ) -> Result<RunId, StoreError> {
        let mut state = self.state.lock().unwrap();
        let gates = state.gate_state(&run.scope, day_start_ms);
        if gates.active_run {
            return Err(StoreError::ActiveRun);
        }
        if gates.runs_today >= caps.max_runs_per_tenant_per_day {
            return Err(StoreError::CapExceeded(GateError::RunsPerDayCap));
        }
        if run.requested_count > caps.max_items_per_run {
            return Err(StoreError::CapExceeded(GateError::ItemsPerRunCap));
        }
        if gates.items_today + run.requested_count > caps.max_items_per_tenant_per_day {
            return Err(StoreError::CapExceeded(GateError::ItemsPerDayCap));
        }
        state.next_id += 1;
        let id = RunId(state.next_id);
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
        if let Some(failed) = update.failed_count {
            run.failed_count = failed;
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
            .filter(|run| run.status.is_active())
            .cloned()
            .collect())
    }

    async fn kill_switch_on(&self) -> Result<bool, StoreError> {
        Ok(self.state.lock().unwrap().kill_switch)
    }

    async fn record_access(
        &self,
        run: RunId,
        host: &str,
        outcome: AccessOutcome,
    ) -> Result<(), StoreError> {
        self.state
            .lock()
            .unwrap()
            .access
            .push((run.0, host.to_owned(), outcome));
        Ok(())
    }
}

type Hook = Box<dyn FnOnce() + Send>;

/// Scripted [`Transport`]: each call pops the next queued answer.
#[derive(Default)]
struct FakeTransport {
    unlock: Mutex<VecDeque<Result<UnlockedPage, BrightDataError>>>,
    trigger: Mutex<VecDeque<Result<SnapshotId, BrightDataError>>>,
    progress: Mutex<VecDeque<Result<SnapshotStatus, BrightDataError>>>,
    download: Mutex<VecDeque<Result<Download, BrightDataError>>>,
    cancelled: Mutex<Vec<String>>,
    calls: Mutex<Vec<&'static str>>,
    on_progress: Mutex<Option<Hook>>,
}

fn pop<T>(queue: &Mutex<VecDeque<T>>, name: &str) -> T {
    queue
        .lock()
        .unwrap()
        .pop_front()
        .unwrap_or_else(|| panic!("unexpected {name} call"))
}

impl FakeTransport {
    fn calls(&self) -> Vec<&'static str> {
        self.calls.lock().unwrap().clone()
    }

    fn cancelled(&self) -> Vec<String> {
        self.cancelled.lock().unwrap().clone()
    }

    fn record(&self, call: &'static str) {
        self.calls.lock().unwrap().push(call);
    }
}

impl Transport for FakeTransport {
    async fn unlock(&self, _request: &UnlockRequest) -> Result<UnlockedPage, BrightDataError> {
        self.record("unlock");
        pop(&self.unlock, "unlock")
    }

    async fn browse(&self, _request: &UnlockRequest) -> Result<UnlockedPage, BrightDataError> {
        self.record("browse");
        pop(&self.unlock, "browse")
    }

    async fn trigger(&self, _trigger: &DatasetTrigger) -> Result<SnapshotId, BrightDataError> {
        self.record("trigger");
        pop(&self.trigger, "trigger")
    }

    async fn progress(&self, _id: &SnapshotId) -> Result<SnapshotStatus, BrightDataError> {
        self.record("progress");
        let hook = self.on_progress.lock().unwrap().take();
        if let Some(hook) = hook {
            hook();
        }
        pop(&self.progress, "progress")
    }

    async fn download(&self, _id: &SnapshotId) -> Result<Download, BrightDataError> {
        self.record("download");
        pop(&self.download, "download")
    }

    async fn cancel(&self, id: &SnapshotId) -> Result<(), BrightDataError> {
        self.record("cancel");
        self.cancelled.lock().unwrap().push(id.as_str().to_owned());
        Ok(())
    }
}

#[derive(Default)]
struct MemorySink {
    pages: Mutex<Vec<String>>,
    records: Mutex<Vec<Value>>,
    reject_pages_containing: Option<&'static str>,
    reject_records: bool,
}

impl RunSink for MemorySink {
    async fn accept_page(&self, url: &TargetUrl, _page: UnlockedPage) -> Result<(), String> {
        if let Some(marker) = self.reject_pages_containing
            && url.as_str().contains(marker)
        {
            return Err("page could not be parsed".to_owned());
        }
        self.pages.lock().unwrap().push(url.as_str().to_owned());
        Ok(())
    }

    async fn accept_records(&self, records: Vec<Value>) -> Result<u64, String> {
        if self.reject_records {
            return Err("records could not be stored".to_owned());
        }
        let count = records.len() as u64;
        self.records.lock().unwrap().extend(records);
        Ok(count)
    }
}

fn runner<'a>(
    transport: &'a FakeTransport,
    store: &'a MemoryStore,
    clock: &'a FixedClock,
) -> Runner<'a, FakeTransport, MemoryStore, FixedClock> {
    Runner {
        transport: Some(transport),
        store,
        clock,
        timing: Timing {
            poll_interval: Duration::ZERO,
            poll_budget: Duration::from_secs(60),
        },
        caps: Caps::default(),
        robots: None,
    }
}

fn queue<T>(items: impl IntoIterator<Item = T>) -> Mutex<VecDeque<T>> {
    Mutex::new(items.into_iter().collect())
}

fn snap() -> SnapshotId {
    SnapshotId::parse(SNAP).unwrap()
}

fn dataset_transport(
    progress: impl IntoIterator<Item = Result<SnapshotStatus, BrightDataError>>,
    download: impl IntoIterator<Item = Result<Download, BrightDataError>>,
) -> FakeTransport {
    FakeTransport {
        trigger: queue([Ok(snap())]),
        progress: queue(progress),
        download: queue(download),
        ..FakeTransport::default()
    }
}

fn trigger() -> DatasetTrigger {
    DatasetTrigger {
        dataset_id: DatasetId::parse("gd_l7q7dkf244hwjntr0").unwrap(),
        inputs: vec![json!({ "url": "https://www.amazon.com.mx/dp/B0001" })],
        discover_by: None,
        limit_per_input: None,
    }
}

fn records(count: usize) -> Download {
    Download::Records((0..count).map(|n| json!({ "n": n })).collect())
}

fn page() -> UnlockedPage {
    UnlockedPage {
        status: 200,
        body: "<html></html>".to_owned(),
    }
}

fn urls(paths: &[&str]) -> Vec<TargetUrl> {
    paths
        .iter()
        .map(|path| TargetUrl::parse(&format!("https://tienda.example.mx/{path}")).unwrap())
        .collect()
}

#[test]
fn gates_fail_closed_in_order() {
    let caps = Caps::default();
    let mut state = GateState {
        kill_switch_on: true,
        connector_enabled: false,
        runs_today: caps.max_runs_per_tenant_per_day,
        items_today: caps.max_items_per_tenant_per_day,
        active_run: true,
    };
    let mut key_present = false;
    let mut requested = caps.max_items_per_run + 1;
    let check =
        |state: &GateState, key: bool, requested: u32| check_gates(state, key, &caps, requested);
    assert_eq!(
        check(&state, key_present, requested),
        Err(GateError::KillSwitch)
    );
    state.kill_switch_on = false;
    assert_eq!(
        check(&state, key_present, requested),
        Err(GateError::ConnectorDisabled)
    );
    state.connector_enabled = true;
    assert_eq!(
        check(&state, key_present, requested),
        Err(GateError::MissingKey)
    );
    key_present = true;
    assert_eq!(
        check(&state, key_present, requested),
        Err(GateError::RunsPerDayCap)
    );
    state.runs_today = 0;
    assert_eq!(
        check(&state, key_present, requested),
        Err(GateError::ItemsPerRunCap)
    );
    requested = 1;
    assert_eq!(
        check(&state, key_present, requested),
        Err(GateError::ItemsPerDayCap)
    );
    state.items_today = caps.max_items_per_tenant_per_day - 1;
    assert_eq!(
        check(&state, key_present, requested),
        Err(GateError::ActiveRun)
    );
    state.active_run = false;
    assert_eq!(check(&state, key_present, requested), Ok(()));
    // A missing settings row is reported as the default state: closed.
    assert_eq!(
        check(&GateState::default(), true, 1),
        Err(GateError::ConnectorDisabled)
    );
}

#[test]
fn helpers_validate_and_bound() {
    assert_eq!(day_start_ms(NOW), NOW - NOW % 86_400_000);
    assert_eq!(day_start_ms(86_400_000), 86_400_000);
    assert_eq!(day_start_ms(-1), -86_400_000);
    let long = "e".repeat(MAX_RUN_ERROR_CHARS + 10);
    let bounded = bound_error(&long);
    assert_eq!(bounded.chars().count(), MAX_RUN_ERROR_CHARS);
    assert!(bounded.ends_with('…'));
    assert_eq!(bound_error("short"), "short");
    assert!(RunScope::new("tenant-1", "liverpool:prices.v1").is_ok());
    for (tenant, job) in [
        ("", "j"),
        ("t", ""),
        ("t t", "j"),
        ("t", "a/b"),
        ("t", &"j".repeat(101)),
    ] {
        assert!(RunScope::new(tenant, job).is_err(), "{tenant:?} {job:?}");
    }
    for status in RunStatus::ALL {
        assert_eq!(RunStatus::parse(status.as_str()), Some(status));
    }
    assert_eq!(RunMode::parse("dataset"), Some(RunMode::Dataset));
    assert_eq!(RunMode::parse("other"), None);
}

#[tokio::test]
async fn dataset_run_trigger_running_ready_download_imported() {
    let transport = dataset_transport(
        [
            Ok(SnapshotStatus::Running),
            Ok(SnapshotStatus::Ready),
            Ok(SnapshotStatus::Ready),
        ],
        [Ok(Download::NotReady), Ok(records(3))],
    );
    let store = MemoryStore::enabled();
    let clock = FixedClock(NOW);
    let sink = MemorySink::default();
    let outcome = runner(&transport, &store, &clock)
        .start_dataset_run(&scope("amazon"), &trigger(), &sink)
        .await
        .unwrap();
    assert_eq!(outcome.status, RunStatus::Imported);
    assert_eq!(outcome.row_count, 3);
    assert_eq!(outcome.message, "");
    assert_eq!(sink.records.lock().unwrap().len(), 3);
    assert_eq!(
        transport.calls(),
        [
            "trigger", "progress", "progress", "download", "progress", "download"
        ]
    );
    let run = store.run(outcome.run_id);
    assert_eq!(run.snapshot_id.as_deref(), Some(SNAP));
    assert_eq!(run.mode, RunMode::Dataset);
    assert_eq!(run.requested_count, 1);
    let input: Value = serde_json::from_str(&run.input_json).unwrap();
    assert_eq!(input["dataset_id"], "gd_l7q7dkf244hwjntr0");
}

#[tokio::test]
async fn exhausted_budget_leaves_the_run_resumable() {
    let transport = dataset_transport([Ok(SnapshotStatus::Running)], []);
    let store = MemoryStore::enabled();
    let clock = FixedClock(NOW);
    let sink = MemorySink::default();
    let mut runner = runner(&transport, &store, &clock);
    runner.timing.poll_budget = Duration::ZERO;
    let outcome = runner
        .start_dataset_run(&scope("amazon"), &trigger(), &sink)
        .await
        .unwrap();
    assert_eq!(outcome.status, RunStatus::Running);
    assert_eq!(outcome.message, POLL_BUDGET_EXHAUSTED);

    transport
        .progress
        .lock()
        .unwrap()
        .push_back(Ok(SnapshotStatus::Ready));
    transport.download.lock().unwrap().push_back(Ok(records(2)));
    let resumed = runner.resume_run(outcome.run_id, &sink).await.unwrap();
    assert_eq!(resumed.status, RunStatus::Imported);
    assert_eq!(resumed.row_count, 2);
}

#[tokio::test]
async fn transient_errors_keep_the_run_active_and_terminal_errors_fail_it() {
    let store = MemoryStore::enabled();
    let clock = FixedClock(NOW);
    let sink = MemorySink::default();

    let transport = dataset_transport([Err(BrightDataError::Transient("HTTP 503".to_owned()))], []);
    let outcome = runner(&transport, &store, &clock)
        .start_dataset_run(&scope("one"), &trigger(), &sink)
        .await
        .unwrap();
    assert_eq!(outcome.status, RunStatus::Running);
    assert_eq!(outcome.message, "HTTP 503");

    let transport = dataset_transport([Err(BrightDataError::Terminal("HTTP 404".to_owned()))], []);
    let outcome = runner(&transport, &store, &clock)
        .start_dataset_run(&scope("two"), &trigger(), &sink)
        .await
        .unwrap();
    assert_eq!(outcome.status, RunStatus::Failed);
    assert_eq!(outcome.message, "HTTP 404");

    let transport = dataset_transport([Ok(SnapshotStatus::Failed)], []);
    let outcome = runner(&transport, &store, &clock)
        .start_dataset_run(&scope("three"), &trigger(), &sink)
        .await
        .unwrap();
    assert_eq!(outcome.status, RunStatus::Failed);
    assert_eq!(outcome.message, COLLECTION_FAILED);

    let transport = FakeTransport {
        trigger: queue([Err(BrightDataError::Terminal("HTTP 400".to_owned()))]),
        ..FakeTransport::default()
    };
    let outcome = runner(&transport, &store, &clock)
        .start_dataset_run(&scope("four"), &trigger(), &sink)
        .await
        .unwrap();
    assert_eq!(outcome.status, RunStatus::Failed);
    assert_eq!(store.run(outcome.run_id).snapshot_id, None);
}

#[tokio::test]
async fn sink_failure_leaves_the_run_ready() {
    let transport = dataset_transport([Ok(SnapshotStatus::Ready)], [Ok(records(1))]);
    let store = MemoryStore::enabled();
    let clock = FixedClock(NOW);
    let sink = MemorySink {
        reject_records: true,
        ..MemorySink::default()
    };
    let outcome = runner(&transport, &store, &clock)
        .start_dataset_run(&scope("amazon"), &trigger(), &sink)
        .await
        .unwrap();
    assert_eq!(outcome.status, RunStatus::Ready);
    assert_eq!(outcome.message, "records could not be stored");
}

#[tokio::test]
async fn kill_switch_mid_run_cancels_the_snapshot() {
    let store = Arc::new(MemoryStore::enabled());
    let transport = dataset_transport([Ok(SnapshotStatus::Running)], []);
    let flip = Arc::clone(&store);
    *transport.on_progress.lock().unwrap() = Some(Box::new(move || flip.set_kill_switch(true)));
    let clock = FixedClock(NOW);
    let outcome = runner(&transport, &store, &clock)
        .start_dataset_run(&scope("amazon"), &trigger(), &MemorySink::default())
        .await
        .unwrap();
    assert_eq!(outcome.status, RunStatus::Cancelled);
    assert_eq!(outcome.message, KILL_SWITCH_STOP);
    assert_eq!(transport.cancelled(), [SNAP]);
    assert_eq!(transport.calls(), ["trigger", "progress", "cancel"]);
}

#[tokio::test]
async fn cancel_from_elsewhere_stops_the_loop() {
    let store = Arc::new(MemoryStore::enabled());
    let transport = dataset_transport([Ok(SnapshotStatus::Running)], []);
    let other = Arc::clone(&store);
    // The run about to be inserted gets id 1.
    *transport.on_progress.lock().unwrap() = Some(Box::new(move || {
        other.force_status(RunId(1), RunStatus::Cancelled)
    }));
    let clock = FixedClock(NOW);
    let outcome = runner(&transport, &store, &clock)
        .start_dataset_run(&scope("amazon"), &trigger(), &MemorySink::default())
        .await
        .unwrap();
    assert_eq!(outcome.run_id, RunId(1));
    assert_eq!(outcome.status, RunStatus::Cancelled);
    assert_eq!(transport.cancelled(), [SNAP]);
}

#[tokio::test]
async fn cancel_run_marks_cancelled_and_cancels_the_snapshot_once() {
    let store = MemoryStore::enabled();
    store.set_kill_switch(true);
    let id = store.seed(
        &scope("amazon"),
        RunMode::Dataset,
        Some(SNAP),
        RunStatus::Running,
        NOW,
    );
    let transport = FakeTransport::default();
    let clock = FixedClock(NOW);
    let runner = runner(&transport, &store, &clock);
    assert!(runner.cancel_run(id).await.unwrap());
    assert_eq!(store.run(id).status, RunStatus::Cancelled);
    assert!(!runner.cancel_run(id).await.unwrap());
    assert_eq!(transport.cancelled(), [SNAP]);
    assert_eq!(runner.cancel_run(RunId(99)).await, Err(RunError::NotFound));
}

#[tokio::test]
async fn unlocker_run_counts_partial_failures() {
    let transport = FakeTransport {
        unlock: queue([
            Ok(page()),
            Err(BrightDataError::Terminal("HTTP 404".to_owned())),
            Ok(page()),
        ]),
        ..FakeTransport::default()
    };
    let store = MemoryStore::enabled();
    let clock = FixedClock(NOW);
    let sink = MemorySink {
        reject_pages_containing: Some("bad"),
        ..MemorySink::default()
    };
    let outcome = runner(&transport, &store, &clock)
        .start_unlocker_run(
            &scope("liverpool"),
            &urls(&["a", "b", "bad"]),
            None,
            PageAccess::Unlocker,
            &sink,
        )
        .await
        .unwrap();
    assert_eq!(outcome.status, RunStatus::Imported);
    assert_eq!((outcome.row_count, outcome.failed_count), (1, 2));
    assert_eq!(outcome.message, "HTTP 404");
    assert_eq!(*sink.pages.lock().unwrap(), ["https://tienda.example.mx/a"]);
    assert_eq!(store.run(outcome.run_id).requested_count, 3);
}

#[tokio::test]
async fn unlocker_run_with_no_page_fails() {
    let transport = FakeTransport {
        unlock: queue([Ok(page())]),
        ..FakeTransport::default()
    };
    let store = MemoryStore::enabled();
    let clock = FixedClock(NOW);
    let sink = MemorySink {
        reject_pages_containing: Some("x"),
        ..MemorySink::default()
    };
    let outcome = runner(&transport, &store, &clock)
        .start_unlocker_run(
            &scope("liverpool"),
            &urls(&["x"]),
            None,
            PageAccess::Unlocker,
            &sink,
        )
        .await
        .unwrap();
    assert_eq!(outcome.status, RunStatus::Failed);
    assert_eq!(outcome.failed_count, 1);
    assert_eq!(outcome.message, "page could not be parsed");
    assert_ne!(outcome.message, NO_PAGE_FETCHED);
}

#[tokio::test]
async fn unlocker_run_stops_on_the_kill_switch() {
    let store = Arc::new(MemoryStore::enabled());
    let transport = FakeTransport {
        unlock: queue([Ok(page())]),
        ..FakeTransport::default()
    };
    struct FlipSink(Arc<MemoryStore>);
    impl RunSink for FlipSink {
        async fn accept_page(&self, _url: &TargetUrl, _page: UnlockedPage) -> Result<(), String> {
            self.0.set_kill_switch(true);
            Ok(())
        }
        async fn accept_records(&self, _records: Vec<Value>) -> Result<u64, String> {
            Ok(0)
        }
    }
    let clock = FixedClock(NOW);
    let outcome = runner(&transport, &store, &clock)
        .start_unlocker_run(
            &scope("liverpool"),
            &urls(&["a", "b"]),
            None,
            PageAccess::Unlocker,
            &FlipSink(Arc::clone(&store)),
        )
        .await
        .unwrap();
    assert_eq!(outcome.status, RunStatus::Cancelled);
    assert_eq!(outcome.row_count, 1);
    assert_eq!(transport.calls(), ["unlock"]);
}

#[tokio::test]
async fn gates_block_runs_before_any_call_or_row() {
    let clock = FixedClock(NOW);
    let sink = MemorySink::default();
    let transport = FakeTransport::default();
    let one = urls(&["a"]);

    let store = MemoryStore::enabled();
    store.set_kill_switch(true);
    let result = runner(&transport, &store, &clock)
        .start_unlocker_run(&scope("j"), &one, None, PageAccess::Unlocker, &sink)
        .await;
    assert_eq!(result, Err(RunError::Gate(GateError::KillSwitch)));

    let disabled = MemoryStore::default();
    let result = runner(&transport, &disabled, &clock)
        .start_dataset_run(&scope("j"), &trigger(), &sink)
        .await;
    assert_eq!(result, Err(RunError::Gate(GateError::ConnectorDisabled)));

    let store = MemoryStore::enabled();
    let no_key: Runner<'_, FakeTransport, _, _> = Runner {
        transport: None,
        ..runner(&transport, &store, &clock)
    };
    let result = no_key
        .start_unlocker_run(&scope("j"), &one, None, PageAccess::Unlocker, &sink)
        .await;
    assert_eq!(result, Err(RunError::Gate(GateError::MissingKey)));

    let many: Vec<String> = (0..51).map(|n| n.to_string()).collect();
    let many: Vec<&str> = many.iter().map(String::as_str).collect();
    let result = runner(&transport, &store, &clock)
        .start_unlocker_run(&scope("j"), &urls(&many), None, PageAccess::Unlocker, &sink)
        .await;
    assert_eq!(result, Err(RunError::Gate(GateError::ItemsPerRunCap)));

    store.seed(&scope("j"), RunMode::Dataset, None, RunStatus::Running, NOW);
    let result = runner(&transport, &store, &clock)
        .start_unlocker_run(&scope("j"), &one, None, PageAccess::Unlocker, &sink)
        .await;
    assert_eq!(result, Err(RunError::Gate(GateError::ActiveRun)));
    assert_eq!(store.count(), 1);

    let capped = MemoryStore::enabled();
    for n in 0..20 {
        capped.seed(
            &scope(&format!("done-{n}")),
            RunMode::Unlocker,
            None,
            RunStatus::Imported,
            NOW,
        );
    }
    let result = runner(&transport, &capped, &clock)
        .start_unlocker_run(&scope("j"), &one, None, PageAccess::Unlocker, &sink)
        .await;
    assert_eq!(result, Err(RunError::Gate(GateError::RunsPerDayCap)));
    // Yesterday's runs do not count.
    let tomorrow = FixedClock(NOW + 86_400_000);
    transport.unlock.lock().unwrap().push_back(Ok(page()));
    let outcome = runner(&transport, &capped, &tomorrow)
        .start_unlocker_run(&scope("j"), &one, None, PageAccess::Unlocker, &sink)
        .await
        .unwrap();
    assert_eq!(outcome.status, RunStatus::Imported);
    assert_eq!(transport.calls(), ["unlock"]);
}

#[tokio::test]
async fn discovery_without_limit_is_rejected_before_any_call_or_row() {
    let transport = FakeTransport::default();
    let store = MemoryStore::enabled();
    let clock = FixedClock(NOW);
    let unbounded = DatasetTrigger {
        discover_by: Some(DiscoverBy::parse("keyword").unwrap()),
        ..trigger()
    };
    let result = runner(&transport, &store, &clock)
        .start_dataset_run(&scope("j"), &unbounded, &MemorySink::default())
        .await;
    assert_eq!(
        result,
        Err(RunError::Invalid(
            "discovery runs need limit_per_input".to_owned()
        ))
    );
    let zero = DatasetTrigger {
        limit_per_input: Some(0),
        ..unbounded
    };
    let result = runner(&transport, &store, &clock)
        .start_dataset_run(&scope("j"), &zero, &MemorySink::default())
        .await;
    assert!(matches!(result, Err(RunError::Invalid(_))));
    assert!(transport.calls().is_empty());
    assert_eq!(store.count(), 0);
}

#[tokio::test]
async fn dataset_items_count_limit_per_input() {
    let transport = FakeTransport::default();
    let store = MemoryStore::enabled();
    let clock = FixedClock(NOW);
    let wide = DatasetTrigger {
        inputs: (0..5)
            .map(|n| json!({ "keyword": n.to_string() }))
            .collect(),
        discover_by: Some(DiscoverBy::parse("keyword").unwrap()),
        limit_per_input: Some(20),
        ..trigger()
    };
    let result = runner(&transport, &store, &clock)
        .start_dataset_run(&scope("j"), &wide, &MemorySink::default())
        .await;
    assert_eq!(result, Err(RunError::Gate(GateError::ItemsPerRunCap)));
    let overflow = DatasetTrigger {
        limit_per_input: Some(u32::MAX),
        ..wide
    };
    let result = runner(&transport, &store, &clock)
        .start_dataset_run(&scope("j"), &overflow, &MemorySink::default())
        .await;
    assert_eq!(result, Err(RunError::Gate(GateError::ItemsPerRunCap)));
    assert!(transport.calls().is_empty());
    assert_eq!(store.count(), 0);

    // 5 × 10 = 50 fits the per-run cap and is stored as the requested count.
    let transport = dataset_transport([Ok(SnapshotStatus::Ready)], [Ok(records(2))]);
    let fits = DatasetTrigger {
        limit_per_input: Some(10),
        ..overflow
    };
    let outcome = runner(&transport, &store, &clock)
        .start_dataset_run(&scope("j"), &fits, &MemorySink::default())
        .await
        .unwrap();
    assert_eq!(outcome.status, RunStatus::Imported);
    assert_eq!(store.run(outcome.run_id).requested_count, 50);
}

#[tokio::test]
async fn resume_rejects_unresumable_runs() {
    let store = MemoryStore::enabled();
    let clock = FixedClock(NOW);
    let transport = FakeTransport::default();
    let runner = runner(&transport, &store, &clock);
    let sink = MemorySink::default();
    let unlocker = store.seed(
        &scope("a"),
        RunMode::Unlocker,
        None,
        RunStatus::Running,
        NOW,
    );
    let no_snapshot = store.seed(&scope("b"), RunMode::Dataset, None, RunStatus::Running, NOW);
    let bad = store.seed(
        &scope("c"),
        RunMode::Dataset,
        Some("../x"),
        RunStatus::Running,
        NOW,
    );
    let done = store.seed(
        &scope("d"),
        RunMode::Dataset,
        Some(SNAP),
        RunStatus::Imported,
        NOW,
    );
    for id in [unlocker, no_snapshot, bad, done] {
        assert!(
            matches!(
                runner.resume_run(id, &sink).await,
                Err(RunError::NotResumable(_))
            ),
            "{id:?}"
        );
    }
    assert_eq!(store.run(bad).status, RunStatus::Failed);
    let killed = store.seed(
        &scope("e"),
        RunMode::Dataset,
        Some(SNAP),
        RunStatus::Running,
        NOW,
    );
    store.set_kill_switch(true);
    assert_eq!(
        runner.resume_run(killed, &sink).await,
        Err(RunError::Gate(GateError::KillSwitch))
    );
    assert!(transport.calls().is_empty());
}

struct FakeRobots(&'static str);

impl crate::robots::RobotsSource for FakeRobots {
    fn fetch<'a>(&'a self, _host: &'a str) -> crate::robots::FetchFuture<'a> {
        Box::pin(async move { crate::robots::Fetched::Body(self.0.to_owned()) })
    }
}

fn checker(robots: &'static str) -> crate::robots::RobotsChecker {
    crate::robots::RobotsChecker::new(Box::new(FakeRobots(robots)), Box::new(FixedClock(NOW)))
}

#[tokio::test]
async fn robots_disallowed_urls_are_skipped_without_a_request() {
    let transport = FakeTransport::default();
    let store = MemoryStore::enabled();
    let clock = FixedClock(NOW);
    let sink = MemorySink::default();
    let robots = checker(
        "User-agent: *
Disallow: /search",
    );
    let outcome = Runner {
        robots: Some(&robots),
        ..runner(&transport, &store, &clock)
    }
    .start_unlocker_run(
        &scope("j"),
        &[TargetUrl::parse("https://shop.example/search?q=a").unwrap()],
        None,
        PageAccess::Unlocker,
        &sink,
    )
    .await
    .unwrap();
    assert_eq!(outcome.status, RunStatus::BlockedNeedsKyc);
    assert_eq!(outcome.failed_count, 1);
    assert!(transport.calls.lock().unwrap().is_empty());
    let access = store.state.lock().unwrap().access.clone();
    assert_eq!(
        access,
        vec![(
            outcome.run_id.0,
            "shop.example".to_owned(),
            AccessOutcome::SkippedRobots
        )]
    );
}

#[tokio::test]
async fn kyc_refusals_end_the_run_blocked_and_mixed_runs_stay_imported() {
    let transport = FakeTransport {
        unlock: queue([Err(BrightDataError::BlockedNeedsKyc {
            reason: RefusalReason::RobotsDisallowed,
            message: "robots".to_owned(),
        })]),
        ..FakeTransport::default()
    };
    let store = MemoryStore::enabled();
    let clock = FixedClock(NOW);
    let sink = MemorySink::default();
    let one = [TargetUrl::parse("https://shop.example/a").unwrap()];
    let outcome = runner(&transport, &store, &clock)
        .start_unlocker_run(&scope("j"), &one, None, PageAccess::Unlocker, &sink)
        .await
        .unwrap();
    assert_eq!(outcome.status, RunStatus::BlockedNeedsKyc);
    assert_eq!(
        store.state.lock().unwrap().access[0].2,
        AccessOutcome::BlockedNeedsKyc
    );

    let transport = FakeTransport {
        unlock: queue([
            Err(BrightDataError::BlockedNeedsKyc {
                reason: RefusalReason::Policy,
                message: "policy".to_owned(),
            }),
            Ok(page()),
        ]),
        ..FakeTransport::default()
    };
    let store = MemoryStore::enabled();
    let two = [
        TargetUrl::parse("https://shop.example/a").unwrap(),
        TargetUrl::parse("https://shop.example/b").unwrap(),
    ];
    let outcome = runner(&transport, &store, &clock)
        .start_unlocker_run(&scope("j"), &two, None, PageAccess::Unlocker, &sink)
        .await
        .unwrap();
    assert_eq!(outcome.status, RunStatus::Imported);
    assert_eq!((outcome.row_count, outcome.failed_count), (1, 1));
}

#[tokio::test]
async fn browser_access_uses_browse_with_the_same_gates_and_events() {
    let clock = FixedClock(NOW);
    let sink = MemorySink::default();
    let transport = FakeTransport {
        unlock: queue([Ok(page())]),
        ..FakeTransport::default()
    };
    let store = MemoryStore::enabled();
    let outcome = runner(&transport, &store, &clock)
        .start_unlocker_run(&scope("j"), &urls(&["a"]), None, PageAccess::Browser, &sink)
        .await
        .unwrap();
    assert_eq!(outcome.status, RunStatus::Imported);
    assert_eq!(transport.calls(), ["browse"]);
    assert_eq!(store.state.lock().unwrap().access[0].2, AccessOutcome::Ok);
    assert!(
        store
            .run(outcome.run_id)
            .input_json
            .contains("\"access\":\"browser\"")
    );

    let disabled = MemoryStore::default();
    let result = runner(&transport, &disabled, &clock)
        .start_unlocker_run(&scope("j"), &urls(&["a"]), None, PageAccess::Browser, &sink)
        .await;
    assert_eq!(result, Err(RunError::Gate(GateError::ConnectorDisabled)));
    assert_eq!(transport.calls(), ["browse"]);
}
