//! Queries for `safety_settings` and `brightdata_runs` (migration
//! `0002_brightdata_runs.sql`).
//!
//! Plain row types only: the `brightdata` crate's types are mapped in
//! `worker`, so `db` does not depend on an adapter. Timestamps cross the
//! boundary as epoch milliseconds (`to_timestamp(ms / 1000.0)` in,
//! `EXTRACT(EPOCH ...) * 1000` out) and JSON as text cast to `jsonb`.

use sqlx::postgres::{PgConnection, PgPool, PgRow};
use sqlx::{Connection as _, Row as _};

/// Session advisory-lock key held by the worker that drives Bright Data
/// runs. Above the `int4` range, so it never collides with the per-tenant
/// `hashtext` transaction locks.
const DRIVER_LOCK_KEY: i64 = 0x6272_6461_7461_7275;

/// Columns selected for a [`RunRow`].
macro_rules! run_columns {
    () => {
        "id, tenant_id, job, mode, input_json::text AS input_json, snapshot_id, status, \
         requested_count, row_count, failed_count, error_message, \
         (EXTRACT(EPOCH FROM created_at) * 1000)::BIGINT AS created_at_ms, \
         (EXTRACT(EPOCH FROM updated_at) * 1000)::BIGINT AS updated_at_ms"
    };
}

/// The `safety_settings` row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SafetySettings {
    /// Stops every Bright Data run.
    pub global_kill_switch: bool,
    /// The Bright Data connector may start runs.
    pub brightdata_enabled: bool,
}

/// Today's counts for a tenant and whether the scope has an active run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GateCounts {
    /// Runs the tenant created since the day start.
    pub runs_today: i64,
    /// Items the tenant requested since the day start.
    pub items_today: i64,
    /// The (tenant, job) scope has a `starting`/`running`/`ready` run.
    pub active_run: bool,
}

/// Per-tenant caps re-checked inside [`insert_run_capped`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunCaps {
    /// Runs per tenant per day.
    pub runs_per_day: i64,
    /// Items per tenant per day.
    pub items_per_day: i64,
    /// Items per run.
    pub items_per_run: i64,
}

/// A run to insert (status `starting`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewRunRow<'a> {
    /// Tenant.
    pub tenant_id: &'a str,
    /// Job name.
    pub job: &'a str,
    /// `unlocker` or `dataset`.
    pub mode: &'a str,
    /// Input as JSON text.
    pub input_json: &'a str,
    /// Items requested.
    pub requested_count: i32,
    /// Creation time (epoch ms).
    pub now_ms: i64,
}

/// Why [`insert_run_capped`] refused or failed.
#[derive(Debug, thiserror::Error)]
pub enum InsertRunError {
    /// The scope already has an active run.
    #[error("a run is already active for this scope")]
    ActiveRun,
    /// The tenant's daily run cap is reached.
    #[error("daily run cap reached")]
    RunsPerDayCap,
    /// Too many items for one run.
    #[error("too many items for one run")]
    ItemsPerRunCap,
    /// The tenant's daily item cap would be exceeded.
    #[error("daily item cap reached")]
    ItemsPerDayCap,
    /// Database error; redact before logging.
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// Fields to change; `None` keeps the column.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunPatch<'a> {
    /// New status.
    pub status: Option<&'a str>,
    /// Snapshot id.
    pub snapshot_id: Option<&'a str>,
    /// Imported items.
    pub row_count: Option<i32>,
    /// Failed items.
    pub failed_count: Option<i32>,
    /// Error message (≤ 1000 characters).
    pub error_message: Option<&'a str>,
}

/// A `brightdata_runs` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRow {
    /// Id.
    pub id: i64,
    /// Tenant.
    pub tenant_id: String,
    /// Job name.
    pub job: String,
    /// `unlocker` or `dataset`.
    pub mode: String,
    /// Input as JSON text.
    pub input_json: String,
    /// Snapshot id.
    pub snapshot_id: Option<String>,
    /// Status.
    pub status: String,
    /// Items requested.
    pub requested_count: i32,
    /// Items imported.
    pub row_count: i32,
    /// Items failed.
    pub failed_count: i32,
    /// Last error ('' when none).
    pub error_message: String,
    /// Creation time (epoch ms).
    pub created_at_ms: i64,
    /// Last update (epoch ms).
    pub updated_at_ms: i64,
}

fn run_row(row: &PgRow) -> Result<RunRow, sqlx::Error> {
    Ok(RunRow {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        job: row.try_get("job")?,
        mode: row.try_get("mode")?,
        input_json: row.try_get("input_json")?,
        snapshot_id: row.try_get("snapshot_id")?,
        status: row.try_get("status")?,
        requested_count: row.try_get("requested_count")?,
        row_count: row.try_get("row_count")?,
        failed_count: row.try_get("failed_count")?,
        error_message: row.try_get("error_message")?,
        created_at_ms: row.try_get("created_at_ms")?,
        updated_at_ms: row.try_get("updated_at_ms")?,
    })
}

/// Reads the settings row; `None` when it is missing (callers fail closed).
///
/// # Errors
///
/// Database errors; redact before logging.
pub async fn safety_settings(pool: &PgPool) -> Result<Option<SafetySettings>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT global_kill_switch, brightdata_enabled FROM safety_settings WHERE id = 1",
    )
    .fetch_optional(pool)
    .await?;
    row.map(|row| {
        Ok(SafetySettings {
            global_kill_switch: row.try_get("global_kill_switch")?,
            brightdata_enabled: row.try_get("brightdata_enabled")?,
        })
    })
    .transpose()
}

/// Sets the settings row (operator action; also used by tests).
///
/// # Errors
///
/// Database errors; redact before logging.
pub async fn set_safety_settings(
    pool: &PgPool,
    settings: SafetySettings,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO safety_settings (id, global_kill_switch, brightdata_enabled, updated_at) \
         VALUES (1, $1, $2, now()) \
         ON CONFLICT (id) DO UPDATE SET global_kill_switch = $1, brightdata_enabled = $2, \
         updated_at = now()",
    )
    .bind(settings.global_kill_switch)
    .bind(settings.brightdata_enabled)
    .execute(pool)
    .await?;
    Ok(())
}

/// Counts the tenant's runs and items since `day_start_ms` and checks for
/// an active run in the scope.
///
/// # Errors
///
/// Database errors; redact before logging.
pub async fn gate_counts(
    pool: &PgPool,
    tenant_id: &str,
    job: &str,
    day_start_ms: i64,
) -> Result<GateCounts, sqlx::Error> {
    let mut connection = pool.acquire().await?;
    gate_counts_on(&mut connection, tenant_id, job, day_start_ms).await
}

async fn gate_counts_on(
    connection: &mut PgConnection,
    tenant_id: &str,
    job: &str,
    day_start_ms: i64,
) -> Result<GateCounts, sqlx::Error> {
    let row = sqlx::query(
        "SELECT \
           count(*) FILTER (WHERE created_at >= to_timestamp($3 / 1000.0)) AS runs_today, \
           COALESCE(sum(requested_count) FILTER (WHERE created_at >= to_timestamp($3 / 1000.0)), 0)::BIGINT \
             AS items_today, \
           bool_or(job = $2 AND status IN ('starting', 'running', 'ready')) AS active_run \
         FROM brightdata_runs WHERE tenant_id = $1",
    )
    .bind(tenant_id)
    .bind(job)
    .bind(day_start_ms)
    .fetch_one(connection)
    .await?;
    Ok(GateCounts {
        runs_today: row.try_get("runs_today")?,
        items_today: row.try_get("items_today")?,
        active_run: row
            .try_get::<Option<bool>, _>("active_run")?
            .unwrap_or(false),
    })
}

/// Inserts a `starting` run in one transaction that serializes the tenant
/// (`pg_advisory_xact_lock`), re-counts the caps and relies on the
/// `brightdata_runs_one_active` index for the one-active-run rule. Checks
/// run in gate order: runs/day, items/run, items/day, active run.
///
/// # Errors
///
/// [`InsertRunError`].
pub async fn insert_run_capped(
    pool: &PgPool,
    run: &NewRunRow<'_>,
    day_start_ms: i64,
    caps: &RunCaps,
) -> Result<i64, InsertRunError> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
        .bind(run.tenant_id)
        .execute(&mut *tx)
        .await?;
    let counts = gate_counts_on(&mut tx, run.tenant_id, run.job, day_start_ms).await?;
    let requested = i64::from(run.requested_count);
    if counts.runs_today >= caps.runs_per_day {
        return Err(InsertRunError::RunsPerDayCap);
    }
    if requested > caps.items_per_run {
        return Err(InsertRunError::ItemsPerRunCap);
    }
    if counts.items_today.saturating_add(requested) > caps.items_per_day {
        return Err(InsertRunError::ItemsPerDayCap);
    }
    let inserted = sqlx::query(
        "INSERT INTO brightdata_runs \
           (tenant_id, job, mode, input_json, requested_count, created_at, updated_at) \
         VALUES ($1, $2, $3, $4::jsonb, $5, to_timestamp($6 / 1000.0), to_timestamp($6 / 1000.0)) \
         RETURNING id",
    )
    .bind(run.tenant_id)
    .bind(run.job)
    .bind(run.mode)
    .bind(run.input_json)
    .bind(run.requested_count)
    .bind(run.now_ms)
    .fetch_one(&mut *tx)
    .await;
    let row = match inserted {
        Ok(row) => row,
        Err(error) if is_active_conflict(&error) => return Err(InsertRunError::ActiveRun),
        Err(error) => return Err(error.into()),
    };
    let id = row.try_get("id")?;
    tx.commit().await?;
    Ok(id)
}

fn is_active_conflict(error: &sqlx::Error) -> bool {
    error.as_database_error().is_some_and(|database| {
        database.is_unique_violation()
            && database.constraint() == Some("brightdata_runs_one_active")
    })
}

/// Applies `patch` only while the run is active; returns whether a row
/// changed. A terminal status (e.g. a cancel from elsewhere) is never
/// overwritten.
///
/// # Errors
///
/// Database errors; redact before logging.
pub async fn update_if_active(
    pool: &PgPool,
    id: i64,
    patch: &RunPatch<'_>,
    now_ms: i64,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE brightdata_runs SET \
           status = COALESCE($2, status), \
           snapshot_id = COALESCE($3, snapshot_id), \
           row_count = COALESCE($4, row_count), \
           failed_count = COALESCE($5, failed_count), \
           error_message = COALESCE($6, error_message), \
           updated_at = to_timestamp($7 / 1000.0) \
         WHERE id = $1 AND status IN ('starting', 'running', 'ready')",
    )
    .bind(id)
    .bind(patch.status)
    .bind(patch.snapshot_id)
    .bind(patch.row_count)
    .bind(patch.failed_count)
    .bind(patch.error_message)
    .bind(now_ms)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Loads one run.
///
/// # Errors
///
/// Database errors; redact before logging.
pub async fn load_run(pool: &PgPool, id: i64) -> Result<Option<RunRow>, sqlx::Error> {
    let row = sqlx::query(concat!(
        "SELECT ",
        run_columns!(),
        " FROM brightdata_runs WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await?;
    row.as_ref().map(run_row).transpose()
}

/// Every active run, oldest first.
///
/// # Errors
///
/// Database errors; redact before logging.
pub async fn active_runs(pool: &PgPool) -> Result<Vec<RunRow>, sqlx::Error> {
    let rows = sqlx::query(concat!(
        "SELECT ",
        run_columns!(),
        " FROM brightdata_runs WHERE status IN ('starting', 'running', 'ready') ORDER BY id"
    ))
    .fetch_all(pool)
    .await?;
    rows.iter().map(run_row).collect()
}

/// Access outcomes for one host, summed over all events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostAccessCounts {
    /// The target host.
    pub host: String,
    /// Pages fetched.
    pub ok: i64,
    /// Refusals that need KYC.
    pub blocked_needs_kyc: i64,
    /// URLs skipped because robots.txt disallows them.
    pub skipped_robots: i64,
    /// Other failures.
    pub failed: i64,
}

/// Records one URL access outcome. `outcome` must be one of `ok`,
/// `blocked_needs_kyc`, `skipped_robots`, `failed` (the table CHECK
/// rejects anything else).
///
/// # Errors
///
/// Database errors; redact before logging.
pub async fn insert_access_event(
    pool: &PgPool,
    run_id: i64,
    host: &str,
    outcome: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO brightdata_access_events (run_id, host, outcome) VALUES ($1, $2, $3)")
        .bind(run_id)
        .bind(host)
        .bind(outcome)
        .execute(pool)
        .await?;
    Ok(())
}

/// `(host, outcome)` of every access event of one run, in insertion
/// order.
///
/// # Errors
///
/// Database errors; redact before logging.
pub async fn access_outcomes_for_run(
    pool: &PgPool,
    run_id: i64,
) -> Result<Vec<(String, String)>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT host, outcome FROM brightdata_access_events WHERE run_id = $1 ORDER BY id",
    )
    .bind(run_id)
    .fetch_all(pool)
    .await?;
    rows.iter()
        .map(|row| Ok((row.try_get("host")?, row.try_get("outcome")?)))
        .collect()
}

/// Outcome counts per host, sorted by host.
///
/// # Errors
///
/// Database errors; redact before logging.
pub async fn access_counts_by_host(pool: &PgPool) -> Result<Vec<HostAccessCounts>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT host,             COUNT(*) FILTER (WHERE outcome = 'ok') AS ok,             COUNT(*) FILTER (WHERE outcome = 'blocked_needs_kyc') AS blocked,             COUNT(*) FILTER (WHERE outcome = 'skipped_robots') AS skipped,             COUNT(*) FILTER (WHERE outcome = 'failed') AS failed          FROM brightdata_access_events GROUP BY host ORDER BY host",
    )
    .fetch_all(pool)
    .await?;
    rows.iter()
        .map(|row| {
            Ok(HostAccessCounts {
                host: row.try_get("host")?,
                ok: row.try_get("ok")?,
                blocked_needs_kyc: row.try_get("blocked")?,
                skipped_robots: row.try_get("skipped")?,
                failed: row.try_get("failed")?,
            })
        })
        .collect()
}

/// A held driver lock: a dedicated connection holding a session advisory
/// lock. Dropping it closes the connection, which releases the lock.
pub struct DriverLock {
    connection: PgConnection,
}

impl std::fmt::Debug for DriverLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DriverLock").finish_non_exhaustive()
    }
}

impl DriverLock {
    /// Releases the lock by closing its connection.
    ///
    /// # Errors
    ///
    /// Database errors while closing; redact before logging.
    pub async fn release(self) -> Result<(), sqlx::Error> {
        self.connection.close().await
    }
}

/// Tries to become the one process that drives Bright Data runs (and may
/// run start-up recovery). `None` when another process holds the lock.
///
/// # Errors
///
/// Database errors; redact before logging.
pub async fn try_driver_lock(pool: &PgPool) -> Result<Option<DriverLock>, sqlx::Error> {
    let mut connection = pool.acquire().await?.detach();
    let locked: bool = sqlx::query("SELECT pg_try_advisory_lock($1) AS locked")
        .bind(DRIVER_LOCK_KEY)
        .fetch_one(&mut connection)
        .await?
        .try_get("locked")?;
    if locked {
        Ok(Some(DriverLock { connection }))
    } else {
        connection.close().await?;
        Ok(None)
    }
}
