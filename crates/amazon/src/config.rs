//! SP-API configuration: credentials, endpoint, marketplace and limits.

use std::time::Duration;

use brightdata::RetryPolicy;
use http_guard::Secret;

/// Amazon.com.mx marketplace id.
pub const MX_MARKETPLACE_ID: &str = "A1AM78C64UM0Y8";
/// North America SP-API host (production).
pub const SPAPI_NA_HOST: &str = "sellingpartnerapi-na.amazon.com";
/// North America SP-API sandbox host (static, canned responses).
pub const SPAPI_NA_SANDBOX_HOST: &str = "sandbox.sellingpartnerapi-na.amazon.com";
/// Login with Amazon host (token refresh).
pub const LWA_HOST: &str = "api.amazon.com";
/// Login with Amazon token endpoint.
pub const LWA_TOKEN_URL: &str = "https://api.amazon.com/auth/o2/token";

/// Default total timeout for one SP-API or LWA request.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Which SP-API environment to call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SpApiEndpoint {
    /// Live data.
    #[default]
    Production,
    /// Amazon's static sandbox (canned responses for documented inputs).
    Sandbox,
}

impl SpApiEndpoint {
    /// The host for this environment.
    #[must_use]
    pub const fn host(self) -> &'static str {
        match self {
            Self::Production => SPAPI_NA_HOST,
            Self::Sandbox => SPAPI_NA_SANDBOX_HOST,
        }
    }
}

/// The SP-API operations this crate calls; each has its own usage plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Operation {
    /// Catalog Items 2022-04-01 `searchCatalogItems`.
    SearchCatalog,
    /// Product Pricing v0 `getItemOffers`.
    ItemOffers,
    /// Product Fees v0 `getMyFeesEstimateForASIN`.
    FeesEstimate,
}

impl Operation {
    /// Every operation, in a fixed order.
    pub const ALL: [Self; 3] = [Self::SearchCatalog, Self::ItemOffers, Self::FeesEstimate];
}

/// A usage plan: one request every `per_request` on average, with up to
/// `burst` requests back to back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimit {
    /// Average spacing (1 / rate).
    pub per_request: Duration,
    /// Burst size (at least 1).
    pub burst: u32,
}

/// Default usage plans (Amazon reference pages, checked 2026-10-19).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateTable {
    /// `searchCatalogItems`: 2 req/s, burst 2.
    pub search_catalog: RateLimit,
    /// `getItemOffers`: 0.5 req/s, burst 1.
    pub item_offers: RateLimit,
    /// `getMyFeesEstimateForASIN`: 1 req/s, burst 2.
    pub fees_estimate: RateLimit,
}

impl Default for RateTable {
    fn default() -> Self {
        Self {
            search_catalog: RateLimit {
                per_request: Duration::from_millis(500),
                burst: 2,
            },
            item_offers: RateLimit {
                per_request: Duration::from_secs(2),
                burst: 1,
            },
            fees_estimate: RateLimit {
                per_request: Duration::from_secs(1),
                burst: 2,
            },
        }
    }
}

impl RateTable {
    /// The plan for `op`.
    #[must_use]
    pub const fn get(&self, op: Operation) -> RateLimit {
        match op {
            Operation::SearchCatalog => self.search_catalog,
            Operation::ItemOffers => self.item_offers,
            Operation::FeesEstimate => self.fees_estimate,
        }
    }
}

/// Missing SP-API credentials. Messages name the setting, never a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    /// No LWA client id.
    #[error("No SP-API LWA client id: set SPAPI_LWA_CLIENT_ID")]
    MissingClientId,
    /// No LWA client secret.
    #[error("No SP-API LWA client secret: set SPAPI_LWA_CLIENT_SECRET")]
    MissingClientSecret,
    /// No refresh token (the app is not authorized yet).
    #[error("No SP-API refresh token: set SPAPI_REFRESH_TOKEN")]
    MissingRefreshToken,
}

/// Everything needed to build an [`crate::SpApiClient`]. Holds [`Secret`]s,
/// so it has no `Debug`.
#[derive(Clone)]
pub struct SpApiConfig {
    pub(crate) client_id: Secret,
    pub(crate) client_secret: Secret,
    pub(crate) refresh_token: Secret,
    /// Production or sandbox.
    pub endpoint: SpApiEndpoint,
    /// Total timeout for one request (default 30 s).
    pub timeout: Duration,
    /// Retries for 429, 5xx and transport failures (default 3: 1 s, 2 s,
    /// 4 s, capped at 30 s; a numeric `Retry-After` replaces the backoff).
    pub retry: RetryPolicy,
    /// Per-operation usage plans.
    pub rates: RateTable,
}

impl SpApiConfig {
    /// Builds a config from optionally loaded secrets
    /// (`SPAPI_LWA_CLIENT_ID`, `SPAPI_LWA_CLIENT_SECRET`,
    /// `SPAPI_REFRESH_TOKEN`), for production.
    ///
    /// # Errors
    ///
    /// [`ConfigError`] naming the first missing setting.
    pub fn from_secrets(
        client_id: Option<Secret>,
        client_secret: Option<Secret>,
        refresh_token: Option<Secret>,
    ) -> Result<Self, ConfigError> {
        Ok(Self {
            client_id: client_id.ok_or(ConfigError::MissingClientId)?,
            client_secret: client_secret.ok_or(ConfigError::MissingClientSecret)?,
            refresh_token: refresh_token.ok_or(ConfigError::MissingRefreshToken)?,
            endpoint: SpApiEndpoint::Production,
            timeout: REQUEST_TIMEOUT,
            retry: RetryPolicy::default(),
            rates: RateTable::default(),
        })
    }

    /// The same config pointed at `endpoint`.
    #[must_use]
    pub fn with_endpoint(mut self, endpoint: SpApiEndpoint) -> Self {
        self.endpoint = endpoint;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secret(value: &str) -> Option<Secret> {
        Some(Secret::new(value.to_owned()).unwrap())
    }

    #[test]
    fn requires_every_credential_in_order() {
        assert_eq!(
            SpApiConfig::from_secrets(None, None, None).err(),
            Some(ConfigError::MissingClientId)
        );
        assert_eq!(
            SpApiConfig::from_secrets(secret("id"), None, None).err(),
            Some(ConfigError::MissingClientSecret)
        );
        let missing = SpApiConfig::from_secrets(secret("id"), secret("s"), None)
            .err()
            .unwrap();
        assert_eq!(
            missing.to_string(),
            "No SP-API refresh token: set SPAPI_REFRESH_TOKEN"
        );
        let config = SpApiConfig::from_secrets(secret("id"), secret("s"), secret("r")).unwrap();
        assert_eq!(config.endpoint.host(), SPAPI_NA_HOST);
        assert_eq!(config.timeout, Duration::from_secs(30));
        assert_eq!(
            config.with_endpoint(SpApiEndpoint::Sandbox).endpoint.host(),
            SPAPI_NA_SANDBOX_HOST
        );
    }

    #[test]
    fn default_rates_match_the_usage_plans() {
        let table = RateTable::default();
        assert_eq!(
            table.get(Operation::SearchCatalog),
            RateLimit {
                per_request: Duration::from_millis(500),
                burst: 2
            }
        );
        assert_eq!(
            table.get(Operation::ItemOffers),
            RateLimit {
                per_request: Duration::from_secs(2),
                burst: 1
            }
        );
        assert_eq!(
            table.get(Operation::FeesEstimate),
            RateLimit {
                per_request: Duration::from_secs(1),
                burst: 2
            }
        );
    }
}
