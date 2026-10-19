//! Client configuration: credentials, timeouts, retry and rate limits.

use std::time::Duration;

use http_guard::Secret;

use crate::rate::RateLimits;
use crate::retry::RetryPolicy;

/// Web Unlocker fetches render protected pages and can be slow.
const UNLOCKER_TIMEOUT: Duration = Duration::from_secs(120);
/// Datasets v3 control calls are small (as linkgo `direct.rs`).
const DATASETS_TIMEOUT: Duration = Duration::from_secs(30);
/// A Browser API session: connect, navigate, wait for `load` (as the
/// Bright Data MCP server's 120 s `page.goto` timeout).
const BROWSER_TIMEOUT: Duration = Duration::from_secs(120);

/// How long a rendered page may keep changing after `load` before its HTML
/// is read. Script-built content (e.g. a product list fetched after load)
/// is missing if the HTML is read at `load`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BrowserSettle {
    /// Pause between two checks (default 500 ms).
    pub poll_interval: Duration,
    /// Consecutive quiet checks needed: no network request in flight and
    /// the same element count as the check before (default 3).
    pub quiet_polls: u32,
    /// Checks before the HTML is read anyway (default 60, about 30 s).
    pub max_polls: u32,
}

impl Default for BrowserSettle {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_millis(500),
            quiet_polls: 3,
            max_polls: 60,
        }
    }
}

/// Missing credentials. Messages name the setting, never a value.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    /// No API key.
    #[error("No Bright Data API key: set BRIGHTDATA_API_KEY")]
    MissingApiKey,
    /// No Web Unlocker zone (wording follows brightdata/cli).
    #[error("No Web Unlocker zone specified: set BRIGHTDATA_UNLOCKER_ZONE")]
    MissingUnlockerZone,
}

/// Everything needed to build a [`crate::BrightDataClient`]. The base URL is
/// fixed (`https://api.brightdata.com`) and not configurable.
///
/// Holds [`Secret`]s, so it has no `Debug`.
#[derive(Clone)]
pub struct BrightDataConfig {
    pub(crate) api_key: Secret,
    pub(crate) unlocker_zone: Secret,
    pub(crate) browser_zone: Option<Secret>,
    /// Total timeout for one Web Unlocker request (default 120 s).
    pub unlocker_timeout: Duration,
    /// Total timeout for one Datasets v3 request (default 30 s).
    pub datasets_timeout: Duration,
    /// Total timeout for one Browser API page fetch (default 120 s).
    pub browser_timeout: Duration,
    /// When a rendered page counts as finished (within `browser_timeout`).
    pub browser_settle: BrowserSettle,
    /// Block images, fonts, media, stylesheets and ad or analytics hosts in
    /// Browser API sessions (default on: the Browser API bills per byte).
    pub browser_block_downloads: bool,
    /// Retry policy for transient failures.
    pub retry: RetryPolicy,
    /// Minimum spacing between requests per zone.
    pub rate: RateLimits,
}

impl BrightDataConfig {
    /// Config with default timeouts, retry and rate limits. The zone name is
    /// treated as a secret: it is redacted from every error.
    #[must_use]
    pub fn new(api_key: Secret, unlocker_zone: Secret) -> Self {
        Self {
            api_key,
            unlocker_zone,
            browser_zone: None,
            unlocker_timeout: UNLOCKER_TIMEOUT,
            datasets_timeout: DATASETS_TIMEOUT,
            browser_timeout: BROWSER_TIMEOUT,
            browser_settle: BrowserSettle::default(),
            browser_block_downloads: true,
            retry: RetryPolicy::default(),
            rate: RateLimits::default(),
        }
    }

    /// Enables the Browser API with this zone (e.g.
    /// `BRIGHTDATA_BROWSER_ZONE`). The zone name is redacted like the
    /// Unlocker zone; without it [`crate::Transport::browse`] fails.
    #[must_use]
    pub fn with_browser_zone(mut self, zone: Option<Secret>) -> Self {
        self.browser_zone = zone;
        self
    }

    /// Builds a config from optionally loaded secrets (e.g.
    /// `BRIGHTDATA_API_KEY`, `BRIGHTDATA_UNLOCKER_ZONE`).
    ///
    /// # Errors
    ///
    /// [`ConfigError`] naming the missing setting.
    pub fn from_secrets(
        api_key: Option<Secret>,
        unlocker_zone: Option<Secret>,
    ) -> Result<Self, ConfigError> {
        let api_key = api_key.ok_or(ConfigError::MissingApiKey)?;
        let unlocker_zone = unlocker_zone.ok_or(ConfigError::MissingUnlockerZone)?;
        Ok(Self::new(api_key, unlocker_zone))
    }
}

#[cfg(test)]
mod tests {
    use super::{BrightDataConfig, ConfigError};
    use http_guard::Secret;
    use std::time::Duration;

    fn secret(value: &str) -> Option<Secret> {
        Some(Secret::new(value.to_owned()).unwrap())
    }

    #[test]
    fn requires_key_then_zone() {
        assert_eq!(
            BrightDataConfig::from_secrets(None, None).err(),
            Some(ConfigError::MissingApiKey)
        );
        let error = BrightDataConfig::from_secrets(secret("k"), None)
            .err()
            .unwrap();
        assert_eq!(
            error.to_string(),
            "No Web Unlocker zone specified: set BRIGHTDATA_UNLOCKER_ZONE"
        );
        let config = BrightDataConfig::from_secrets(secret("k"), secret("z")).unwrap();
        assert_eq!(config.unlocker_timeout, Duration::from_secs(120));
        assert_eq!(config.datasets_timeout, Duration::from_secs(30));
        assert_eq!(config.browser_timeout, Duration::from_secs(120));
        assert_eq!(
            config.browser_settle.poll_interval,
            Duration::from_millis(500)
        );
        assert_eq!(config.browser_settle.quiet_polls, 3);
        assert_eq!(config.browser_settle.max_polls, 60);
        assert!(config.browser_block_downloads);
        assert!(config.browser_zone.is_none());
        assert_eq!(config.retry.max_retries, 3);
    }
}
