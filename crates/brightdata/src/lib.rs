//! Async Bright Data client (Web Unlocker and Datasets v3) with durable,
//! capped run tracking.
//!
//! Product-agnostic: no mex-scrape domain types, so other projects can adopt
//! it (mex-scrape ADR 0006).
//! Ported from linkgo's blocking connector (`brightdata/direct.rs`,
//! `runs.rs`, `recovery.rs`).
//!
//! - [`BrightDataClient`]: the live client. Every request goes through an
//!   `http_guard::GuardedClient` allowlisted to `api.brightdata.com`, is rate
//!   limited per zone, retried with capped exponential backoff when the
//!   error is transient, and classified as [`BrightDataError::Terminal`] or
//!   [`BrightDataError::Transient`]. Errors never contain the URL, the API
//!   key or the zone name.
//! - Browser API (ADR 0007): [`Transport::browse`] renders a page in a
//!   remote Chrome over CDP (`http_guard::GuardedWebSocket`), only for
//!   targets proven to need JavaScript; needs a browser zone
//!   ([`BrightDataConfig::with_browser_zone`]).
//! - [`Transport`]: the five calls a run needs; implemented by the client
//!   and by test fakes.
//! - [`runs`]: run start/drive/resume/cancel behind fail-closed gates, with
//!   storage behind [`runs::RunStore`] (Postgres lives in `db`/`worker`).
//! - [`recovery`]: start-up recovery of interrupted runs.

mod client;
mod config;
mod error;
mod ids;
mod rate;
pub mod recovery;
mod refusal;
mod retry;
pub mod robots;
pub mod runs;
mod transport;

pub use client::browser::{BRIGHTDATA_BROWSER_HOST, BRIGHTDATA_BROWSER_PORT};
pub use client::{
    BRIGHTDATA_API_BASE, BRIGHTDATA_API_HOST, BrightDataClient, DatasetTrigger, Download,
    MAX_TRIGGER_INPUTS, SnapshotStatus, UnlockRequest, UnlockedPage,
};
pub use config::{BrightDataConfig, BrowserSettle, ConfigError};
pub use error::{BrightDataError, is_transient_status};
pub use ids::{CountryCode, DatasetId, DiscoverBy, SnapshotId, TargetUrl};
pub use rate::{RateKey, RateLimits, ZoneLimiter};
pub use refusal::{RefusalReason, classify_browser_error, classify_unlocker};
pub use retry::{RetryPolicy, parse_retry_after};
pub use transport::Transport;
