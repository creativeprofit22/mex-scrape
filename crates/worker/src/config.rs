//! Worker settings from the environment (ADR 0011). Every variable is
//! optional with a documented default; a malformed or out-of-range value
//! fails closed with an error naming the variable, never the value.

use std::path::PathBuf;
use std::str::FromStr as _;

use brightdata::runs::Caps;
use croner::Cron;
use domain::PriceMxn;

use crate::metrics::CostRates;

/// A setting could not be read; holds the variable name only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid value for {0}")]
pub struct ConfigError(pub &'static str);

/// Validated worker settings.
#[derive(Debug, Clone)]
pub struct WorkerConfig {
    /// `REFRESH_CRON` (UTC, default `0 */6 * * *`).
    pub refresh_cron: Cron,
    /// `CATEGORY_BROWSE_CRON` (UTC, default `30 3 * * *`).
    pub category_cron: Cron,
    /// `JOB_VISIBILITY_SECS` (default 900, 660–86400): how long a read
    /// message stays hidden; above the 600 s Bright Data poll budget.
    pub visibility_secs: i32,
    /// `JOB_MAX_ATTEMPTS` (default 5, 1–50): reads before a failing
    /// message is dead-lettered.
    pub max_attempts: i32,
    /// `JOB_POLL_IDLE_MS` (default 2000, 100–60000): sleep when both
    /// queues are empty.
    pub poll_idle_ms: u64,
    /// `BRIGHTDATA_MAX_RUNS_PER_TENANT_PER_DAY`,
    /// `BRIGHTDATA_MAX_ITEMS_PER_TENANT_PER_DAY`,
    /// `BRIGHTDATA_MAX_ITEMS_PER_RUN` (defaults: [`Caps::default`]).
    pub caps: Caps,
    /// `AMAZON_MAX_REQUESTS_PER_TENANT_PER_DAY` (default 1000).
    pub amazon_requests_per_day: u32,
    /// `SEARCH_MAX_OFFERS_PER_RETAILER` (default 3, 1–20): offers per
    /// retailer matched and priced by a search.
    pub search_max_offers_per_retailer: usize,
    /// `REFRESH_BATCH_SIZE` (default 25, 1–50): SKUs per refresh run.
    pub refresh_batch_size: usize,
    /// `CATEGORY_MAX_PAGES_PER_RUN` (default 4, 1–50): listing pages per
    /// category browse run.
    pub category_max_pages: usize,
    /// `ROI_PREP_FEE_CENTAVOS` (default 0).
    pub prep_fee: PriceMxn,
    /// `ROI_SHIPPING_CENTAVOS` (default 0).
    pub shipping: PriceMxn,
    /// `BRIGHTDATA_UNLOCKER_MICROUSD_PER_REQUEST` (default 1500) and
    /// `BRIGHTDATA_BROWSER_MICROUSD_PER_MB` (default 8000).
    pub cost_rates: CostRates,
    /// `CATEGORY_LISTINGS_PATH` (default `config/category_listings.json`).
    pub category_listings_path: PathBuf,
    /// `DEMO_DATA` (default `false`; exactly `true` or `false`): serve the
    /// demo catalog through a fake Bright Data connector, with no network
    /// calls (ADR 0016). It also raises the default Bright Data caps to
    /// [`DEMO_CAPS`], since nothing is paid; set caps still win.
    pub demo_data: bool,
}

/// Default Bright Data caps in demo mode: a demo search is one page run
/// plus one or two Amazon dataset runs per offer, which the paid defaults
/// would cap after one search.
pub const DEMO_CAPS: Caps = Caps {
    max_runs_per_tenant_per_day: 10_000,
    max_items_per_tenant_per_day: 1_000_000,
    max_items_per_run: 50,
};

fn flag(lookup: &impl Fn(&str) -> Option<String>, name: &'static str) -> Result<bool, ConfigError> {
    match lookup(name).as_deref().map(str::trim) {
        None | Some("false") => Ok(false),
        Some("true") => Ok(true),
        Some(_) => Err(ConfigError(name)),
    }
}

fn number<T: std::str::FromStr + PartialOrd + Copy>(
    lookup: &impl Fn(&str) -> Option<String>,
    name: &'static str,
    default: T,
    min: T,
    max: T,
) -> Result<T, ConfigError> {
    match lookup(name) {
        None => Ok(default),
        Some(raw) => raw
            .trim()
            .parse::<T>()
            .ok()
            .filter(|v| *v >= min && *v <= max)
            .ok_or(ConfigError(name)),
    }
}

fn cron(
    lookup: &impl Fn(&str) -> Option<String>,
    name: &'static str,
    default: &str,
) -> Result<Cron, ConfigError> {
    let raw = lookup(name).unwrap_or_else(|| default.to_owned());
    Cron::from_str(raw.trim()).map_err(|_| ConfigError(name))
}

fn centavos(
    lookup: &impl Fn(&str) -> Option<String>,
    name: &'static str,
) -> Result<PriceMxn, ConfigError> {
    let value = number(lookup, name, 0_i64, 0, 100_000_000)?;
    PriceMxn::from_centavos(value).map_err(|_| ConfigError(name))
}

impl WorkerConfig {
    /// Reads settings through `lookup` (the process environment in
    /// production).
    ///
    /// # Errors
    ///
    /// [`ConfigError`] naming the first malformed variable.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let demo_data = flag(&lookup, "DEMO_DATA")?;
        let defaults = if demo_data {
            DEMO_CAPS
        } else {
            Caps::default()
        };
        let path = lookup("CATEGORY_LISTINGS_PATH")
            .map(|p| p.trim().to_owned())
            .unwrap_or_else(|| "config/category_listings.json".to_owned());
        if path.is_empty() {
            return Err(ConfigError("CATEGORY_LISTINGS_PATH"));
        }
        Ok(Self {
            refresh_cron: cron(&lookup, "REFRESH_CRON", "0 */6 * * *")?,
            category_cron: cron(&lookup, "CATEGORY_BROWSE_CRON", "30 3 * * *")?,
            visibility_secs: number(&lookup, "JOB_VISIBILITY_SECS", 900, 660, 86_400)?,
            max_attempts: number(&lookup, "JOB_MAX_ATTEMPTS", 5, 1, 50)?,
            poll_idle_ms: number(&lookup, "JOB_POLL_IDLE_MS", 2000, 100, 60_000)?,
            caps: Caps {
                max_runs_per_tenant_per_day: number(
                    &lookup,
                    "BRIGHTDATA_MAX_RUNS_PER_TENANT_PER_DAY",
                    defaults.max_runs_per_tenant_per_day,
                    0,
                    10_000,
                )?,
                max_items_per_tenant_per_day: number(
                    &lookup,
                    "BRIGHTDATA_MAX_ITEMS_PER_TENANT_PER_DAY",
                    defaults.max_items_per_tenant_per_day,
                    0,
                    1_000_000,
                )?,
                max_items_per_run: number(
                    &lookup,
                    "BRIGHTDATA_MAX_ITEMS_PER_RUN",
                    defaults.max_items_per_run,
                    1,
                    1_000,
                )?,
            },
            amazon_requests_per_day: number(
                &lookup,
                "AMAZON_MAX_REQUESTS_PER_TENANT_PER_DAY",
                1000,
                0,
                1_000_000,
            )?,
            search_max_offers_per_retailer: number(
                &lookup,
                "SEARCH_MAX_OFFERS_PER_RETAILER",
                3,
                1,
                20,
            )?,
            refresh_batch_size: number(&lookup, "REFRESH_BATCH_SIZE", 25, 1, 50)?,
            category_max_pages: number(&lookup, "CATEGORY_MAX_PAGES_PER_RUN", 4, 1, 50)?,
            prep_fee: centavos(&lookup, "ROI_PREP_FEE_CENTAVOS")?,
            shipping: centavos(&lookup, "ROI_SHIPPING_CENTAVOS")?,
            cost_rates: CostRates {
                unlocker_microusd_per_request: number(
                    &lookup,
                    "BRIGHTDATA_UNLOCKER_MICROUSD_PER_REQUEST",
                    1500,
                    0,
                    1_000_000,
                )?,
                browser_microusd_per_mb: number(
                    &lookup,
                    "BRIGHTDATA_BROWSER_MICROUSD_PER_MB",
                    8000,
                    0,
                    10_000_000,
                )?,
            },
            category_listings_path: PathBuf::from(path),
            demo_data,
        })
    }

    /// Reads settings from the process environment.
    ///
    /// # Errors
    ///
    /// [`ConfigError`] naming the first malformed variable.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn load(pairs: &[(&str, &str)]) -> Result<WorkerConfig, ConfigError> {
        let map: BTreeMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        WorkerConfig::from_lookup(|name| map.get(name).cloned())
    }

    #[test]
    fn defaults_match_the_docs() {
        let c = load(&[]).unwrap();
        assert_eq!(c.refresh_cron.as_str(), "0 */6 * * *");
        assert_eq!(c.category_cron.as_str(), "30 3 * * *");
        assert_eq!(
            (c.visibility_secs, c.max_attempts, c.poll_idle_ms),
            (900, 5, 2000)
        );
        assert_eq!(c.caps, Caps::default());
        assert!(!c.demo_data);
        assert_eq!(c.amazon_requests_per_day, 1000);
        assert_eq!(
            (
                c.search_max_offers_per_retailer,
                c.refresh_batch_size,
                c.category_max_pages
            ),
            (3, 25, 4)
        );
        assert_eq!((c.prep_fee.centavos(), c.shipping.centavos()), (0, 0));
        assert_eq!(c.cost_rates.unlocker_microusd_per_request, 1500);
        assert_eq!(c.cost_rates.browser_microusd_per_mb, 8000);
        assert_eq!(
            c.category_listings_path,
            PathBuf::from("config/category_listings.json")
        );
    }

    #[test]
    fn overrides_are_read() {
        let c = load(&[
            ("REFRESH_CRON", "*/5 * * * *"),
            ("JOB_MAX_ATTEMPTS", "2"),
            ("BRIGHTDATA_MAX_RUNS_PER_TENANT_PER_DAY", "40"),
            ("ROI_PREP_FEE_CENTAVOS", "1500"),
        ])
        .unwrap();
        assert_eq!(c.refresh_cron.as_str(), "*/5 * * * *");
        assert_eq!(c.max_attempts, 2);
        assert_eq!(c.caps.max_runs_per_tenant_per_day, 40);
        assert_eq!(c.prep_fee.centavos(), 1500);
    }

    #[test]
    fn demo_mode_raises_only_unset_caps() {
        let c = load(&[("DEMO_DATA", "true")]).unwrap();
        assert!(c.demo_data);
        assert_eq!(c.caps, DEMO_CAPS);
        let c = load(&[
            ("DEMO_DATA", "true"),
            ("BRIGHTDATA_MAX_RUNS_PER_TENANT_PER_DAY", "40"),
        ])
        .unwrap();
        assert_eq!(c.caps.max_runs_per_tenant_per_day, 40);
        assert_eq!(
            c.caps.max_items_per_tenant_per_day,
            DEMO_CAPS.max_items_per_tenant_per_day
        );
        assert!(!load(&[("DEMO_DATA", "false")]).unwrap().demo_data);
    }

    #[test]
    fn bad_values_fail_closed_naming_the_variable_only() {
        for (name, value) in [
            ("REFRESH_CRON", "not a cron"),
            ("CATEGORY_BROWSE_CRON", "61 * * * *"),
            ("JOB_VISIBILITY_SECS", "30"),
            ("JOB_MAX_ATTEMPTS", "0"),
            ("JOB_MAX_ATTEMPTS", "-1"),
            ("REFRESH_BATCH_SIZE", "abc"),
            ("ROI_SHIPPING_CENTAVOS", "-5"),
            ("CATEGORY_LISTINGS_PATH", "  "),
            ("DEMO_DATA", "1"),
            ("DEMO_DATA", "TRUE"),
        ] {
            let err = load(&[(name, value)]).unwrap_err();
            assert_eq!(err, ConfigError(name));
            assert!(!err.to_string().contains(value) || value.trim().is_empty());
        }
    }
}
