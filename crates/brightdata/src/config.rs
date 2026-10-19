//! Client configuration: credentials, timeouts, retry and rate limits.

use std::time::Duration;

use http_guard::Secret;

use crate::rate::RateLimits;
use crate::retry::RetryPolicy;

/// Web Unlocker fetches render protected pages and can be slow.
const UNLOCKER_TIMEOUT: Duration = Duration::from_secs(120);
/// Datasets v3 control calls are small (as linkgo `direct.rs`).
const DATASETS_TIMEOUT: Duration = Duration::from_secs(30);

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
    /// Total timeout for one Web Unlocker request (default 120 s).
    pub unlocker_timeout: Duration,
    /// Total timeout for one Datasets v3 request (default 30 s).
    pub datasets_timeout: Duration,
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
            unlocker_timeout: UNLOCKER_TIMEOUT,
            datasets_timeout: DATASETS_TIMEOUT,
            retry: RetryPolicy::default(),
            rate: RateLimits::default(),
        }
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
        assert_eq!(config.retry.max_retries, 3);
    }
}
