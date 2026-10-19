//! Resumes the dataset runs that start-up recovery kept active.
//!
//! No product importer exists yet, so results go to [`RefusingSink`]: it
//! refuses every record with fixed text, which leaves a downloaded run
//! `ready` and resumable (the snapshot stays on Bright Data for 24 h).

use brightdata::runs::{Clock, RunId, RunSink, RunStore, Runner};
use brightdata::{
    BrightDataClient, BrightDataConfig, ConfigError, TargetUrl, Transport, UnlockedPage,
};
use http_guard::{Redactor, Secret};
use serde_json::Value;

/// Returned for every result until a product importer is registered.
pub(crate) const NO_IMPORTER: &str =
    "No importer is registered for Bright Data results yet; the run stays resumable";

/// Placeholder [`RunSink`]: refuses pages and records with [`NO_IMPORTER`].
pub(crate) struct RefusingSink;

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
pub(crate) fn client(
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
pub(crate) struct ResumeReport {
    /// Runs now terminal (imported, failed or cancelled).
    pub(crate) finished: u32,
    /// Runs still active (still collecting, refused by the sink, or a
    /// transient error); resumable later.
    pub(crate) active: u32,
    /// Resumes refused (a gate, an invalid run or storage); the run is
    /// left as it was.
    pub(crate) refused: u32,
}

/// Resumes each kept run in order. The caller must hold the driver lock.
pub(crate) async fn resume_kept<T: Transport, S: RunStore, C: Clock>(
    runner: &Runner<'_, T, S, C>,
    kept: &[RunId],
    sink: &impl RunSink,
    redactor: &Redactor,
) -> ResumeReport {
    let mut report = ResumeReport::default();
    for &id in kept {
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
    use super::{NO_IMPORTER, RefusingSink, ResumeReport, client, resume_kept};
    use brightdata::runs::{
        AccessOutcome, Caps, Clock, GateError, GateState, NewRun, RunError, RunId, RunMode,
        RunRecord, RunScope, RunSink, RunStatus, RunStore, RunUpdate, Runner, StoreError, Timing,
    };
    use brightdata::{BrightDataClient, TargetUrl, UnlockedPage};
    use http_guard::{Redactor, Secret};

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
                finished: 0,
                active: 0,
                refused: 2
            }
        );
    }
}
