//! Per-retailer Bright Data access metrics (job metrics for
//! `blocked_needs_kyc`), derived from the stored per-host access events.

use std::collections::BTreeMap;

use db::PgPool;
use db::brightdata_runs::{HostAccessCounts, access_counts_by_host};

/// Totals for one retailer; hosts outside the registry group under their
/// host name.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct RetailerAccess {
    pub(crate) ok: i64,
    pub(crate) blocked_needs_kyc: i64,
    pub(crate) skipped_robots: i64,
    pub(crate) failed: i64,
}

/// Sums host counts per retailer id, sorted by id.
pub(crate) fn per_retailer(counts: &[HostAccessCounts]) -> BTreeMap<String, RetailerAccess> {
    let mut totals: BTreeMap<String, RetailerAccess> = BTreeMap::new();
    for count in counts {
        let key = retailers::retailer_for_host(&count.host)
            .map_or_else(|| count.host.clone(), |retailer| retailer.id.to_owned());
        let entry = totals.entry(key).or_default();
        entry.ok += count.ok;
        entry.blocked_needs_kyc += count.blocked_needs_kyc;
        entry.skipped_robots += count.skipped_robots;
        entry.failed += count.failed;
    }
    totals
}

/// Logs one line per retailer. Storage errors are logged and swallowed:
/// metrics never fail a run.
pub(crate) async fn log_access_metrics(pool: &PgPool) {
    match access_counts_by_host(pool).await {
        Ok(counts) => {
            for (retailer, total) in per_retailer(&counts) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn host(name: &str, ok: i64, blocked: i64) -> HostAccessCounts {
        HostAccessCounts {
            host: name.to_owned(),
            ok,
            blocked_needs_kyc: blocked,
            skipped_robots: 1,
            failed: 0,
        }
    }

    #[test]
    fn groups_registered_hosts_by_retailer_and_keeps_unknown_hosts() {
        let totals = per_retailer(&[
            host("www.liverpool.com.mx", 1, 2),
            host("WWW.LIVERPOOL.COM.MX", 0, 1),
            host("other.example", 5, 0),
        ]);
        assert_eq!(totals["liverpool"].blocked_needs_kyc, 3);
        assert_eq!(totals["liverpool"].skipped_robots, 2);
        assert_eq!(totals["other.example"].ok, 5);
    }
}
