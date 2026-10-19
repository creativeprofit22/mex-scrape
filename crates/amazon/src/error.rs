//! SP-API client errors. Messages never contain a URL, and every secret
//! (LWA credentials and access tokens) is redacted before it is stored here.

use app::AmazonSourceError;

/// Why an SP-API (or LWA) call failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SpApiError {
    /// Credentials were rejected (LWA `invalid_grant`, SP-API 401/403) or
    /// the app lacks the needed role.
    #[error("SP-API not authorized: {0}")]
    NotAuthorized(String),
    /// 429 after every retry.
    #[error("SP-API rate limit reached: {0}")]
    RateLimited(String),
    /// 5xx, timeouts or transport failures after every retry.
    #[error("SP-API unavailable: {0}")]
    Transient(String),
    /// The input was rejected before or by the API (other 4xx).
    #[error("SP-API rejected the request: {0}")]
    Invalid(String),
    /// The response could not be read, or was not in MXN.
    #[error("SP-API response could not be parsed: {0}")]
    Parse(String),
    /// The client could not be built.
    #[error("SP-API client could not be created: {0}")]
    Build(String),
}

impl SpApiError {
    /// The port error for this failure (no detail crosses the port).
    #[must_use]
    pub fn to_source_error(&self) -> AmazonSourceError {
        match self {
            Self::NotAuthorized(_) => AmazonSourceError::NotAuthorized,
            Self::RateLimited(_) => AmazonSourceError::RateLimited,
            Self::Transient(_) | Self::Build(_) => AmazonSourceError::Unavailable,
            Self::Invalid(_) => AmazonSourceError::InvalidInput,
            Self::Parse(_) => AmazonSourceError::ParseFailed,
        }
    }
}
