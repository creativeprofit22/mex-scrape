//! Application layer for mex-scrape (ADR 0009): use cases and the ports
//! (traits) they need from the outside world.
//!
//! This crate does no I/O and depends only on `domain` plus non-I/O
//! libraries. Infrastructure crates (`db`, `retailers`, `amazon`, ...)
//! implement the ports; the entry points (`api`, `worker`) build those
//! adapters and inject them into the use cases.

pub mod access;
pub mod account;
pub mod amazon;
pub mod assistant;
pub mod ports;
pub mod price_series;
pub mod price_stats;
pub mod product;
pub mod roi;
pub mod search;

pub use access::{AccessReport, AccessReportError, HostAccessReport};
pub use account::{
    Profile, ProfileError, SignIn, SignInError, SignUp, SignUpError, UpdateOnboarding,
};
pub use amazon::{FallbackAmazonSource, GatedAmazonSource};
pub use assistant::{
    AssistantChat, AssistantError, AssistantStorageError, ChatOutcome, DecideMatch,
    MAX_TRACKED_LIMIT, MatchReviewError, RecordAgentUsage, ReviewTarget, SuggestMatch,
    TRACKED_DAYS, TokenCaps, TrackedOffers, TrackedOffersView, TrackedQuery, epoch_day,
};
pub use ports::{
    AccessCountsRepo, AccessTotals, AgentUsageRepo, AmazonDataSource, AmazonProduct,
    AmazonProductRepo, AmazonSource, AmazonSourceError, AssistantEngine, BudgetError, CatalogItem,
    CatalogResult, ChatId, Clock, CreateUserError, EngineError, EngineReply, EngineRequest,
    FeeDetail, FeeEstimate, Fulfillment, HashError, HostAccess, MatchDecisionRepo, MatchRepo,
    NewDecision, NewSuggestion, OfferSource, OfferSourceError, OfferSummary, PasswordHasher,
    PortFuture, PriceHistoryRepo, PricePoint, RecentResultsFilter, RepoError, RequestBudget,
    ResultMatchView, RetailerOutcomeView, SalesRank, SearchId, SearchRepo, SearchResultView,
    SearchView, StoredDecision, SuggestionId, SuggestionView, TrackedResultView, UpsertOutcome,
    UserCredentials, UserRecord, UserRepo,
};
pub use price_series::{
    DailyPrice, ListingPriceSeries, MAX_SERIES_DAYS, PriceSeries, PriceSeriesError,
    ProductPriceSeries,
};
pub use price_stats::{ListingPriceStats, PriceStats, PriceStatsError, ProductPriceStats};
pub use product::{MAX_BRAND_CHARS, MAX_TITLE_CHARS, ProductName};
pub use roi::{MAX_ROI_BATCH, RoiBatch, RoiBatchError};
pub use search::{SearchError, SearchResults, StartSearch};
