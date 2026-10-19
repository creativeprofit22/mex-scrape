//! Ports: the traits use cases call and infrastructure implements.
//!
//! Every port is object-safe (`Arc<dyn Port>`): async methods return a boxed
//! [`PortFuture`]. Signatures use only `domain` and app-owned types, and
//! errors are app-owned enums without detail strings, so adapter errors
//! (which may echo URLs or credentials) never cross a port.

use std::future::Future;
use std::pin::Pin;

use domain::{
    Asin, Availability, Gtin, Offer, PriceMxn, ProductMatch, RetailerId, RetailerSku, UnixMillis,
};

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

/// Which provider an Amazon result came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AmazonDataSource {
    /// Amazon's Selling Partner API (primary).
    SpApi,
    /// Bright Data's Amazon dataset (fallback).
    BrightData,
}

impl AmazonDataSource {
    /// Stable lowercase name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SpApi => "sp_api",
            Self::BrightData => "bright_data",
        }
    }
}

/// A best-sellers rank of a catalog item in one category.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SalesRank {
    /// Category name as Amazon shows it.
    pub category: String,
    /// Rank in that category (1 is best).
    pub rank: u32,
}

/// One Amazon.com.mx catalog item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogItem {
    /// Amazon product id.
    pub asin: Asin,
    /// Item title.
    pub title: String,
    /// Brand, when known.
    pub brand: Option<String>,
    /// Image URLs, main image first.
    pub images: Vec<String>,
    /// Sales ranks, best (lowest) first.
    pub sales_ranks: Vec<SalesRank>,
}

/// Catalog items found by one lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogResult {
    /// Matching items (possibly none).
    pub items: Vec<CatalogItem>,
    /// Provider that answered.
    pub source: AmazonDataSource,
    /// When the data was read.
    pub fetched_at: UnixMillis,
}

/// Offer prices for one ASIN on Amazon.com.mx (new condition).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfferSummary {
    /// Buy-box landed price, when there is a buy box.
    pub buy_box_mxn: Option<PriceMxn>,
    /// Lowest landed price of a new offer.
    pub lowest_new_mxn: Option<PriceMxn>,
    /// Number of new offers.
    pub offer_count: u32,
    /// Provider that answered.
    pub source: AmazonDataSource,
    /// When the data was read.
    pub fetched_at: UnixMillis,
}

/// Who ships the order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Fulfillment {
    /// Fulfilled by Amazon.
    Fba,
    /// Shipped by the seller.
    Merchant,
}

/// One fee in a [`FeeEstimate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeeDetail {
    /// Amazon fee type, e.g. `ReferralFee`.
    pub fee_type: String,
    /// Final fee amount.
    pub amount_mxn: PriceMxn,
}

/// Estimated Amazon selling fees for one ASIN at one price, in MXN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeeEstimate {
    /// Sum of all fees.
    pub total_mxn: PriceMxn,
    /// Individual fees.
    pub details: Vec<FeeDetail>,
    /// Provider that answered.
    pub source: AmazonDataSource,
    /// When the data was read.
    pub fetched_at: UnixMillis,
}

/// Why an [`AmazonSource`] call failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AmazonSourceError {
    /// No credentials are configured for this source.
    #[error("Amazon source not configured")]
    NotConfigured,
    /// The credentials were rejected or lack the needed role.
    #[error("Amazon source not authorized")]
    NotAuthorized,
    /// The input (keyword, price) is not acceptable.
    #[error("invalid Amazon lookup input")]
    InvalidInput,
    /// This source cannot answer this kind of lookup.
    #[error("lookup not supported by this Amazon source")]
    Unsupported,
    /// Amazon could not be reached or refused the request.
    #[error("Amazon source unavailable")]
    Unavailable,
    /// Amazon asked the caller to slow down.
    #[error("Amazon rate limit reached")]
    RateLimited,
    /// The response could not be understood (or was not in MXN).
    #[error("Amazon response could not be parsed")]
    ParseFailed,
}

/// Amazon.com.mx catalog, offer and fee lookups. All money is MXN.
pub trait AmazonSource: Send + Sync {
    /// Catalog items carrying barcode `gtin`.
    fn catalog_by_gtin<'a>(
        &'a self,
        gtin: &'a Gtin,
    ) -> PortFuture<'a, Result<CatalogResult, AmazonSourceError>>;

    /// Catalog items matching `keywords`.
    fn catalog_by_keyword<'a>(
        &'a self,
        keywords: &'a str,
    ) -> PortFuture<'a, Result<CatalogResult, AmazonSourceError>>;

    /// New-condition offer prices for `asin`.
    fn offers<'a>(
        &'a self,
        asin: &'a Asin,
    ) -> PortFuture<'a, Result<OfferSummary, AmazonSourceError>>;

    /// Estimated selling fees for `asin` sold at `price`.
    fn fees<'a>(
        &'a self,
        asin: &'a Asin,
        price: PriceMxn,
        fulfillment: Fulfillment,
    ) -> PortFuture<'a, Result<FeeEstimate, AmazonSourceError>>;
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

/// Stored, append-only price history of retailer offers (implemented by
/// `db::price_history::PgPriceHistory`).
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

/// What [`MatchRepo::upsert`] did with the match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpsertOutcome {
    /// The match was inserted or replaced the stored one.
    Written,
    /// A reviewer had rejected the listing's match; the rejection was kept
    /// unchanged and the new match was not stored.
    KeptRejected,
}

/// Stored links between retailer listings and Amazon products.
pub trait MatchRepo: Send + Sync {
    /// Inserts or replaces the listing's match. A stored `rejected` match
    /// (set by a reviewer) is kept unchanged and
    /// [`UpsertOutcome::KeptRejected`] is returned.
    fn upsert<'a>(
        &'a self,
        m: &'a ProductMatch,
    ) -> PortFuture<'a, Result<UpsertOutcome, RepoError>>;

    /// Listings with an accepted match to `asin`, sorted by retailer then
    /// SKU.
    fn accepted_listings<'a>(
        &'a self,
        asin: &'a Asin,
    ) -> PortFuture<'a, Result<Vec<(RetailerId, RetailerSku)>, RepoError>>;
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
