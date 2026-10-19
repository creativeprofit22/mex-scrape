//! [`app::AccessCountsRepo`] over Postgres (`brightdata_access_events`).

use app::{AccessCountsRepo, AccessTotals, HostAccess, PortFuture, RepoError};
use sqlx::postgres::PgPool;

use crate::brightdata_runs::access_counts_by_host;

/// Per-host access counts read from Postgres. Errors become
/// [`RepoError::Unavailable`] without detail; callers log a fixed message.
#[derive(Clone)]
pub struct PgAccessCounts(pub PgPool);

impl AccessCountsRepo for PgAccessCounts {
    fn counts_by_host(&self) -> PortFuture<'_, Result<Vec<HostAccess>, RepoError>> {
        Box::pin(async move {
            let rows = access_counts_by_host(&self.0)
                .await
                .map_err(|_| RepoError::Unavailable)?;
            Ok(rows
                .into_iter()
                .map(|row| HostAccess {
                    host: row.host,
                    totals: AccessTotals {
                        ok: row.ok,
                        blocked_needs_kyc: row.blocked_needs_kyc,
                        skipped_robots: row.skipped_robots,
                        failed: row.failed,
                    },
                })
                .collect())
        })
    }
}
