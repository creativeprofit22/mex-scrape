//! Per-retailer Bright Data access metrics (job metrics for
//! `blocked_needs_kyc`), logged from the `app` access report use case.

use app::AccessReport;

/// Logs one line per retailer. Storage errors are logged and swallowed:
/// metrics never fail a run.
pub async fn log_access_metrics(report: &AccessReport) {
    match report.per_retailer().await {
        Ok(totals) => {
            for (retailer, total) in totals {
                tracing::info!(
                    retailer = %retailer,
                    ok = total.ok,
                    blocked_needs_kyc = total.blocked_needs_kyc,
                    skipped_robots = total.skipped_robots,
                    failed = total.failed,
                    "Bright Data access metrics"
                );
            }
        }
        Err(_) => tracing::warn!("reading Bright Data access metrics failed"),
    }
}
