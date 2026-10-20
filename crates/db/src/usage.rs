//! The paid-provider request budget (`provider_usage`, migration
//! `0006_jobs.sql`) and the [`app::RequestBudget`] adapter over it.
//!
//! One transaction checks the global kill switch (a missing
//! `safety_settings` row counts as on) and takes the requests with a
//! conditional upsert, so concurrent callers can never exceed the cap.

use std::sync::Arc;

use app::{BudgetError, Clock, PortFuture, RequestBudget};
use domain::{DAY_MS, TenantId};
use sqlx::Row as _;
use sqlx::postgres::PgPool;

/// The only provider metered today.
pub const PROVIDER_AMAZON: &str = "amazon";

/// What [`try_consume`] decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsumeOutcome {
    /// The requests were taken.
    Granted,
    /// The global kill switch is on (or settings are missing).
    KillSwitch,
    /// Taking them would exceed the daily cap; nothing was taken.
    CapReached,
}

/// Takes `n` Amazon requests from `tenant_id`'s budget for the UTC day
/// starting at `day_start_ms`, if the kill switch is off and the total
/// stays within `cap`.
///
/// # Errors
///
/// Database errors; redact before logging. Callers must treat an error as
/// a refusal (fail closed).
pub async fn try_consume(
    pool: &PgPool,
    tenant_id: &str,
    day_start_ms: i64,
    n: i32,
    cap: i32,
) -> Result<ConsumeOutcome, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let kill = sqlx::query("SELECT global_kill_switch FROM safety_settings WHERE id = 1 FOR SHARE")
        .fetch_optional(&mut *tx)
        .await?
        .map(|row| row.try_get::<bool, _>("global_kill_switch"))
        .transpose()?
        .unwrap_or(true);
    if kill {
        return Ok(ConsumeOutcome::KillSwitch);
    }
    // Inserts only when `n` alone fits; on conflict the WHERE runs on the
    // locked, latest row, so racing callers are serialized.
    let done = sqlx::query(
        "INSERT INTO provider_usage (tenant_id, provider, day, requests) \
         SELECT $1, $2, (to_timestamp($3 / 1000.0) AT TIME ZONE 'UTC')::date, $4 \
         WHERE $4 <= $5 \
         ON CONFLICT (tenant_id, provider, day) DO UPDATE \
           SET requests = provider_usage.requests + EXCLUDED.requests \
           WHERE provider_usage.requests + EXCLUDED.requests <= $5",
    )
    .bind(tenant_id)
    .bind(PROVIDER_AMAZON)
    .bind(day_start_ms)
    .bind(n)
    .bind(cap)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(if done.rows_affected() == 1 {
        ConsumeOutcome::Granted
    } else {
        ConsumeOutcome::CapReached
    })
}

/// Amazon requests `tenant_id` used on the UTC day starting at
/// `day_start_ms`.
///
/// # Errors
///
/// Database errors; redact before logging.
pub async fn used(pool: &PgPool, tenant_id: &str, day_start_ms: i64) -> Result<i64, sqlx::Error> {
    let row = sqlx::query(
        "SELECT requests::BIGINT AS requests FROM provider_usage \
         WHERE tenant_id = $1 AND provider = $2 \
           AND day = (to_timestamp($3 / 1000.0) AT TIME ZONE 'UTC')::date",
    )
    .bind(tenant_id)
    .bind(PROVIDER_AMAZON)
    .bind(day_start_ms)
    .fetch_optional(pool)
    .await?;
    row.map_or(Ok(0), |r| r.try_get("requests"))
}

/// Start of the UTC day containing `now_ms`.
#[must_use]
pub const fn day_start(now_ms: i64) -> i64 {
    now_ms - now_ms.rem_euclid(DAY_MS)
}

/// [`RequestBudget`] over `provider_usage`, with a daily cap per tenant.
/// Storage errors become [`BudgetError::Unavailable`] (a refusal).
#[derive(Clone)]
pub struct PgRequestBudget {
    pool: PgPool,
    clock: Arc<dyn Clock>,
    cap_per_day: u32,
}

impl PgRequestBudget {
    /// A budget of `cap_per_day` Amazon requests per tenant per UTC day.
    #[must_use]
    pub fn new(pool: PgPool, clock: Arc<dyn Clock>, cap_per_day: u32) -> Self {
        Self {
            pool,
            clock,
            cap_per_day,
        }
    }
}

impl RequestBudget for PgRequestBudget {
    fn consume<'a>(
        &'a self,
        tenant: &'a TenantId,
        n: u32,
    ) -> PortFuture<'a, Result<(), BudgetError>> {
        Box::pin(async move {
            // Amounts beyond i32 can never fit a cap; refuse them.
            let n = i32::try_from(n).map_err(|_| BudgetError::CapReached)?;
            let cap = i32::try_from(self.cap_per_day).unwrap_or(i32::MAX);
            let start = day_start(self.clock.now().0);
            match try_consume(&self.pool, tenant.as_str(), start, n, cap).await {
                Ok(ConsumeOutcome::Granted) => Ok(()),
                Ok(ConsumeOutcome::KillSwitch) => Err(BudgetError::KillSwitch),
                Ok(ConsumeOutcome::CapReached) => Err(BudgetError::CapReached),
                Err(_) => Err(BudgetError::Unavailable),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::day_start;

    #[test]
    fn day_start_truncates_to_utc_midnight() {
        assert_eq!(day_start(0), 0);
        assert_eq!(day_start(86_399_999), 0);
        assert_eq!(day_start(86_400_000), 86_400_000);
        assert_eq!(day_start(-1), -86_400_000);
    }
}
