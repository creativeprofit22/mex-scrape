//! Durable, capped Bright Data runs (ported from linkgo `brightdata/runs.rs`
//! and `store.rs`, async, with a generic tenant + job scope instead of a
//! campaign).
//!
//! A run is a row in storage ([`RunStore`]); this module decides what to
//! write. Every start passes [`check_gates`] in a fixed order and fails
//! closed. Results go to a caller-supplied [`RunSink`], so this crate never
//! sees product types.

use std::future::Future;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use tokio::time::Instant;

use crate::client::{DatasetTrigger, Download, SnapshotStatus, UnlockRequest, UnlockedPage};
use crate::error::BrightDataError;
use crate::ids::{CountryCode, SnapshotId, TargetUrl};
use crate::transport::Transport;

/// Longest error message stored on a run (characters).
pub const MAX_RUN_ERROR_CHARS: usize = 1000;
/// Largest serialized run input (bytes).
pub const MAX_INPUT_JSON_BYTES: usize = 20_000;
/// One UTC day in milliseconds; daily caps reset at UTC midnight.
pub const DAY_MS: i64 = 86_400_000;

/// Stored when the global kill switch stops a run.
pub const KILL_SWITCH_STOP: &str = "Stopped by the global kill switch";
/// Stored when a run is cancelled through [`Runner::cancel_run`].
pub const CANCELLED_BY_REQUEST: &str = "Cancelled by request";
/// Stored when Bright Data reports a failed collection.
pub const COLLECTION_FAILED: &str = "Bright Data reported the collection failed";
/// Stored when Bright Data reports a cancelled collection.
pub const COLLECTION_CANCELLED: &str = "Bright Data cancelled the collection";
/// Stored when the poll budget runs out; the run stays resumable.
pub const POLL_BUDGET_EXHAUSTED: &str = "Bright Data is still collecting; resume the run later";
/// Stored when every page of a Web Unlocker run failed without a message.
pub const NO_PAGE_FETCHED: &str = "No page could be fetched";

/// Start of the UTC day containing `now_ms`.
#[must_use]
pub fn day_start_ms(now_ms: i64) -> i64 {
    now_ms - now_ms.rem_euclid(DAY_MS)
}

/// Bounds a message to [`MAX_RUN_ERROR_CHARS`] characters (callers pass
/// text that is already redacted).
#[must_use]
pub fn bound_error(message: &str) -> String {
    if message.chars().count() <= MAX_RUN_ERROR_CHARS {
        return message.to_owned();
    }
    let mut bounded: String = message.chars().take(MAX_RUN_ERROR_CHARS - 1).collect();
    bounded.push('…');
    bounded
}

fn valid_scope_part(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b':' | b'-'))
}

/// Who a run belongs to: a tenant and a job name (e.g. `liverpool:prices`).
/// At most one run per scope is active at a time.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RunScope {
    tenant_id: String,
    job: String,
}

impl RunScope {
    /// Validates both parts: 1–100 characters of `[A-Za-z0-9_.:-]`.
    ///
    /// # Errors
    ///
    /// [`RunError::Invalid`].
    pub fn new(tenant_id: &str, job: &str) -> Result<Self, RunError> {
        if !valid_scope_part(tenant_id) || !valid_scope_part(job) {
            return Err(RunError::Invalid("invalid run scope".to_owned()));
        }
        Ok(Self {
            tenant_id: tenant_id.to_owned(),
            job: job.to_owned(),
        })
    }

    /// The tenant id.
    #[must_use]
    pub fn tenant_id(&self) -> &str {
        &self.tenant_id
    }

    /// The job name.
    #[must_use]
    pub fn job(&self) -> &str {
        &self.job
    }
}

/// How a run fetches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RunMode {
    /// Web Unlocker page fetches (not resumable).
    Unlocker,
    /// A Datasets v3 snapshot (resumable by snapshot id).
    Dataset,
}

impl RunMode {
    /// Stored text.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unlocker => "unlocker",
            Self::Dataset => "dataset",
        }
    }

    /// Parses stored text.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "unlocker" => Some(Self::Unlocker),
            "dataset" => Some(Self::Dataset),
            _ => None,
        }
    }
}

/// Run lifecycle. `starting`, `running` and `ready` are active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RunStatus {
    /// Inserted; nothing sent yet.
    Starting,
    /// Fetching, or waiting for a snapshot.
    Running,
    /// Snapshot ready; downloading/importing.
    Ready,
    /// Finished; results handed to the sink.
    Imported,
    /// Finished with an error.
    Failed,
    /// Stopped by request, by the kill switch, or by Bright Data.
    Cancelled,
}

impl RunStatus {
    /// Every status, in lifecycle order.
    pub const ALL: [Self; 6] = [
        Self::Starting,
        Self::Running,
        Self::Ready,
        Self::Imported,
        Self::Failed,
        Self::Cancelled,
    ];

    /// Stored text.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Ready => "ready",
            Self::Imported => "imported",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    /// Parses stored text.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|status| status.as_str() == value)
    }

    /// True for `starting`, `running` and `ready`.
    #[must_use]
    pub fn is_active(self) -> bool {
        matches!(self, Self::Starting | Self::Running | Self::Ready)
    }
}

/// A stored run id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RunId(pub i64);

/// A stored run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRecord {
    /// Id.
    pub id: RunId,
    /// Owner.
    pub scope: RunScope,
    /// Fetch mode.
    pub mode: RunMode,
    /// Serialized input (URLs or trigger), at most [`MAX_INPUT_JSON_BYTES`].
    pub input_json: String,
    /// Snapshot id as stored (validated again before use).
    pub snapshot_id: Option<String>,
    /// Status.
    pub status: RunStatus,
    /// Items requested (URLs or trigger inputs).
    pub requested_count: u32,
    /// Items imported.
    pub row_count: u32,
    /// Items that failed.
    pub failed_count: u32,
    /// Last error, bounded and redacted; empty when none.
    pub error_message: String,
    /// Creation time (epoch ms).
    pub created_at_ms: i64,
    /// Last update (epoch ms).
    pub updated_at_ms: i64,
}

/// A run to insert.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewRun {
    /// Owner.
    pub scope: RunScope,
    /// Fetch mode.
    pub mode: RunMode,
    /// Serialized input.
    pub input_json: String,
    /// Items requested.
    pub requested_count: u32,
    /// Creation time (epoch ms).
    pub now_ms: i64,
}

/// Fields to change on an active run; `None` leaves a field as is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunUpdate {
    /// New status.
    pub status: Option<RunStatus>,
    /// Snapshot id.
    pub snapshot_id: Option<SnapshotId>,
    /// Imported items.
    pub row_count: Option<u32>,
    /// Failed items.
    pub failed_count: Option<u32>,
    /// Error message (bounded by the runner).
    pub error_message: Option<String>,
}

impl RunUpdate {
    /// Changes only the status.
    #[must_use]
    pub fn status(status: RunStatus) -> Self {
        Self {
            status: Some(status),
            ..Self::default()
        }
    }

    /// Sets the (bounded) error message.
    #[must_use]
    pub fn message(mut self, message: &str) -> Self {
        self.error_message = Some(bound_error(message));
        self
    }
}

/// What the gates need to know, read from storage. A store with no settings
/// row must report the kill switch on and the connector disabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GateState {
    /// Global kill switch is on.
    pub kill_switch_on: bool,
    /// The Bright Data connector is enabled.
    pub connector_enabled: bool,
    /// Runs this tenant created today (UTC).
    pub runs_today: u32,
    /// Items this tenant requested today (UTC).
    pub items_today: u32,
    /// The scope already has an active run.
    pub active_run: bool,
}

/// Per-tenant spending caps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caps {
    /// Runs per tenant per UTC day (default 20).
    pub max_runs_per_tenant_per_day: u32,
    /// Items (URLs or inputs) per tenant per UTC day (default 500).
    pub max_items_per_tenant_per_day: u32,
    /// Items per run (default 50).
    pub max_items_per_run: u32,
}

impl Default for Caps {
    fn default() -> Self {
        Self {
            max_runs_per_tenant_per_day: 20,
            max_items_per_tenant_per_day: 500,
            max_items_per_run: 50,
        }
    }
}

/// Snapshot polling (as linkgo: every 10 s, for up to 10 minutes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timing {
    /// Delay between progress polls.
    pub poll_interval: Duration,
    /// How long one drive may poll before leaving the run resumable.
    pub poll_budget: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(10),
            poll_budget: Duration::from_secs(600),
        }
    }
}

/// Why a run may not start. Checked in declaration order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum GateError {
    /// The global kill switch is on (or settings are missing).
    #[error("Bright Data is stopped by the global kill switch")]
    KillSwitch,
    /// The connector is disabled (or settings are missing).
    #[error("The Bright Data connector is not enabled")]
    ConnectorDisabled,
    /// No API key is configured.
    #[error("No Bright Data API key is configured")]
    MissingKey,
    /// The tenant's daily run cap is reached.
    #[error("Daily Bright Data run limit reached for this tenant")]
    RunsPerDayCap,
    /// The run asks for more items than one run may carry.
    #[error("Too many items for one Bright Data run")]
    ItemsPerRunCap,
    /// The tenant's daily item cap would be exceeded.
    #[error("Daily Bright Data item limit reached for this tenant")]
    ItemsPerDayCap,
    /// The scope already has an active run.
    #[error("A Bright Data run is already active for this job")]
    ActiveRun,
}

/// Start gates, in order, failing closed: kill switch → connector enabled →
/// key present → runs/day → items/run → items/day → one active run.
///
/// # Errors
///
/// The first [`GateError`] that applies.
pub fn check_gates(
    state: &GateState,
    key_present: bool,
    caps: &Caps,
    requested: u32,
) -> Result<(), GateError> {
    check_resume_gates(state, key_present)?;
    if state.runs_today >= caps.max_runs_per_tenant_per_day {
        return Err(GateError::RunsPerDayCap);
    }
    if requested > caps.max_items_per_run {
        return Err(GateError::ItemsPerRunCap);
    }
    if state.items_today.saturating_add(requested) > caps.max_items_per_tenant_per_day {
        return Err(GateError::ItemsPerDayCap);
    }
    if state.active_run {
        return Err(GateError::ActiveRun);
    }
    Ok(())
}

/// Resume gates: kill switch → connector enabled → key present. Caps were
/// paid at start and the run is already the scope's active run.
///
/// # Errors
///
/// The first [`GateError`] that applies.
pub fn check_resume_gates(state: &GateState, key_present: bool) -> Result<(), GateError> {
    if state.kill_switch_on {
        return Err(GateError::KillSwitch);
    }
    if !state.connector_enabled {
        return Err(GateError::ConnectorDisabled);
    }
    if !key_present {
        return Err(GateError::MissingKey);
    }
    Ok(())
}

/// Storage failures and atomic gate re-checks.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    /// The scope already has an active run (unique index).
    #[error("A Bright Data run is already active for this job")]
    ActiveRun,
    /// A cap was exceeded when re-checked inside the insert.
    #[error("{0}")]
    CapExceeded(GateError),
    /// Storage failed; the message is fixed or redacted.
    #[error("{0}")]
    Storage(String),
}

/// Why a run operation failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RunError {
    /// A start gate refused the run.
    #[error("{0}")]
    Gate(GateError),
    /// The input was rejected before anything was stored.
    #[error("invalid run input: {0}")]
    Invalid(String),
    /// No run with this id.
    #[error("Bright Data run not found")]
    NotFound,
    /// The run cannot be resumed.
    #[error("Bright Data run cannot be resumed: {0}")]
    NotResumable(&'static str),
    /// Storage failed.
    #[error("{0}")]
    Store(String),
}

impl From<GateError> for RunError {
    fn from(error: GateError) -> Self {
        Self::Gate(error)
    }
}

impl From<StoreError> for RunError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::ActiveRun => Self::Gate(GateError::ActiveRun),
            StoreError::CapExceeded(gate) => Self::Gate(gate),
            StoreError::Storage(message) => Self::Store(message),
        }
    }
}

/// Durable run storage. Implementations must be atomic where noted.
pub trait RunStore: Send + Sync {
    /// Settings and today's counts for `scope` (days start at
    /// `day_start_ms`). Missing settings ⇒ kill switch on, disabled.
    fn gate_state(
        &self,
        scope: &RunScope,
        day_start_ms: i64,
    ) -> impl Future<Output = Result<GateState, StoreError>> + Send;

    /// Inserts a `starting` run after re-checking the tenant caps and the
    /// one-active-run rule atomically.
    fn insert_run_capped(
        &self,
        run: &NewRun,
        day_start_ms: i64,
        caps: &Caps,
    ) -> impl Future<Output = Result<RunId, StoreError>> + Send;

    /// Applies `update` only while the run is active; returns false when
    /// the run is already terminal (so a cancel from elsewhere sticks).
    fn update_if_active(
        &self,
        id: RunId,
        update: &RunUpdate,
        now_ms: i64,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send;

    /// Loads a run.
    fn load_run(
        &self,
        id: RunId,
    ) -> impl Future<Output = Result<Option<RunRecord>, StoreError>> + Send;

    /// Every active run, oldest first.
    fn active_runs(&self) -> impl Future<Output = Result<Vec<RunRecord>, StoreError>> + Send;

    /// The global kill switch (missing settings ⇒ on).
    fn kill_switch_on(&self) -> impl Future<Output = Result<bool, StoreError>> + Send;
}

/// Where run results go. Implemented by callers (store adapters, the
/// Amazon fallback), which parse and persist product data.
pub trait RunSink: Send + Sync {
    /// One unlocked page. An error counts the URL as failed; it must be
    /// redacted.
    fn accept_page(
        &self,
        url: &TargetUrl,
        page: UnlockedPage,
    ) -> impl Future<Output = Result<(), String>> + Send;

    /// A downloaded snapshot. Returns how many records were imported. An
    /// error leaves the run `ready` and resumable; it must be redacted.
    fn accept_records(
        &self,
        records: Vec<Value>,
    ) -> impl Future<Output = Result<u64, String>> + Send;
}

/// Wall clock in epoch milliseconds; injected so tests are deterministic.
pub trait Clock: Send + Sync {
    /// Now, in milliseconds since the Unix epoch.
    fn now_ms(&self) -> i64;
}

/// The system clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| {
                i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
            })
    }
}

/// Where a run ended up after a start, resume or drive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutcome {
    /// The run.
    pub run_id: RunId,
    /// Its status now (active statuses mean "resume later").
    pub status: RunStatus,
    /// Imported items.
    pub row_count: u32,
    /// Failed items.
    pub failed_count: u32,
    /// Stored error message (empty when none).
    pub message: String,
}

impl From<RunRecord> for RunOutcome {
    fn from(run: RunRecord) -> Self {
        Self {
            run_id: run.id,
            status: run.status,
            row_count: run.row_count,
            failed_count: run.failed_count,
            message: run.error_message,
        }
    }
}

/// Starts, drives, resumes and cancels runs. `transport` is `None` when no
/// API key is configured, which fails the key gate.
///
/// One process should drive a given run at a time (the worker takes a
/// driver lock before start-up recovery).
pub struct Runner<'a, T, S, C> {
    /// Bright Data, or `None` without an API key.
    pub transport: Option<&'a T>,
    /// Run storage.
    pub store: &'a S,
    /// Clock for stored timestamps and daily caps.
    pub clock: &'a C,
    /// Poll timing.
    pub timing: Timing,
    /// Per-tenant caps.
    pub caps: Caps,
}

impl<'a, T: Transport, S: RunStore, C: Clock> Runner<'a, T, S, C> {
    /// Fetches `urls` with Web Unlocker as one run and hands each page to
    /// `sink`. Ends `imported` if any page was accepted, else `failed`.
    ///
    /// # Errors
    ///
    /// [`RunError::Invalid`] or [`RunError::Gate`] before anything is
    /// stored; [`RunError::Store`] if storage fails mid-run (the run is then
    /// failed by the next start-up recovery).
    pub async fn start_unlocker_run(
        &self,
        scope: &RunScope,
        urls: &[TargetUrl],
        country: Option<CountryCode>,
        sink: &impl RunSink,
    ) -> Result<RunOutcome, RunError> {
        if urls.is_empty() {
            return Err(RunError::Invalid("no URLs to fetch".to_owned()));
        }
        let requested = item_count(urls.len())?;
        let input = json!({
            "urls": urls.iter().map(TargetUrl::as_str).collect::<Vec<_>>(),
            "country": country.as_ref().map(CountryCode::as_str),
        });
        let input_json = bounded_input(&input)?;
        let transport = self.gate(scope, requested).await?;
        let id = self
            .insert(scope, RunMode::Unlocker, input_json, requested)
            .await?;
        if !self
            .update(id, RunUpdate::status(RunStatus::Running))
            .await?
        {
            return self.outcome(id).await;
        }
        let (mut rows, mut failed) = (0_u32, 0_u32);
        let mut first_error: Option<String> = None;
        for url in urls {
            if self.stop_requested(id, transport, None).await? {
                return self.outcome(id).await;
            }
            let request = UnlockRequest {
                url: url.clone(),
                country,
            };
            let result = match transport.unlock(&request).await {
                Ok(page) => sink.accept_page(url, page).await,
                Err(error) => Err(error.message().to_owned()),
            };
            match result {
                Ok(()) => rows += 1,
                Err(message) => {
                    failed += 1;
                    first_error.get_or_insert(message);
                }
            }
            let counts = RunUpdate {
                row_count: Some(rows),
                failed_count: Some(failed),
                ..RunUpdate::default()
            };
            if !self.update(id, counts).await? {
                return self.outcome(id).await;
            }
        }
        let message = first_error.unwrap_or_default();
        let last = if rows == 0 {
            let message = if message.is_empty() {
                NO_PAGE_FETCHED
            } else {
                &message
            };
            RunUpdate::status(RunStatus::Failed).message(message)
        } else {
            RunUpdate::status(RunStatus::Imported).message(&message)
        };
        self.update(id, last).await?;
        self.outcome(id).await
    }

    /// Triggers a Datasets v3 collection as one run and drives it until it
    /// is imported, fails, is cancelled, or the poll budget runs out (then
    /// it stays active and can be resumed with [`Runner::resume_run`]).
    ///
    /// # Errors
    ///
    /// As [`Runner::start_unlocker_run`].
    pub async fn start_dataset_run(
        &self,
        scope: &RunScope,
        trigger: &DatasetTrigger,
        sink: &impl RunSink,
    ) -> Result<RunOutcome, RunError> {
        trigger
            .validate()
            .map_err(|error| RunError::Invalid(error.message().to_owned()))?;
        let requested = dataset_item_count(trigger)?;
        let input = json!({
            "dataset_id": trigger.dataset_id.as_str(),
            "inputs": trigger.inputs,
            "discover_by": trigger.discover_by.as_ref().map(|value| value.as_str()),
            "limit_per_input": trigger.limit_per_input,
        });
        let input_json = bounded_input(&input)?;
        let transport = self.gate(scope, requested).await?;
        let id = self
            .insert(scope, RunMode::Dataset, input_json, requested)
            .await?;
        let snapshot = match transport.trigger(trigger).await {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.update(
                    id,
                    RunUpdate::status(RunStatus::Failed).message(error.message()),
                )
                .await?;
                return self.outcome(id).await;
            }
        };
        let started = RunUpdate {
            snapshot_id: Some(snapshot.clone()),
            ..RunUpdate::status(RunStatus::Running)
        };
        if !self.update(id, started).await? {
            // Cancelled while triggering: stop the paid collection too.
            let _ = transport.cancel(&snapshot).await;
            return self.outcome(id).await;
        }
        self.drive_snapshot(id, transport, &snapshot, sink).await
    }

    /// Continues an active dataset run from its stored snapshot id. Checks
    /// the kill switch, connector and key gates (caps were paid at start).
    ///
    /// # Errors
    ///
    /// [`RunError::NotFound`], [`RunError::NotResumable`],
    /// [`RunError::Gate`] or [`RunError::Store`].
    pub async fn resume_run(&self, id: RunId, sink: &impl RunSink) -> Result<RunOutcome, RunError> {
        let run = self.store.load_run(id).await?.ok_or(RunError::NotFound)?;
        if !run.status.is_active() {
            return Err(RunError::NotResumable("the run is not active"));
        }
        if run.mode != RunMode::Dataset {
            return Err(RunError::NotResumable(
                "Web Unlocker runs cannot be resumed",
            ));
        }
        let Some(raw) = run.snapshot_id.as_deref() else {
            return Err(RunError::NotResumable("the run has no snapshot id"));
        };
        let Ok(snapshot) = SnapshotId::parse(raw) else {
            self.update(
                id,
                RunUpdate::status(RunStatus::Failed).message("The stored snapshot id is invalid"),
            )
            .await?;
            return Err(RunError::NotResumable("the stored snapshot id is invalid"));
        };
        let state = self
            .store
            .gate_state(&run.scope, day_start_ms(self.clock.now_ms()))
            .await?;
        check_resume_gates(&state, self.transport.is_some())?;
        let transport = self.transport.ok_or(GateError::MissingKey)?;
        self.drive_snapshot(id, transport, &snapshot, sink).await
    }

    /// Cancels an active run and, best effort, its Bright Data collection.
    /// Never gated, so it works with the kill switch on. Returns false if
    /// the run had already finished.
    ///
    /// # Errors
    ///
    /// [`RunError::NotFound`] or [`RunError::Store`].
    pub async fn cancel_run(&self, id: RunId) -> Result<bool, RunError> {
        let run = self.store.load_run(id).await?.ok_or(RunError::NotFound)?;
        let cancelled = self
            .update(
                id,
                RunUpdate::status(RunStatus::Cancelled).message(CANCELLED_BY_REQUEST),
            )
            .await?;
        if cancelled
            && let Some(transport) = self.transport
            && let Some(Ok(snapshot)) = run.snapshot_id.as_deref().map(SnapshotId::parse)
        {
            let _ = transport.cancel(&snapshot).await;
        }
        Ok(cancelled)
    }

    /// Polls a snapshot until it is imported, fails, is cancelled or the
    /// budget runs out (linkgo `drive_snapshot`). Transient errors and an
    /// exhausted budget leave the run active and resumable.
    async fn drive_snapshot(
        &self,
        id: RunId,
        transport: &T,
        snapshot: &SnapshotId,
        sink: &impl RunSink,
    ) -> Result<RunOutcome, RunError> {
        let started = Instant::now();
        loop {
            if self.stop_requested(id, transport, Some(snapshot)).await? {
                return self.outcome(id).await;
            }
            match transport.progress(snapshot).await {
                Ok(SnapshotStatus::Running) => {}
                Ok(SnapshotStatus::Ready) => {
                    if !self.update(id, RunUpdate::status(RunStatus::Ready)).await? {
                        continue;
                    }
                    match transport.download(snapshot).await {
                        Ok(Download::NotReady) => {}
                        Ok(Download::Records(records)) => {
                            let update = match sink.accept_records(records).await {
                                Ok(count) => RunUpdate {
                                    row_count: Some(u32::try_from(count).unwrap_or(u32::MAX)),
                                    ..RunUpdate::status(RunStatus::Imported).message("")
                                },
                                Err(message) => RunUpdate::default().message(&message),
                            };
                            self.update(id, update).await?;
                            return self.outcome(id).await;
                        }
                        Err(error) => return self.record_error(id, &error).await,
                    }
                }
                Ok(SnapshotStatus::Failed) => {
                    let update = RunUpdate::status(RunStatus::Failed).message(COLLECTION_FAILED);
                    self.update(id, update).await?;
                    return self.outcome(id).await;
                }
                Ok(SnapshotStatus::Cancelled) => {
                    let update =
                        RunUpdate::status(RunStatus::Cancelled).message(COLLECTION_CANCELLED);
                    self.update(id, update).await?;
                    return self.outcome(id).await;
                }
                Err(error) => return self.record_error(id, &error).await,
            }
            if started.elapsed() >= self.timing.poll_budget {
                self.update(id, RunUpdate::default().message(POLL_BUDGET_EXHAUSTED))
                    .await?;
                return self.outcome(id).await;
            }
            tokio::time::sleep(self.timing.poll_interval).await;
        }
    }

    /// Transient: keep the status (resumable) and store the message.
    /// Terminal: fail the run.
    async fn record_error(
        &self,
        id: RunId,
        error: &BrightDataError,
    ) -> Result<RunOutcome, RunError> {
        let update = if error.is_transient() {
            RunUpdate::default().message(error.message())
        } else {
            RunUpdate::status(RunStatus::Failed).message(error.message())
        };
        self.update(id, update).await?;
        self.outcome(id).await
    }

    /// True when the run must stop: the kill switch is on (the run is
    /// cancelled) or the run is no longer active (cancelled elsewhere). A
    /// running snapshot is cancelled best effort either way.
    async fn stop_requested(
        &self,
        id: RunId,
        transport: &T,
        snapshot: Option<&SnapshotId>,
    ) -> Result<bool, RunError> {
        let stop = if self.store.kill_switch_on().await? {
            let update = RunUpdate::status(RunStatus::Cancelled).message(KILL_SWITCH_STOP);
            self.update(id, update).await?;
            true
        } else {
            let run = self.store.load_run(id).await?.ok_or(RunError::NotFound)?;
            !run.status.is_active()
        };
        if stop && let Some(snapshot) = snapshot {
            let _ = transport.cancel(snapshot).await;
        }
        Ok(stop)
    }

    async fn gate(&self, scope: &RunScope, requested: u32) -> Result<&'a T, RunError> {
        let day_start = day_start_ms(self.clock.now_ms());
        let state = self.store.gate_state(scope, day_start).await?;
        check_gates(&state, self.transport.is_some(), &self.caps, requested)?;
        Ok(self.transport.ok_or(GateError::MissingKey)?)
    }

    async fn insert(
        &self,
        scope: &RunScope,
        mode: RunMode,
        input_json: String,
        requested_count: u32,
    ) -> Result<RunId, RunError> {
        let now_ms = self.clock.now_ms();
        let run = NewRun {
            scope: scope.clone(),
            mode,
            input_json,
            requested_count,
            now_ms,
        };
        Ok(self
            .store
            .insert_run_capped(&run, day_start_ms(now_ms), &self.caps)
            .await?)
    }

    async fn update(&self, id: RunId, update: RunUpdate) -> Result<bool, RunError> {
        Ok(self
            .store
            .update_if_active(id, &update, self.clock.now_ms())
            .await?)
    }

    async fn outcome(&self, id: RunId) -> Result<RunOutcome, RunError> {
        let run = self.store.load_run(id).await?.ok_or(RunError::NotFound)?;
        Ok(run.into())
    }
}

fn item_count(len: usize) -> Result<u32, RunError> {
    u32::try_from(len).map_err(|_| RunError::Gate(GateError::ItemsPerRunCap))
}

/// Billable items of a dataset trigger: one per input, or `limit_per_input`
/// per input when set. Discovery without a limit is unbounded (Bright Data
/// bills per record), so it is rejected, as is a zero limit.
fn dataset_item_count(trigger: &DatasetTrigger) -> Result<u32, RunError> {
    let inputs = item_count(trigger.inputs.len())?;
    match (trigger.discover_by.as_ref(), trigger.limit_per_input) {
        (_, Some(0)) => Err(RunError::Invalid(
            "limit_per_input must be at least 1".to_owned(),
        )),
        (Some(_), None) => Err(RunError::Invalid(
            "discovery runs need limit_per_input".to_owned(),
        )),
        (None, None) => Ok(inputs),
        (_, Some(limit)) => inputs
            .checked_mul(limit)
            .ok_or(RunError::Gate(GateError::ItemsPerRunCap)),
    }
}

fn bounded_input(input: &Value) -> Result<String, RunError> {
    let text = input.to_string();
    if text.len() > MAX_INPUT_JSON_BYTES {
        return Err(RunError::Invalid(format!(
            "run input is larger than {MAX_INPUT_JSON_BYTES} bytes"
        )));
    }
    Ok(text)
}

#[cfg(test)]
pub(crate) mod tests;
