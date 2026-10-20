//! Resumes the dataset runs that start-up recovery kept active.
//!
//! No product importer exists yet, so results go to [`RefusingSink`]: it
//! refuses every record with fixed text, which leaves a downloaded run
//! `ready` and resumable (the snapshot stays on Bright Data for 24 h).
//!
//! Amazon fallback lookups (job `amazon_mx_fallback`, any tenant) are not
//! resumed: their result was only useful to the caller that died with the
//! worker, and while active they would block every new fallback lookup.
//! They are failed with [`FALLBACK_INTERRUPTED`] instead.

use brightdata::runs::{
    Clock, RunId, RunScope, RunSink, RunStatus, RunStore, RunUpdate, Runner, StoreError,
};
use brightdata::{
    BrightDataClient, BrightDataConfig, ConfigError, SnapshotId, TargetUrl, Transport, UnlockedPage,
};
use http_guard::{Redactor, Secret};
use serde_json::Value;

/// Returned for every result until a product importer is registered.
pub const NO_IMPORTER: &str =
    "No importer is registered for Bright Data results yet; the run stays resumable";

/// Stored on Amazon fallback runs interrupted by a worker stop.
pub const FALLBACK_INTERRUPTED: &str = "Amazon fallback lookup interrupted; its caller is gone";

/// Placeholder [`RunSink`]: refuses pages and records with [`NO_IMPORTER`].
pub struct RefusingSink;

impl RunSink for RefusingSink {
    async fn accept_page(&self, _url: &TargetUrl, _page: UnlockedPage) -> Result<(), String> {
        Err(NO_IMPORTER.to_owned())
    }

    async fn accept_records(&self, _records: Vec<Value>) -> Result<u64, String> {
        Err(NO_IMPORTER.to_owned())
    }
}

/// Builds the live client, or `None` when a credential is missing (runs
/// then fail the key gate, closed). `browser_zone` is optional: without it
/// Browser API fetches fail terminally. Logs name settings only; client
/// errors pass through `redactor`.
pub fn client(
    api_key: Option<Secret>,
    unlocker_zone: Option<Secret>,
    browser_zone: Option<Secret>,
    redactor: &Redactor,
) -> Option<BrightDataClient> {
    let config = match BrightDataConfig::from_secrets(api_key, unlocker_zone) {
        Ok(config) => config.with_browser_zone(browser_zone),
        Err(error @ ConfigError::MissingApiKey) => {
            tracing::info!(error = %error, "Bright Data client not configured");
            return None;
        }
        Err(error @ ConfigError::MissingUnlockerZone) => {
            tracing::warn!(error = %error, "Bright Data client not configured");
            return None;
        }
    };
    match BrightDataClient::new(config) {
        Ok(client) => Some(client),
        Err(error) => {
            let message = redactor.sanitize(error.message());
            tracing::warn!(error = %message, "Bright Data client could not be built");
            None
        }
    }
}

/// How the kept runs ended up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ResumeReport {
    /// Runs now terminal (imported, failed or cancelled).
    pub finished: u32,
    /// Runs still active (still collecting, refused by the sink, or a
    /// transient error); resumable later.
    pub active: u32,
    /// Resumes refused (a gate, an invalid run or storage); the run is
    /// left as it was.
    pub refused: u32,
    /// Amazon fallback runs failed instead of resumed.
    pub abandoned: u32,
}

/// True for an Amazon fallback run scope: job `amazon_mx_fallback`, for
/// any paying tenant.
fn is_amazon_fallback(scope: &RunScope) -> bool {
    scope.job() == amazon::FALLBACK_JOB
}

/// Fails `id` with [`FALLBACK_INTERRUPTED`] when it is an Amazon fallback
/// run and, best effort, cancels its Bright Data collection. Returns false
/// (doing nothing) for any other run, or one that no longer exists.
async fn abandon_fallback<T: Transport, S: RunStore, C: Clock>(
    runner: &Runner<'_, T, S, C>,
    id: RunId,
) -> Result<bool, StoreError> {
    let Some(run) = runner.store.load_run(id).await? else {
        return Ok(false);
    };
    if !is_amazon_fallback(&run.scope) {
        return Ok(false);
    }
    let update = RunUpdate::status(RunStatus::Failed).message(FALLBACK_INTERRUPTED);
    let failed = runner
        .store
        .update_if_active(id, &update, runner.clock.now_ms())
        .await?;
    if failed
        && let Some(transport) = runner.transport
        && let Some(Ok(snapshot)) = run.snapshot_id.as_deref().map(SnapshotId::parse)
    {
        let _ = transport.cancel(&snapshot).await;
    }
    Ok(true)
}

/// Resumes each kept run in order, except Amazon fallback runs, which are
/// failed (see the module docs). The caller must hold the driver lock.
pub async fn resume_kept<T: Transport, S: RunStore, C: Clock>(
    runner: &Runner<'_, T, S, C>,
    kept: &[RunId],
    sink: &impl RunSink,
    redactor: &Redactor,
) -> ResumeReport {
    let mut report = ResumeReport::default();
    for &id in kept {
        match abandon_fallback(runner, id).await {
            Ok(true) => {
                tracing::info!(run_id = id.0, "Amazon fallback run failed, not resumed");
                report.abandoned += 1;
                continue;
            }
            Ok(false) => {}
            Err(error) => {
                let message = redactor.sanitize(&error.to_string());
                tracing::warn!(run_id = id.0, error = %message, "Bright Data run check refused");
                report.refused += 1;
                continue;
            }
        }
        match runner.resume_run(id, sink).await {
            Ok(outcome) => {
                let message = redactor.sanitize(&outcome.message);
                tracing::info!(
                    run_id = id.0,
                    status = outcome.status.as_str(),
                    rows = outcome.row_count,
                    message = %message,
                    "Bright Data run resumed"
                );
                if outcome.status.is_active() {
                    report.active += 1;
                } else {
                    report.finished += 1;
                }
            }
            Err(error) => {
                let message = redactor.sanitize(&error.to_string());
                tracing::warn!(run_id = id.0, error = %message, "Bright Data run resume refused");
                report.refused += 1;
            }
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::{
        FALLBACK_INTERRUPTED, NO_IMPORTER, RefusingSink, ResumeReport, client, resume_kept,
    };
    use brightdata::runs::{
        AccessOutcome, Caps, Clock, GateError, GateState, NewRun, RunError, RunId, RunMode,
        RunRecord, RunScope, RunSink, RunStatus, RunStore, RunUpdate, Runner, StoreError, Timing,
    };
    use brightdata::{BrightDataClient, TargetUrl, UnlockedPage};
    use http_guard::{Redactor, Secret};
    use std::sync::Mutex;

    const NOW: i64 = 1_790_000_000_000;

    struct FixedClock;

    impl Clock for FixedClock {
        fn now_ms(&self) -> i64 {
            NOW
        }
    }

    /// One kept dataset run; the connector is enabled and the kill switch
    /// off, so only the key gate can refuse.
    struct KeptStore;

    fn kept_run() -> RunRecord {
        RunRecord {
            id: RunId(7),
            scope: RunScope::new("tenant-1", "amazon").unwrap(),
            mode: RunMode::Dataset,
            input_json: "{}".to_owned(),
            snapshot_id: Some("sd_test".to_owned()),
            status: RunStatus::Running,
            requested_count: 1,
            row_count: 0,
            failed_count: 0,
            error_message: String::new(),
            created_at_ms: NOW,
            updated_at_ms: NOW,
        }
    }

    impl RunStore for KeptStore {
        async fn gate_state(&self, _: &RunScope, _: i64) -> Result<GateState, StoreError> {
            Ok(GateState {
                connector_enabled: true,
                ..GateState::default()
            })
        }
        async fn insert_run_capped(
            &self,
            _: &NewRun,
            _: i64,
            _: &Caps,
        ) -> Result<RunId, StoreError> {
            Err(StoreError::Storage("unused".to_owned()))
        }
        async fn update_if_active(
            &self,
            _: RunId,
            _: &RunUpdate,
            _: i64,
        ) -> Result<bool, StoreError> {
            Ok(true)
        }
        async fn load_run(&self, id: RunId) -> Result<Option<RunRecord>, StoreError> {
            Ok((id == RunId(7)).then(kept_run))
        }
        async fn active_runs(&self) -> Result<Vec<RunRecord>, StoreError> {
            Ok(vec![kept_run()])
        }
        async fn kill_switch_on(&self) -> Result<bool, StoreError> {
            Ok(false)
        }
        async fn record_access(
            &self,
            _: RunId,
            _: &str,
            _: AccessOutcome,
        ) -> Result<(), StoreError> {
            Ok(())
        }
    }

    fn secret(value: &str) -> Option<Secret> {
        Some(Secret::new(value.to_owned()).unwrap())
    }

    #[test]
    fn client_needs_key_and_zone() {
        let redactor = Redactor::default();
        assert!(client(None, None, None, &redactor).is_none());
        assert!(client(None, secret("zone"), None, &redactor).is_none());
        assert!(client(secret("key"), None, secret("browser"), &redactor).is_none());
        assert!(client(secret("key"), secret("zone"), None, &redactor).is_some());
        assert!(client(secret("key"), secret("zone"), secret("browser"), &redactor).is_some());
    }

    #[tokio::test]
    async fn sink_refuses_with_fixed_text() {
        let sink = RefusingSink;
        let url = TargetUrl::parse("https://www.amazon.com.mx/dp/B000000000").unwrap();
        let page = UnlockedPage {
            status: 200,
            body: String::new(),
        };
        assert_eq!(
            sink.accept_page(&url, page).await,
            Err(NO_IMPORTER.to_owned())
        );
        assert_eq!(
            sink.accept_records(vec![serde_json::json!({})]).await,
            Err(NO_IMPORTER.to_owned())
        );
    }

    #[tokio::test]
    async fn kept_runs_without_a_key_fail_the_key_gate() {
        let runner: Runner<'_, BrightDataClient, _, _> = Runner {
            transport: None,
            store: &KeptStore,
            clock: &FixedClock,
            timing: Timing::default(),
            caps: Caps::default(),
            robots: None,
        };
        assert_eq!(
            runner.resume_run(RunId(7), &RefusingSink).await,
            Err(RunError::Gate(GateError::MissingKey))
        );
        let redactor = Redactor::default();
        let report = resume_kept(&runner, &[RunId(7), RunId(8)], &RefusingSink, &redactor).await;
        assert_eq!(
            report,
            ResumeReport {
                refused: 2,
                ..ResumeReport::default()
            }
        );
    }

    /// In-memory runs; the connector is enabled and the kill switch off.
    struct MemStore(Mutex<Vec<RunRecord>>);

    fn running(id: i64, tenant: &str, job: &str) -> RunRecord {
        RunRecord {
            id: RunId(id),
            scope: RunScope::new(tenant, job).unwrap(),
            ..kept_run()
        }
    }

    impl RunStore for MemStore {
        async fn gate_state(&self, _: &RunScope, _: i64) -> Result<GateState, StoreError> {
            Ok(GateState {
                connector_enabled: true,
                ..GateState::default()
            })
        }
        async fn insert_run_capped(
            &self,
            _: &NewRun,
            _: i64,
            _: &Caps,
        ) -> Result<RunId, StoreError> {
            Err(StoreError::Storage("unused".to_owned()))
        }
        async fn update_if_active(
            &self,
            id: RunId,
            update: &RunUpdate,
            now_ms: i64,
        ) -> Result<bool, StoreError> {
            let mut runs = self.0.lock().unwrap();
            let Some(run) = runs
                .iter_mut()
                .find(|run| run.id == id && run.status.is_active())
            else {
                return Ok(false);
            };
            if let Some(status) = update.status {
                run.status = status;
            }
            if let Some(message) = &update.error_message {
                run.error_message.clone_from(message);
            }
            run.updated_at_ms = now_ms;
            Ok(true)
        }
        async fn load_run(&self, id: RunId) -> Result<Option<RunRecord>, StoreError> {
            let runs = self.0.lock().unwrap();
            Ok(runs.iter().find(|run| run.id == id).cloned())
        }
        async fn active_runs(&self) -> Result<Vec<RunRecord>, StoreError> {
            let runs = self.0.lock().unwrap();
            Ok(runs
                .iter()
                .filter(|run| run.status.is_active())
                .cloned()
                .collect())
        }
        async fn kill_switch_on(&self) -> Result<bool, StoreError> {
            Ok(false)
        }
        async fn record_access(
            &self,
            _: RunId,
            _: &str,
            _: AccessOutcome,
        ) -> Result<(), StoreError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn kept_amazon_fallback_runs_fail_instead_of_resuming() {
        let store = MemStore(Mutex::new(vec![
            running(1, amazon::FALLBACK_TENANT, amazon::FALLBACK_JOB),
            running(2, "tenant-1", "amazon"),
            running(3, "default", amazon::FALLBACK_JOB),
        ]));
        let runner: Runner<'_, BrightDataClient, _, _> = Runner {
            transport: None,
            store: &store,
            clock: &FixedClock,
            timing: Timing::default(),
            caps: Caps::default(),
            robots: None,
        };
        let redactor = Redactor::default();
        let kept = [RunId(1), RunId(2), RunId(3)];
        let report = resume_kept(&runner, &kept, &RefusingSink, &redactor).await;
        // Run 2 is still resumed (and, without a key, refused by the gate).
        assert_eq!(
            report,
            ResumeReport {
                refused: 1,
                abandoned: 2,
                ..ResumeReport::default()
            }
        );
        // Fallback runs fail whichever tenant paid for them.
        for id in [RunId(1), RunId(3)] {
            let fallback = store.load_run(id).await.unwrap().unwrap();
            assert_eq!(fallback.status, RunStatus::Failed);
            assert_eq!(fallback.error_message, FALLBACK_INTERRUPTED);
        }
        let active: Vec<RunId> = store
            .active_runs()
            .await
            .unwrap()
            .iter()
            .map(|run| run.id)
            .collect();
        assert_eq!(active, vec![RunId(2)]);
    }
}
