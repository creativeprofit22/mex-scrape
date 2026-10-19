//! Postgres-backed `brightdata::runs::RunStore`, bridging the `brightdata`
//! run model to `db::brightdata_runs` rows.

use brightdata::SnapshotId;
use brightdata::runs::{
    Caps, GateError, GateState, NewRun, RunId, RunMode, RunRecord, RunScope, RunStatus, RunStore,
    RunUpdate, StoreError,
};
use db::PgPool;
use db::brightdata_runs::{self as rows, InsertRunError, NewRunRow, RunCaps, RunPatch, RunRow};
use http_guard::Redactor;

/// Fixed text returned for any storage failure; the redacted detail is
/// logged.
const STORAGE_ERROR: &str = "Bright Data run storage failed";
/// A stored row that does not parse back into the run model.
const INVALID_ROW: &str = "A stored Bright Data run is invalid";

/// [`RunStore`] over the `brightdata_runs` and `safety_settings` tables.
pub(crate) struct PgRunStore {
    pool: PgPool,
    redactor: Redactor,
}

impl PgRunStore {
    /// `redactor` must cover the database URL (`DatabaseUrl::redactor`).
    pub(crate) fn new(pool: PgPool, redactor: Redactor) -> Self {
        Self { pool, redactor }
    }

    /// Logs the redacted database error and returns fixed text.
    fn storage(&self, error: &impl std::fmt::Display) -> StoreError {
        let detail = self.redactor.sanitize(&error.to_string());
        tracing::warn!(error = %detail, "Bright Data run storage failed");
        StoreError::Storage(STORAGE_ERROR.to_owned())
    }
}

fn to_i32(value: u32) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

fn to_u32(value: i64) -> u32 {
    u32::try_from(value.max(0)).unwrap_or(u32::MAX)
}

fn record(row: RunRow) -> Result<RunRecord, StoreError> {
    let invalid = || StoreError::Storage(INVALID_ROW.to_owned());
    let scope = RunScope::new(&row.tenant_id, &row.job).map_err(|_| invalid())?;
    Ok(RunRecord {
        id: RunId(row.id),
        scope,
        mode: RunMode::parse(&row.mode).ok_or_else(invalid)?,
        input_json: row.input_json,
        snapshot_id: row.snapshot_id,
        status: RunStatus::parse(&row.status).ok_or_else(invalid)?,
        requested_count: to_u32(i64::from(row.requested_count)),
        row_count: to_u32(i64::from(row.row_count)),
        failed_count: to_u32(i64::from(row.failed_count)),
        error_message: row.error_message,
        created_at_ms: row.created_at_ms,
        updated_at_ms: row.updated_at_ms,
    })
}

impl RunStore for PgRunStore {
    async fn gate_state(
        &self,
        scope: &RunScope,
        day_start_ms: i64,
    ) -> Result<GateState, StoreError> {
        let settings = rows::safety_settings(&self.pool)
            .await
            .map_err(|error| self.storage(&error))?;
        let counts = rows::gate_counts(&self.pool, scope.tenant_id(), scope.job(), day_start_ms)
            .await
            .map_err(|error| self.storage(&error))?;
        // A missing settings row fails closed.
        let (kill_switch_on, connector_enabled) = settings.map_or((true, false), |settings| {
            (settings.global_kill_switch, settings.brightdata_enabled)
        });
        Ok(GateState {
            kill_switch_on,
            connector_enabled,
            runs_today: to_u32(counts.runs_today),
            items_today: to_u32(counts.items_today),
            active_run: counts.active_run,
        })
    }

    async fn insert_run_capped(
        &self,
        run: &NewRun,
        day_start_ms: i64,
        caps: &Caps,
    ) -> Result<RunId, StoreError> {
        let requested_count = i32::try_from(run.requested_count)
            .map_err(|_| StoreError::CapExceeded(GateError::ItemsPerRunCap))?;
        let row = NewRunRow {
            tenant_id: run.scope.tenant_id(),
            job: run.scope.job(),
            mode: run.mode.as_str(),
            input_json: &run.input_json,
            requested_count,
            now_ms: run.now_ms,
        };
        let caps = RunCaps {
            runs_per_day: i64::from(caps.max_runs_per_tenant_per_day),
            items_per_day: i64::from(caps.max_items_per_tenant_per_day),
            items_per_run: i64::from(caps.max_items_per_run),
        };
        match rows::insert_run_capped(&self.pool, &row, day_start_ms, &caps).await {
            Ok(id) => Ok(RunId(id)),
            Err(InsertRunError::ActiveRun) => Err(StoreError::ActiveRun),
            Err(InsertRunError::RunsPerDayCap) => {
                Err(StoreError::CapExceeded(GateError::RunsPerDayCap))
            }
            Err(InsertRunError::ItemsPerRunCap) => {
                Err(StoreError::CapExceeded(GateError::ItemsPerRunCap))
            }
            Err(InsertRunError::ItemsPerDayCap) => {
                Err(StoreError::CapExceeded(GateError::ItemsPerDayCap))
            }
            Err(InsertRunError::Database(error)) => Err(self.storage(&error)),
        }
    }

    async fn update_if_active(
        &self,
        id: RunId,
        update: &RunUpdate,
        now_ms: i64,
    ) -> Result<bool, StoreError> {
        let patch = RunPatch {
            status: update.status.map(RunStatus::as_str),
            snapshot_id: update.snapshot_id.as_ref().map(SnapshotId::as_str),
            row_count: update.row_count.map(to_i32),
            failed_count: update.failed_count.map(to_i32),
            error_message: update.error_message.as_deref(),
        };
        rows::update_if_active(&self.pool, id.0, &patch, now_ms)
            .await
            .map_err(|error| self.storage(&error))
    }

    async fn load_run(&self, id: RunId) -> Result<Option<RunRecord>, StoreError> {
        rows::load_run(&self.pool, id.0)
            .await
            .map_err(|error| self.storage(&error))?
            .map(record)
            .transpose()
    }

    async fn active_runs(&self) -> Result<Vec<RunRecord>, StoreError> {
        rows::active_runs(&self.pool)
            .await
            .map_err(|error| self.storage(&error))?
            .into_iter()
            .map(record)
            .collect()
    }

    async fn kill_switch_on(&self) -> Result<bool, StoreError> {
        let settings = rows::safety_settings(&self.pool)
            .await
            .map_err(|error| self.storage(&error))?;
        Ok(settings.is_none_or(|settings| settings.global_kill_switch))
    }
}

#[cfg(test)]
mod tests {
    use super::{INVALID_ROW, record, to_i32, to_u32};
    use brightdata::runs::{RunMode, RunStatus, StoreError};
    use db::brightdata_runs::RunRow;

    fn row() -> RunRow {
        RunRow {
            id: 7,
            tenant_id: "tenant-1".to_owned(),
            job: "amazon".to_owned(),
            mode: "dataset".to_owned(),
            input_json: "{}".to_owned(),
            snapshot_id: Some("sd_1".to_owned()),
            status: "running".to_owned(),
            requested_count: 2,
            row_count: 0,
            failed_count: 0,
            error_message: String::new(),
            created_at_ms: 1,
            updated_at_ms: 2,
        }
    }

    #[test]
    fn rows_map_to_records_or_fail_closed() {
        let run = record(row()).unwrap();
        assert_eq!(run.mode, RunMode::Dataset);
        assert_eq!(run.status, RunStatus::Running);
        assert_eq!(run.requested_count, 2);
        for broken in [
            RunRow {
                status: "paused".to_owned(),
                ..row()
            },
            RunRow {
                mode: "browser".to_owned(),
                ..row()
            },
            RunRow {
                tenant_id: String::new(),
                ..row()
            },
        ] {
            assert_eq!(
                record(broken),
                Err(StoreError::Storage(INVALID_ROW.to_owned()))
            );
        }
    }

    #[test]
    fn counts_saturate() {
        assert_eq!(to_i32(u32::MAX), i32::MAX);
        assert_eq!(to_u32(-5), 0);
        assert_eq!(to_u32(i64::MAX), u32::MAX);
    }
}
