//! Start-up recovery of interrupted runs (ported from linkgo
//! `brightdata/recovery.rs`).
//!
//! At start-up no run is being driven, so every active run was interrupted.
//! Web Unlocker runs cannot be resumed and fail. Dataset runs with a
//! snapshot id stay resumable for 24 hours (Bright Data keeps the snapshot
//! server-side); older ones, and ones without a snapshot id, fail.

use crate::runs::{Clock, RunId, RunMode, RunStatus, RunStore, RunUpdate, StoreError};

/// Stored on interrupted Web Unlocker runs.
pub const INTERRUPTED_ERROR: &str = "Interrupted when the worker stopped";
/// Stored on dataset runs interrupted before Bright Data accepted them.
pub const NO_SNAPSHOT_ERROR: &str = "Interrupted before Bright Data accepted the request";
/// Stored on dataset runs older than the resume window.
pub const EXPIRED_ERROR: &str = "Bright Data results were not collected within 24 hours";
/// How long a dataset run with a snapshot id stays resumable.
pub const RESUME_WINDOW_MS: i64 = 24 * 60 * 60 * 1000;

/// What to do with one interrupted run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryAction {
    /// Leave it active; it can be resumed by snapshot id.
    Keep,
    /// Mark it failed with this message.
    Fail(&'static str),
}

/// Pure decision for one active run that nothing is driving. The 24 h
/// boundary is inclusive.
#[must_use]
pub fn recovery_action(
    mode: RunMode,
    has_snapshot: bool,
    updated_at_ms: i64,
    now_ms: i64,
) -> RecoveryAction {
    if mode != RunMode::Dataset {
        return RecoveryAction::Fail(INTERRUPTED_ERROR);
    }
    if !has_snapshot {
        return RecoveryAction::Fail(NO_SNAPSHOT_ERROR);
    }
    if now_ms.saturating_sub(updated_at_ms) <= RESUME_WINDOW_MS {
        RecoveryAction::Keep
    } else {
        RecoveryAction::Fail(EXPIRED_ERROR)
    }
}

/// Result of one recovery pass.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RecoveryReport {
    /// Runs marked failed.
    pub failed: u32,
    /// Dataset runs left active, oldest first. The caller must resume them
    /// with [`crate::runs::Runner::resume_run`] (or fail them): while active
    /// they block new runs for their scope.
    pub kept: Vec<RunId>,
}

/// Applies [`recovery_action`] to every active run. Call only when no run is
/// being driven (the worker holds a driver lock), or live runs would fail.
///
/// # Errors
///
/// [`StoreError`] when storage fails; runs already handled stay handled.
pub async fn recover_interrupted(
    store: &impl RunStore,
    clock: &impl Clock,
) -> Result<RecoveryReport, StoreError> {
    let now_ms = clock.now_ms();
    let mut report = RecoveryReport::default();
    for run in store.active_runs().await? {
        let action = recovery_action(
            run.mode,
            run.snapshot_id.is_some(),
            run.updated_at_ms,
            now_ms,
        );
        match action {
            RecoveryAction::Keep => report.kept.push(run.id),
            RecoveryAction::Fail(message) => {
                let update = RunUpdate::status(RunStatus::Failed).message(message);
                if store.update_if_active(run.id, &update, now_ms).await? {
                    report.failed += 1;
                }
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::{
        EXPIRED_ERROR, INTERRUPTED_ERROR, NO_SNAPSHOT_ERROR, RecoveryAction, RecoveryReport,
        recover_interrupted, recovery_action,
    };
    use crate::runs::tests::{FixedClock, MemoryStore, scope};
    use crate::runs::{RunMode, RunStatus};

    const NOW: i64 = 1_790_000_000_000;
    const HOUR: i64 = 60 * 60 * 1000;

    #[test]
    fn decision_matrix() {
        use RunMode::{Dataset, Unlocker};
        let cases = [
            (Unlocker, true, NOW, RecoveryAction::Fail(INTERRUPTED_ERROR)),
            (
                Unlocker,
                false,
                NOW,
                RecoveryAction::Fail(INTERRUPTED_ERROR),
            ),
            (Dataset, false, NOW, RecoveryAction::Fail(NO_SNAPSHOT_ERROR)),
            (Dataset, true, NOW - HOUR, RecoveryAction::Keep),
            (Dataset, true, NOW - 24 * HOUR, RecoveryAction::Keep),
            (
                Dataset,
                true,
                NOW - 24 * HOUR - 1,
                RecoveryAction::Fail(EXPIRED_ERROR),
            ),
            (Dataset, true, i64::MIN, RecoveryAction::Fail(EXPIRED_ERROR)),
        ];
        for (mode, has_snapshot, updated, expected) in cases {
            assert_eq!(
                recovery_action(mode, has_snapshot, updated, NOW),
                expected,
                "{mode:?} snapshot={has_snapshot} updated={updated}"
            );
        }
    }

    #[tokio::test]
    async fn recovers_active_runs_only() {
        let store = MemoryStore::enabled();
        let unlocker = store.seed(
            &scope("a"),
            RunMode::Unlocker,
            None,
            RunStatus::Running,
            NOW,
        );
        let fresh = store.seed(
            &scope("b"),
            RunMode::Dataset,
            Some("sd_fresh"),
            RunStatus::Running,
            NOW - HOUR,
        );
        let stale = store.seed(
            &scope("c"),
            RunMode::Dataset,
            Some("sd_stale"),
            RunStatus::Ready,
            NOW - 25 * HOUR,
        );
        let no_snapshot = store.seed(
            &scope("d"),
            RunMode::Dataset,
            None,
            RunStatus::Starting,
            NOW,
        );
        let done = store.seed(
            &scope("e"),
            RunMode::Unlocker,
            None,
            RunStatus::Imported,
            NOW,
        );

        let report = recover_interrupted(&store, &FixedClock(NOW)).await.unwrap();
        assert_eq!(
            report,
            RecoveryReport {
                failed: 3,
                kept: vec![fresh]
            }
        );
        let status = |id| store.run(id).status;
        assert_eq!(status(unlocker), RunStatus::Failed);
        assert_eq!(store.run(unlocker).error_message, INTERRUPTED_ERROR);
        assert_eq!(status(fresh), RunStatus::Running);
        assert_eq!(status(stale), RunStatus::Failed);
        assert_eq!(store.run(stale).error_message, EXPIRED_ERROR);
        assert_eq!(status(no_snapshot), RunStatus::Failed);
        assert_eq!(status(done), RunStatus::Imported);
    }
}
