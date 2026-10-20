//! Per-retailer job metrics: requests, parse results, `blocked_needs_kyc`
//! and an estimated Bright Data cost. Each job logs one
//! `mex_scrape::metrics` event per retailer, sorted by retailer id.
//!
//! The cost is an estimate: Web Unlocker is billed per fetched page; the
//! Browser API per MB of traffic, which we approximate with the returned
//! HTML size (it underestimates: scripts, images and fonts are not
//! counted).

use std::collections::BTreeMap;

use brightdata::runs::{AccessOutcome, PageAccess};

/// Bytes in one billed megabyte.
const BYTES_PER_MB: u128 = 1_000_000;

/// Bright Data price assumptions for the cost estimate, in micro-USD.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CostRates {
    /// Per fetched Web Unlocker page.
    pub unlocker_microusd_per_request: u64,
    /// Per MB of Browser API HTML.
    pub browser_microusd_per_mb: u64,
}

/// Counters for one retailer within one job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RetailerMetrics {
    /// URLs sent to Bright Data (robots.txt skips excluded).
    pub requests: u32,
    /// Fetched pages that parsed into at least one offer.
    pub parse_ok: u32,
    /// Fetched pages that did not parse (or had no offers).
    pub parse_failed: u32,
    /// URLs refused by Bright Data until KYC.
    pub blocked_needs_kyc: u32,
    /// URLs skipped by our robots.txt pre-check (no request made).
    pub skipped_robots: u32,
    /// Fetched Web Unlocker pages (billed per page).
    pub unlocker_pages: u32,
    /// HTML bytes returned by the Browser API.
    pub browser_bytes: u64,
}

impl RetailerMetrics {
    /// Estimated cost in micro-USD, rounded up.
    #[must_use]
    pub fn est_cost_microusd(&self, rates: CostRates) -> u64 {
        let unlocker =
            u128::from(self.unlocker_pages) * u128::from(rates.unlocker_microusd_per_request);
        let browser = (u128::from(self.browser_bytes) * u128::from(rates.browser_microusd_per_mb))
            .div_ceil(BYTES_PER_MB);
        u64::try_from(unlocker + browser).unwrap_or(u64::MAX)
    }
}

/// Metrics of one job, keyed by retailer id (sorted).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct JobMetrics {
    by_retailer: BTreeMap<String, RetailerMetrics>,
}

impl JobMetrics {
    fn entry(&mut self, retailer: &str) -> &mut RetailerMetrics {
        self.by_retailer.entry(retailer.to_owned()).or_default()
    }

    /// One fetched page of `bytes` HTML, and whether it parsed.
    pub fn page(&mut self, retailer: &str, access: PageAccess, bytes: usize, parsed: bool) {
        let m = self.entry(retailer);
        match access {
            PageAccess::Unlocker => m.unlocker_pages = m.unlocker_pages.saturating_add(1),
            PageAccess::Browser => {
                m.browser_bytes = m
                    .browser_bytes
                    .saturating_add(u64::try_from(bytes).unwrap_or(u64::MAX));
            }
        }
        if parsed {
            m.parse_ok = m.parse_ok.saturating_add(1);
        } else {
            m.parse_failed = m.parse_failed.saturating_add(1);
        }
    }

    /// One recorded access outcome of the job's run.
    pub fn access(&mut self, retailer: &str, outcome: AccessOutcome) {
        let m = self.entry(retailer);
        match outcome {
            AccessOutcome::SkippedRobots => {
                m.skipped_robots = m.skipped_robots.saturating_add(1);
            }
            AccessOutcome::BlockedNeedsKyc => {
                m.requests = m.requests.saturating_add(1);
                m.blocked_needs_kyc = m.blocked_needs_kyc.saturating_add(1);
            }
            AccessOutcome::Ok | AccessOutcome::Failed => {
                m.requests = m.requests.saturating_add(1);
            }
        }
    }

    /// Adds another job part's counters (e.g. a second run).
    pub fn merge(&mut self, other: &Self) {
        for (retailer, o) in &other.by_retailer {
            let m = self.entry(retailer);
            m.requests = m.requests.saturating_add(o.requests);
            m.parse_ok = m.parse_ok.saturating_add(o.parse_ok);
            m.parse_failed = m.parse_failed.saturating_add(o.parse_failed);
            m.blocked_needs_kyc = m.blocked_needs_kyc.saturating_add(o.blocked_needs_kyc);
            m.skipped_robots = m.skipped_robots.saturating_add(o.skipped_robots);
            m.unlocker_pages = m.unlocker_pages.saturating_add(o.unlocker_pages);
            m.browser_bytes = m.browser_bytes.saturating_add(o.browser_bytes);
        }
    }

    /// The counters of `retailer` (zero when untouched).
    #[must_use]
    pub fn get(&self, retailer: &str) -> RetailerMetrics {
        self.by_retailer.get(retailer).copied().unwrap_or_default()
    }

    /// Every retailer's counters, sorted by retailer id.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &RetailerMetrics)> {
        self.by_retailer.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Logs one `mex_scrape::metrics` event per retailer.
    pub fn emit(&self, job: &str, msg_id: i64, tenant: &str, rates: CostRates) {
        for (retailer, m) in self.iter() {
            tracing::info!(
                target: "mex_scrape::metrics",
                job,
                msg_id,
                tenant,
                retailer,
                requests = m.requests,
                parse_ok = m.parse_ok,
                parse_failed = m.parse_failed,
                blocked_needs_kyc = m.blocked_needs_kyc,
                skipped_robots = m.skipped_robots,
                est_cost_microusd = m.est_cost_microusd(rates),
                "job retailer metrics"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATES: CostRates = CostRates {
        unlocker_microusd_per_request: 1500,
        browser_microusd_per_mb: 8000,
    };

    #[test]
    fn counts_and_cost_per_retailer() {
        let mut m = JobMetrics::default();
        m.page("walmart_mx", PageAccess::Unlocker, 10, true);
        m.page("walmart_mx", PageAccess::Unlocker, 10, false);
        m.access("walmart_mx", AccessOutcome::Ok);
        m.access("walmart_mx", AccessOutcome::Ok);
        m.access("walmart_mx", AccessOutcome::BlockedNeedsKyc);
        m.access("walmart_mx", AccessOutcome::SkippedRobots);
        m.page("sears_mx", PageAccess::Browser, 1_500_000, true);
        m.access("sears_mx", AccessOutcome::Ok);
        let w = m.get("walmart_mx");
        assert_eq!(
            (w.requests, w.parse_ok, w.parse_failed, w.blocked_needs_kyc),
            (3, 1, 1, 1)
        );
        assert_eq!(w.skipped_robots, 1);
        assert_eq!(w.est_cost_microusd(RATES), 3000);
        // 1.5 MB at 8000 µUSD/MB.
        assert_eq!(m.get("sears_mx").est_cost_microusd(RATES), 12_000);
        let order: Vec<_> = m.iter().map(|(r, _)| r).collect();
        assert_eq!(order, ["sears_mx", "walmart_mx"]);
    }

    #[test]
    fn browser_cost_rounds_up_and_merge_adds() {
        let mut a = JobMetrics::default();
        a.page("sanborns", PageAccess::Browser, 1, true);
        assert_eq!(a.get("sanborns").est_cost_microusd(RATES), 1);
        let mut b = JobMetrics::default();
        b.page("sanborns", PageAccess::Browser, 999_999, false);
        a.merge(&b);
        let s = a.get("sanborns");
        assert_eq!(
            (s.browser_bytes, s.parse_ok, s.parse_failed),
            (1_000_000, 1, 1)
        );
        assert_eq!(s.est_cost_microusd(RATES), 8000);
        assert_eq!(a.get("unknown"), RetailerMetrics::default());
    }
}
