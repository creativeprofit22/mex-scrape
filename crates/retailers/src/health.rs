//! Per-adapter parse success rate with a logged alert when it drops below a
//! threshold. State is owned by the caller (no globals).

use std::collections::{BTreeMap, VecDeque};

use domain::Offer;

use crate::adapter::{Page, ParseError, ParseKind, Retailer};

/// Env var for [`HealthConfig::threshold`].
pub const THRESHOLD_ENV: &str = "RETAILER_PARSE_ALERT_THRESHOLD";
/// Env var for [`HealthConfig::min_samples`].
pub const MIN_SAMPLES_ENV: &str = "RETAILER_PARSE_ALERT_MIN_SAMPLES";

/// Alert settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HealthConfig {
    /// Alert when the success rate falls below this (0.0..=1.0).
    pub threshold: f64,
    /// No alert until this many results are in the window.
    pub min_samples: usize,
    /// Rolling window size (most recent results per adapter).
    pub window: usize,
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            threshold: 0.8,
            min_samples: 20,
            window: 100,
        }
    }
}

/// A malformed health setting (names the variable, never echoes the value).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0} is invalid")]
pub struct HealthConfigError(pub &'static str);

impl HealthConfig {
    /// Reads [`THRESHOLD_ENV`] (a number in 0..=1) and [`MIN_SAMPLES_ENV`]
    /// (1..=window) through `lookup`; unset values keep their defaults.
    ///
    /// # Errors
    /// [`HealthConfigError`] naming the malformed variable.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, HealthConfigError> {
        let mut cfg = Self::default();
        if let Some(raw) = lookup(THRESHOLD_ENV) {
            cfg.threshold = raw
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|t| (0.0..=1.0).contains(t))
                .ok_or(HealthConfigError(THRESHOLD_ENV))?;
        }
        if let Some(raw) = lookup(MIN_SAMPLES_ENV) {
            cfg.min_samples = raw
                .trim()
                .parse::<usize>()
                .ok()
                .filter(|n| (1..=cfg.window).contains(n))
                .ok_or(HealthConfigError(MIN_SAMPLES_ENV))?;
        }
        Ok(cfg)
    }
}

/// Result of recording one parse.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HealthStatus {
    /// At or above threshold, or too few samples.
    Ok,
    /// Just dropped below threshold: alert now.
    Alert {
        /// Success rate in the window.
        rate: f64,
    },
    /// Still below threshold; already alerted.
    Low {
        /// Success rate in the window.
        rate: f64,
    },
}

#[derive(Debug, Default)]
struct Window {
    results: VecDeque<bool>,
    alerted: bool,
}

/// Rolling parse success per adapter id.
#[derive(Debug)]
pub struct ParseHealth {
    config: HealthConfig,
    by_id: BTreeMap<String, Window>,
}

impl ParseHealth {
    /// Empty tracker.
    #[must_use]
    pub fn new(config: HealthConfig) -> Self {
        Self {
            config,
            by_id: BTreeMap::new(),
        }
    }

    /// The settings in use.
    #[must_use]
    pub fn config(&self) -> HealthConfig {
        self.config
    }

    /// Records one parse result for `id`. Alerts once per drop; re-arms after
    /// the rate is back at or above the threshold.
    pub fn record(&mut self, id: &str, ok: bool) -> HealthStatus {
        let cfg = self.config;
        let w = self.by_id.entry(id.to_owned()).or_default();
        w.results.push_back(ok);
        while w.results.len() > cfg.window.max(1) {
            w.results.pop_front();
        }
        let n = w.results.len();
        let rate = rate_of(&w.results);
        if n < cfg.min_samples || rate >= cfg.threshold {
            w.alerted = false;
            return HealthStatus::Ok;
        }
        if w.alerted {
            HealthStatus::Low { rate }
        } else {
            w.alerted = true;
            HealthStatus::Alert { rate }
        }
    }

    /// Success rate and sample count for `id`, if any results exist.
    #[must_use]
    pub fn rate(&self, id: &str) -> Option<(f64, usize)> {
        self.by_id
            .get(id)
            .map(|w| (rate_of(&w.results), w.results.len()))
    }

    /// `(id, rate, samples)` for every adapter seen, sorted by id.
    #[must_use]
    pub fn snapshot(&self) -> Vec<(String, f64, usize)> {
        self.by_id
            .iter()
            .map(|(id, w)| (id.clone(), rate_of(&w.results), w.results.len()))
            .collect()
    }
}

fn rate_of(results: &VecDeque<bool>) -> f64 {
    if results.is_empty() {
        return 1.0;
    }
    let ok = results.iter().filter(|r| **r).count();
    #[allow(clippy::cast_precision_loss)] // window sizes are small
    let r = ok as f64 / results.len() as f64;
    r
}

/// Parses `page` with `adapter`, records success (a search page counts as a
/// success only with at least one offer) and logs a warning when the rate
/// first drops below the threshold.
///
/// # Errors
/// The adapter's [`ParseError`].
pub fn parse_page(
    adapter: &dyn Retailer,
    kind: ParseKind,
    page: &Page,
    health: &mut ParseHealth,
) -> Result<Vec<Offer>, ParseError> {
    let result = match kind {
        ParseKind::Search => adapter.parse_search(page),
        ParseKind::Product => adapter.parse_product(page).map(|o| vec![o]),
    };
    // A page that states it has no results parsed fine.
    let ok = match &result {
        Ok(offers) => !offers.is_empty(),
        Err(error) => *error == ParseError::NoResults,
    };
    let id = adapter.info().id;
    if let HealthStatus::Alert { rate } = health.record(id, ok) {
        tracing::warn!(
            retailer = id,
            kind = kind.as_str(),
            rate,
            threshold = health.config().threshold,
            "retailer parse success rate below threshold"
        );
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> HealthConfig {
        HealthConfig {
            threshold: 0.8,
            min_samples: 5,
            window: 10,
        }
    }

    struct Failing;

    impl Retailer for Failing {
        fn info(&self) -> &'static crate::RetailerInfo {
            &crate::registry::LIVERPOOL
        }
        fn search_url(&self, _: &str) -> Result<brightdata::TargetUrl, crate::AdapterError> {
            Err(crate::AdapterError::EmptyQuery)
        }
        fn product_url(
            &self,
            _: &domain::RetailerSku,
        ) -> Result<brightdata::TargetUrl, crate::AdapterError> {
            Err(crate::AdapterError::InvalidSku)
        }
        fn parse_search(&self, _: &Page) -> Result<Vec<Offer>, ParseError> {
            Ok(Vec::new())
        }
        fn parse_product(&self, _: &Page) -> Result<Offer, ParseError> {
            Err(ParseError::NoOffers)
        }
    }

    #[test]
    fn parse_page_records_failures_and_empty_searches() {
        let mut h = ParseHealth::new(cfg());
        let page = crate::parse::test_support::page("https://www.liverpool.com.mx/", "");
        assert!(parse_page(&Failing, ParseKind::Search, &page, &mut h).is_ok());
        assert!(parse_page(&Failing, ParseKind::Product, &page, &mut h).is_err());
        assert_eq!(h.rate("liverpool"), Some((0.0, 2)));
    }

    /// A store whose search page states it has no results.
    struct Empty;

    impl Retailer for Empty {
        fn info(&self) -> &'static crate::RetailerInfo {
            &crate::registry::LIVERPOOL
        }
        fn search_url(&self, _: &str) -> Result<brightdata::TargetUrl, crate::AdapterError> {
            Err(crate::AdapterError::EmptyQuery)
        }
        fn product_url(
            &self,
            _: &domain::RetailerSku,
        ) -> Result<brightdata::TargetUrl, crate::AdapterError> {
            Err(crate::AdapterError::InvalidSku)
        }
        fn parse_search(&self, _: &Page) -> Result<Vec<Offer>, ParseError> {
            Err(ParseError::NoResults)
        }
        fn parse_product(&self, _: &Page) -> Result<Offer, ParseError> {
            Err(ParseError::NoOffers)
        }
    }

    #[test]
    fn a_declared_empty_result_counts_as_parsed() {
        let mut h = ParseHealth::new(cfg());
        let page = crate::parse::test_support::page("https://www.liverpool.com.mx/", "");
        assert_eq!(
            parse_page(&Empty, ParseKind::Search, &page, &mut h),
            Err(ParseError::NoResults)
        );
        assert_eq!(h.rate("liverpool"), Some((1.0, 1)));
    }

    #[test]
    fn no_alert_before_min_samples() {
        let mut h = ParseHealth::new(cfg());
        for _ in 0..4 {
            assert_eq!(h.record("a", false), HealthStatus::Ok);
        }
        assert_eq!(h.record("a", false), HealthStatus::Alert { rate: 0.0 });
    }

    #[test]
    fn alerts_once_per_drop_and_rearms_after_recovery() {
        let mut h = ParseHealth::new(cfg());
        for _ in 0..10 {
            assert_eq!(h.record("a", true), HealthStatus::Ok);
        }
        // 8/10 stays at threshold.
        assert_eq!(h.record("a", false), HealthStatus::Ok);
        assert_eq!(h.record("a", false), HealthStatus::Ok);
        // 7/10 drops below.
        assert_eq!(h.record("a", false), HealthStatus::Alert { rate: 0.7 });
        assert!(matches!(h.record("a", false), HealthStatus::Low { .. }));
        // Other adapters are independent.
        assert_eq!(h.record("b", false), HealthStatus::Ok);
        for _ in 0..10 {
            h.record("a", true);
        }
        assert_eq!(h.rate("a"), Some((1.0, 10)));
        for _ in 0..2 {
            assert_eq!(h.record("a", false), HealthStatus::Ok);
        }
        assert_eq!(h.record("a", false), HealthStatus::Alert { rate: 0.7 });
        let snap = h.snapshot();
        assert_eq!(
            snap.iter().map(|s| s.0.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
    }

    #[test]
    fn config_from_lookup() {
        fn env(pairs: Vec<(&'static str, &'static str)>) -> impl Fn(&str) -> Option<String> {
            move |k| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| (*v).to_owned())
            }
        }
        assert_eq!(
            HealthConfig::from_lookup(env(vec![])).unwrap(),
            HealthConfig::default()
        );
        let c =
            HealthConfig::from_lookup(env(vec![(THRESHOLD_ENV, "0.5"), (MIN_SAMPLES_ENV, "7")]))
                .unwrap();
        assert_eq!((c.threshold, c.min_samples), (0.5, 7));
        for bad in [
            (THRESHOLD_ENV, "1.5"),
            (THRESHOLD_ENV, "x"),
            (MIN_SAMPLES_ENV, "0"),
            (MIN_SAMPLES_ENV, "101"),
        ] {
            assert_eq!(
                HealthConfig::from_lookup(env(vec![bad])),
                Err(HealthConfigError(bad.0))
            );
        }
    }
}
