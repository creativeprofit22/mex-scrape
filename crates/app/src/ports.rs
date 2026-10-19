//! Ports: the traits use cases call and infrastructure implements.
//!
//! Every port is object-safe (`Arc<dyn Port>`): async methods return a boxed
//! [`PortFuture`]. Signatures use only `domain` and app-owned types, and
//! errors are app-owned enums without detail strings, so adapter errors
//! (which may echo URLs or credentials) never cross a port.

use std::future::Future;
use std::pin::Pin;

use domain::{Availability, Gtin, Offer, PriceMxn, RetailerId, RetailerSku, UnixMillis};

/// Boxed future returned by async port methods.
pub type PortFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The current time. Injected so use cases stay deterministic in tests.
pub trait Clock: Send + Sync {
    /// Milliseconds since the Unix epoch.
    fn now(&self) -> UnixMillis;
}

/// Why an [`OfferSource`] call failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum OfferSourceError {
    /// No retailer with that id is registered.
    #[error("unknown retailer")]
    UnknownRetailer,
    /// The search query is blank or too long.
    #[error("invalid search query")]
    InvalidQuery,
    /// The SKU does not have the retailer's format.
    #[error("invalid retailer SKU")]
    InvalidSku,
    /// The retailer has no keyword search (category listings only).
    #[error("keyword search is not available for this retailer")]
    SearchUnavailable,
    /// The page provider refused the fetch until KYC is completed.
    #[error("fetch blocked until KYC is completed")]
    BlockedNeedsKyc,
    /// The page could not be fetched.
    #[error("page fetch failed")]
    FetchFailed,
    /// The page was fetched but no offer could be parsed from it.
    #[error("page could not be parsed")]
    ParseFailed,
    /// This source cannot fetch pages (no fetcher configured).
    #[error("offer source unavailable")]
    Unavailable,
}

/// Offers from Mexican retailers: registry lookups, search and product pages.
pub trait OfferSource: Send + Sync {
    /// The registered retailer owning `host` (case-insensitive), if any.
    fn retailer_for_host(&self, host: &str) -> Option<RetailerId>;

    /// Every offer on the retailer's search page for `query`.
    fn search<'a>(
        &'a self,
        retailer: &'a RetailerId,
        query: &'a str,
    ) -> PortFuture<'a, Result<Vec<Offer>, OfferSourceError>>;

    /// The offer on the retailer's product page for `sku`.
    fn product<'a>(
        &'a self,
        retailer: &'a RetailerId,
        sku: &'a RetailerSku,
    ) -> PortFuture<'a, Result<Offer, OfferSourceError>>;
}

/// An Amazon MX listing found for a GTIN. Preliminary: the Amazon phase may
/// refine these fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmazonListing {
    /// Amazon Standard Identification Number.
    pub asin: String,
    /// Listing title.
    pub title: String,
    /// Current buy-box price, when one is shown.
    pub price_mxn: Option<PriceMxn>,
    /// When the listing was read.
    pub fetched_at: UnixMillis,
}

/// Why an [`AmazonSource`] call failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AmazonSourceError {
    /// Amazon could not be reached or refused the request.
    #[error("Amazon source unavailable")]
    Unavailable,
    /// Amazon asked the caller to slow down.
    #[error("Amazon rate limit reached")]
    RateLimited,
    /// The response could not be understood.
    #[error("Amazon response could not be parsed")]
    ParseFailed,
}

/// Amazon MX catalog lookups. Interface only until the Amazon phase.
pub trait AmazonSource: Send + Sync {
    /// The listing for `gtin`, or `None` when Amazon has none.
    fn listing_by_gtin<'a>(
        &'a self,
        gtin: &'a Gtin,
    ) -> PortFuture<'a, Result<Option<AmazonListing>, AmazonSourceError>>;
}

/// Why a repository call failed. Carries no detail: storage errors may
/// echo connection strings, so adapters log (redacted) themselves or not
/// at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RepoError {
    /// Storage could not be read or written.
    #[error("storage unavailable")]
    Unavailable,
}

/// One observed price of a retailer offer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PricePoint {
    /// Selling price.
    pub price_mxn: PriceMxn,
    /// Strike-through list price, when shown.
    pub list_price_mxn: Option<PriceMxn>,
    /// Stock state.
    pub availability: Availability,
    /// When the price was seen.
    pub observed_at: UnixMillis,
}

/// Stored price history of retailer offers. Interface only until the
/// matching phase.
pub trait PriceHistoryRepo: Send + Sync {
    /// Records the offer's current price.
    fn record<'a>(&'a self, offer: &'a Offer) -> PortFuture<'a, Result<(), RepoError>>;

    /// Prices of one offer observed at or after `since`, oldest first.
    fn history<'a>(
        &'a self,
        retailer: &'a RetailerId,
        sku: &'a RetailerSku,
        since: UnixMillis,
    ) -> PortFuture<'a, Result<Vec<PricePoint>, RepoError>>;
}

/// Page-access outcome totals (Bright Data fetches).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct AccessTotals {
    /// Pages fetched.
    pub ok: i64,
    /// Refusals that need KYC (`blocked_needs_kyc`).
    pub blocked_needs_kyc: i64,
    /// URLs skipped because robots.txt disallows them.
    pub skipped_robots: i64,
    /// Other failures.
    pub failed: i64,
}

impl AccessTotals {
    /// Adds `other` to these totals (saturating, so bad data cannot panic).
    pub fn add(&mut self, other: Self) {
        self.ok = self.ok.saturating_add(other.ok);
        self.blocked_needs_kyc = self
            .blocked_needs_kyc
            .saturating_add(other.blocked_needs_kyc);
        self.skipped_robots = self.skipped_robots.saturating_add(other.skipped_robots);
        self.failed = self.failed.saturating_add(other.failed);
    }
}

/// Access outcome totals for one target host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostAccess {
    /// The target host, as stored.
    pub host: String,
    /// Outcome totals.
    pub totals: AccessTotals,
}

/// Stored per-host access outcomes.
pub trait AccessCountsRepo: Send + Sync {
    /// Outcome totals per host.
    fn counts_by_host(&self) -> PortFuture<'_, Result<Vec<HostAccess>, RepoError>>;
}
