//! Amazon.com.mx catalog, offer and fee data (ADR 0003, ADR 0010).
//!
//! - [`SpApiClient`]: Selling Partner API client. Login with Amazon (LWA)
//!   access tokens are refreshed and cached ([`SpApiConfig`] holds the
//!   credentials) and sent as `x-amz-access-token` to the North America
//!   endpoint for marketplace [`MX_MARKETPLACE_ID`]. Requests are spaced per
//!   operation ([`RateLimiter`]) and retried with capped backoff on 429,
//!   5xx and transport failures. Every money value is MXN.
//! - [`SpApiAmazonSource`]: the SP-API as an `app::ports::AmazonSource`
//!   (primary).
//! - [`BrightDataAmazonSource`]: Bright Data's Amazon dataset
//!   [`AMAZON_GLOBAL_DATASET_ID`] as an `AmazonSource` (fallback; fees
//!   unsupported), run through `brightdata::runs::Runner`.
//!
//! `app::FallbackAmazonSource` combines the two. All HTTP goes through
//! `http-guard`; credentials and tokens are redacted from every error.

mod brightdata_source;
mod client;
mod config;
mod error;
mod lwa;
mod parse;
mod rate;
mod spapi_source;

pub use brightdata_source::{
    AMAZON_GLOBAL_DATASET_ID, AMAZON_MX_ORIGIN, BrightDataAmazonSource, FALLBACK_JOB,
    FALLBACK_TENANT, LIMIT_PER_INPUT,
};
pub use client::{IdentifierType, MAX_IDENTIFIERS, MAX_KEYWORD_CHARS, SpApiClient};
pub use config::{
    ConfigError, LWA_HOST, LWA_TOKEN_URL, MX_MARKETPLACE_ID, Operation, RateLimit, RateTable,
    SPAPI_NA_HOST, SPAPI_NA_SANDBOX_HOST, SpApiConfig, SpApiEndpoint,
};
pub use error::SpApiError;
pub use parse::MXN;
pub use rate::{RateLimiter, per_request_from_header};
pub use spapi_source::SpApiAmazonSource;
