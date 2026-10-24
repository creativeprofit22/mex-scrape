//! Ports: the traits use cases call and infrastructure implements.
//!
//! Every port is object-safe (`Arc<dyn Port>`): async methods return a boxed
//! [`PortFuture`]. Signatures use only `domain` and app-owned types, and
//! errors are app-owned enums without detail strings, so adapter errors
//! (which may echo URLs or credentials) never cross a port.

use std::future::Future;
use std::pin::Pin;

use domain::{
    AgentStop, Asin, Availability, ChatHistory, Confidence, Email, Gtin, Locale, MatchDecision,
    MatchStatus, Memberships, Offer, Plan, PriceMxn, ProductMatch, RetailerId,
    RetailerSearchOutcome, RetailerSku, SearchQuery, SearchStatus, SuggestionReason, TenantId,
    TokenCount, TokenUsage, UnixMillis, UserId,
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

    /// The source named `name` (as [`Self::as_str`] writes it), if any.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        [Self::SpApi, Self::BrightData]
            .into_iter()
            .find(|s| s.as_str() == name)
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
    /// Refused before any call: the global kill switch is on or the
    /// tenant's daily Amazon request cap is reached. Never falls back.
    #[error("Amazon request refused by the spending guard")]
    Refused,
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

    /// Whether any listing is matched to `asin`, in any status (`rejected`
    /// included): the product is known even when nothing is accepted.
    fn has_any_match<'a>(&'a self, asin: &'a Asin) -> PortFuture<'a, Result<bool, RepoError>>;
}

/// The Amazon name and brand of a matched product, as seen in a catalog
/// lookup (ADR 0017). Build it with [`AmazonProduct::new`] or
/// [`AmazonProduct::from_catalog`], which normalize and bound the text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmazonProduct {
    /// Amazon product id.
    pub asin: Asin,
    /// Product name on Amazon: trimmed, single-spaced, 1 to
    /// [`crate::product::MAX_TITLE_CHARS`] characters.
    pub title: String,
    /// Brand, when known: trimmed, single-spaced, 1 to
    /// [`crate::product::MAX_BRAND_CHARS`] characters.
    pub brand: Option<String>,
    /// Provider that answered the lookup.
    pub source: AmazonDataSource,
    /// When the lookup was read.
    pub seen_at: UnixMillis,
}

/// Stored Amazon product names (implemented by
/// `db::amazon_products::PgAmazonProducts`).
pub trait AmazonProductRepo: Send + Sync {
    /// Stores the product, or refreshes the stored one when `product` is at
    /// least as new; a missing brand keeps the stored brand. Older data is
    /// ignored without error.
    fn upsert<'a>(&'a self, product: &'a AmazonProduct) -> PortFuture<'a, Result<(), RepoError>>;

    /// The stored product for `asin`, if any.
    fn find<'a>(
        &'a self,
        asin: &'a Asin,
    ) -> PortFuture<'a, Result<Option<AmazonProduct>, RepoError>>;
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

/// Why a [`RequestBudget`] refused or could not answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BudgetError {
    /// The global kill switch is on (or safety settings are missing).
    #[error("kill switch is on")]
    KillSwitch,
    /// The tenant's daily cap would be exceeded.
    #[error("daily request cap reached")]
    CapReached,
    /// The budget store could not be read; callers fail closed.
    #[error("request budget unavailable")]
    Unavailable,
}

/// A per-tenant daily budget of paid Amazon requests (SP-API or the
/// Bright Data fallback), checked together with the global kill switch.
/// Implemented by `db::usage::PgRequestBudget`.
pub trait RequestBudget: Send + Sync {
    /// Atomically takes `n` requests from `tenant`'s budget for today, or
    /// takes nothing and refuses.
    fn consume<'a>(
        &'a self,
        tenant: &'a TenantId,
        n: u32,
    ) -> PortFuture<'a, Result<(), BudgetError>>;
}

/// Id of a stored search.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SearchId(pub i64);

/// What one retailer contributed to a search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetailerOutcomeView {
    /// The retailer.
    pub retailer: RetailerId,
    /// Its outcome.
    pub outcome: RetailerSearchOutcome,
    /// Offers it returned.
    pub offers: u32,
}

/// The Amazon side of a search result, when a match was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultMatchView {
    /// Matched Amazon product.
    pub asin: Asin,
    /// Review state of the match.
    pub status: MatchStatus,
    /// Matcher confidence.
    pub confidence: Confidence,
    /// Amazon selling price used for ROI (buy box, else lowest new).
    pub amazon_price_mxn: Option<PriceMxn>,
    /// Estimated Amazon fees at that price.
    pub fees_mxn: Option<PriceMxn>,
    /// Profit per unit in centavos (negative for a loss).
    pub profit_centavos: Option<i64>,
    /// Profit over total cost, basis points.
    pub roi_bp: Option<i64>,
    /// Profit over selling price, basis points.
    pub margin_bp: Option<i64>,
}

/// One retailer offer found by a search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResultView {
    /// The retailer.
    pub retailer: RetailerId,
    /// Retailer SKU.
    pub retailer_sku: RetailerSku,
    /// Listing title.
    pub title: String,
    /// Listing URL.
    pub url: String,
    /// Retailer price.
    pub price_mxn: PriceMxn,
    /// Stock state when the offer was seen.
    pub availability: Availability,
    /// Whether buying needs a paid store membership (Costco, Sam's Club).
    pub membership_required: bool,
    /// Amazon match and ROI, when found.
    pub amazon: Option<ResultMatchView>,
}

/// A stored search with its outcomes and results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchView {
    /// Search id.
    pub id: SearchId,
    /// The cleaned query.
    pub query: String,
    /// Job status.
    pub status: SearchStatus,
    /// Fixed reason code for `failed`/`refused`/`partial` (empty otherwise).
    pub reason: String,
    /// When the search was created.
    pub created_at: UnixMillis,
    /// When it reached a terminal status.
    pub finished_at: Option<UnixMillis>,
    /// Per-retailer outcomes, sorted by retailer.
    pub retailers: Vec<RetailerOutcomeView>,
    /// Results, sorted by retailer then SKU.
    pub results: Vec<SearchResultView>,
    /// Results hidden because they need a membership the reader lacks
    /// (always 0 as stored; set by [`crate::SearchResults`]).
    pub hidden_results: u32,
}

/// Stored searches (implemented by `db::searches::PgSearches`).
pub trait SearchRepo: Send + Sync {
    /// Stores a `queued` search and enqueues its job in one transaction.
    fn create_and_enqueue<'a>(
        &'a self,
        tenant: &'a TenantId,
        query: &'a SearchQuery,
    ) -> PortFuture<'a, Result<SearchId, RepoError>>;

    /// The tenant's search `id`; `None` if unknown or owned by another
    /// tenant.
    fn load<'a>(
        &'a self,
        tenant: &'a TenantId,
        id: SearchId,
    ) -> PortFuture<'a, Result<Option<SearchView>, RepoError>>;

    /// Results of the tenant's searches created at or after `since`, one
    /// row per listing (its newest occurrence), newest first, at most
    /// `limit` rows. Stored state only: no overlay, no membership hiding.
    fn recent_results<'a>(
        &'a self,
        tenant: &'a TenantId,
        since: UnixMillis,
        filter: &'a RecentResultsFilter,
        limit: u32,
    ) -> PortFuture<'a, Result<Vec<TrackedResultView>, RepoError>>;
}

/// Narrows [`SearchRepo::recent_results`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecentResultsFilter {
    /// Listing title contains this text (case-insensitive).
    pub title_contains: Option<SearchQuery>,
    /// Only this listing.
    pub listing: Option<(RetailerId, RetailerSku)>,
}

/// A search result of one of the tenant's recent searches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackedResultView {
    /// Search the result belongs to.
    pub search_id: SearchId,
    /// When that search was created.
    pub seen_at: UnixMillis,
    /// The result as stored.
    pub result: SearchResultView,
}

/// A stored user account, without its password hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserRecord {
    /// User id.
    pub id: UserId,
    /// Normalized sign-in address.
    pub email: Email,
    /// The tenant owning the user's data (`user-<id>`).
    pub tenant: TenantId,
    /// Subscription plan.
    pub plan: Plan,
    /// Store memberships.
    pub memberships: Memberships,
    /// Whether onboarding (memberships) was completed.
    pub onboarded: bool,
    /// Preferred UI language.
    pub locale: Locale,
}

/// A user with the stored password hash, for sign-in only.
#[derive(Clone, PartialEq, Eq)]
pub struct UserCredentials {
    /// The account.
    pub user: UserRecord,
    /// PHC-format password hash.
    pub password_hash: String,
}

impl std::fmt::Debug for UserCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UserCredentials")
            .field("user", &self.user)
            .field("password_hash", &"<redacted>")
            .finish()
    }
}

/// Why a user could not be created.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CreateUserError {
    /// Another account already uses this address.
    #[error("e-mail address already registered")]
    EmailTaken,
    /// Storage could not be written.
    #[error("storage unavailable")]
    Unavailable,
}

/// Stored user accounts (implemented by `db::users::PgUsers`). Every query
/// is keyed by user id or e-mail; the tenant is derived from the id.
pub trait UserRepo: Send + Sync {
    /// Creates a `free`, not-onboarded account with tenant `user-<id>`.
    fn create<'a>(
        &'a self,
        email: &'a Email,
        password_hash: &'a str,
        locale: Locale,
    ) -> PortFuture<'a, Result<UserRecord, CreateUserError>>;

    /// The account and password hash for `email`, if registered.
    fn find_credentials<'a>(
        &'a self,
        email: &'a Email,
    ) -> PortFuture<'a, Result<Option<UserCredentials>, RepoError>>;

    /// The account `id`, if it exists.
    fn find_by_id(&self, id: UserId) -> PortFuture<'_, Result<Option<UserRecord>, RepoError>>;

    /// Stores memberships and marks onboarding done; `None` if `id` is gone.
    fn set_memberships(
        &self,
        id: UserId,
        memberships: Memberships,
    ) -> PortFuture<'_, Result<Option<UserRecord>, RepoError>>;

    /// Stores the preferred language; `None` if `id` is gone.
    fn set_locale(
        &self,
        id: UserId,
        locale: Locale,
    ) -> PortFuture<'_, Result<Option<UserRecord>, RepoError>>;
}

/// The password hasher could not hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("password could not be hashed")]
pub struct HashError;

/// Slow, salted password hashing (implemented in `api` with argon2).
pub trait PasswordHasher: Send + Sync {
    /// A PHC-format hash of `password`.
    fn hash<'a>(&'a self, password: &'a str) -> PortFuture<'a, Result<String, HashError>>;

    /// Whether `password` matches `hash`. A malformed hash never matches.
    fn verify<'a>(&'a self, password: &'a str, hash: &'a str) -> PortFuture<'a, bool>;

    /// A valid hash of a random secret, verified against when the e-mail is
    /// unknown so sign-in takes the same time either way.
    fn dummy_hash(&self) -> &str;
}

/// Id of a stored assistant match suggestion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SuggestionId(pub i64);

/// An assistant chat id: 8..=64 ASCII letters, digits or `-`, minted by the
/// api per chat request.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChatId(String);

impl ChatId {
    /// Validates a chat id.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        let ok = (8..=64).contains(&raw.len())
            && raw.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
        ok.then(|| Self(raw.to_owned()))
    }

    /// The id.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A match suggestion to store (made by the assistant, never applied).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSuggestion {
    /// Owning tenant.
    pub tenant: TenantId,
    /// Chat that produced it.
    pub chat_id: ChatId,
    /// Retailer of the listing.
    pub retailer: RetailerId,
    /// The retailer's product id.
    pub retailer_sku: RetailerSku,
    /// Amazon product of the match under review.
    pub asin: Asin,
    /// Listing title (copied from the tenant's stored result).
    pub title: String,
    /// Proposed verdict.
    pub decision: MatchDecision,
    /// Why.
    pub reason: SuggestionReason,
    /// When it was made.
    pub created_at: UnixMillis,
}

/// A stored match suggestion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuggestionView {
    /// Suggestion id.
    pub id: SuggestionId,
    /// Chat that produced it.
    pub chat_id: ChatId,
    /// Retailer of the listing.
    pub retailer: RetailerId,
    /// The retailer's product id.
    pub retailer_sku: RetailerSku,
    /// Amazon product of the match under review.
    pub asin: Asin,
    /// Listing title.
    pub title: String,
    /// Proposed verdict.
    pub decision: MatchDecision,
    /// Why.
    pub reason: String,
    /// When it was made.
    pub created_at: UnixMillis,
}

/// A user's verdict on a listing's match, scoped to their tenant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewDecision {
    /// Owning tenant.
    pub tenant: TenantId,
    /// Retailer of the listing.
    pub retailer: RetailerId,
    /// The retailer's product id.
    pub retailer_sku: RetailerSku,
    /// Amazon product the verdict is about.
    pub asin: Asin,
    /// Verdict.
    pub decision: MatchDecision,
    /// Who decided.
    pub decided_by: UserId,
    /// The assistant suggestion it confirms, if any.
    pub suggestion_id: Option<SuggestionId>,
    /// When.
    pub decided_at: UnixMillis,
}

/// A stored tenant verdict.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct StoredDecision {
    /// Retailer of the listing.
    pub retailer: RetailerId,
    /// The retailer's product id.
    pub retailer_sku: RetailerSku,
    /// Amazon product the verdict is about.
    pub asin: Asin,
    /// Verdict.
    pub decision: MatchDecision,
}

/// Per-tenant match suggestions and decisions (implemented by
/// `db::assistant::PgMatchDecisions`). The global match table is never
/// written through this port.
pub trait MatchDecisionRepo: Send + Sync {
    /// Stores a suggestion.
    fn create_suggestion<'a>(
        &'a self,
        s: &'a NewSuggestion,
    ) -> PortFuture<'a, Result<SuggestionId, RepoError>>;

    /// The tenant's suggestions from `chat_id`, oldest first.
    fn suggestions_for_chat<'a>(
        &'a self,
        tenant: &'a TenantId,
        chat_id: &'a ChatId,
    ) -> PortFuture<'a, Result<Vec<SuggestionView>, RepoError>>;

    /// The tenant's suggestion `id`; `None` if unknown or another tenant's.
    fn suggestion<'a>(
        &'a self,
        tenant: &'a TenantId,
        id: SuggestionId,
    ) -> PortFuture<'a, Result<Option<SuggestionView>, RepoError>>;

    /// Inserts or replaces the tenant's verdict on the listing.
    fn decide<'a>(&'a self, d: &'a NewDecision) -> PortFuture<'a, Result<(), RepoError>>;

    /// The tenant's verdicts about `asin`, sorted by retailer then SKU.
    fn decisions_for_asin<'a>(
        &'a self,
        tenant: &'a TenantId,
        asin: &'a Asin,
    ) -> PortFuture<'a, Result<Vec<StoredDecision>, RepoError>>;

    /// The tenant's verdicts on any of `listings`, sorted by retailer then
    /// SKU.
    fn decisions_for_listings<'a>(
        &'a self,
        tenant: &'a TenantId,
        listings: &'a [(RetailerId, RetailerSku)],
    ) -> PortFuture<'a, Result<Vec<StoredDecision>, RepoError>>;
}

/// Assistant tokens used per tenant and UTC day (implemented by
/// `db::assistant::PgAgentUsage`). `epoch_day` counts days since
/// 1970-01-01 UTC.
pub trait AgentUsageRepo: Send + Sync {
    /// Tokens the tenant used on `epoch_day`.
    fn used_on<'a>(
        &'a self,
        tenant: &'a TenantId,
        epoch_day: i64,
    ) -> PortFuture<'a, Result<TokenUsage, RepoError>>;

    /// Atomically adds `usage` and returns the day's new total.
    fn add<'a>(
        &'a self,
        tenant: &'a TenantId,
        epoch_day: i64,
        usage: TokenUsage,
    ) -> PortFuture<'a, Result<TokenUsage, RepoError>>;
}

/// One chat run handed to the assistant engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineRequest {
    /// Chat id (scopes suggestions and the tool grant).
    pub chat_id: ChatId,
    /// Signed-in user.
    pub user: UserId,
    /// Their tenant.
    pub tenant: TenantId,
    /// Reply language.
    pub locale: Locale,
    /// Validated history, last message from the user.
    pub history: ChatHistory,
    /// Tokens the tenant may still use today.
    pub token_budget: TokenCount,
}

/// What the engine answered (untrusted; cleaned by the use case).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineReply {
    /// Assistant text.
    pub reply: String,
    /// Why the run ended.
    pub stop: AgentStop,
    /// Tokens used by the run.
    pub usage: TokenUsage,
}

/// Why the assistant engine failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum EngineError {
    /// No engine configured, or it could not be reached.
    #[error("assistant unavailable")]
    Unavailable,
    /// The tenant already has a chat in flight.
    #[error("assistant busy")]
    Busy,
    /// The run took too long.
    #[error("assistant timed out")]
    Timeout,
    /// The run failed or answered something invalid.
    #[error("assistant failed")]
    Failed,
}

/// The AI assistant (implemented in `api` by the gg-agent sidecar client,
/// ADR 0013).
pub trait AssistantEngine: Send + Sync {
    /// Runs one chat.
    fn chat<'a>(
        &'a self,
        request: &'a EngineRequest,
    ) -> PortFuture<'a, Result<EngineReply, EngineError>>;
}
