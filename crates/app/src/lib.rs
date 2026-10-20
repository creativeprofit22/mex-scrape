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
pub mod ports;
pub mod price_series;
pub mod price_stats;
pub mod roi;
pub mod search;

pub use access::{AccessReport, AccessReportError, HostAccessReport};
pub use account::{
    Profile, ProfileError, SignIn, SignInError, SignUp, SignUpError, UpdateOnboarding,
};
pub use amazon::{FallbackAmazonSource, GatedAmazonSource};
pub use ports::{
    AccessCountsRepo, AccessTotals, AmazonDataSource, AmazonSource, AmazonSourceError, BudgetError,
    CatalogItem, CatalogResult, Clock, CreateUserError, FeeDetail, FeeEstimate, Fulfillment,
    HashError, HostAccess, MatchRepo, OfferSource, OfferSourceError, OfferSummary, PasswordHasher,
    PortFuture, PriceHistoryRepo, PricePoint, RepoError, RequestBudget, ResultMatchView,
    RetailerOutcomeView, SalesRank, SearchId, SearchRepo, SearchResultView, SearchView,
    UpsertOutcome, UserCredentials, UserRecord, UserRepo,
};
pub use price_series::{
    DailyPrice, ListingPriceSeries, MAX_SERIES_DAYS, PriceSeries, PriceSeriesError,
    ProductPriceSeries,
};
pub use price_stats::{ListingPriceStats, PriceStats, PriceStatsError, ProductPriceStats};
pub use roi::{MAX_ROI_BATCH, RoiBatch, RoiBatchError};
pub use search::{SearchError, SearchResults, StartSearch};
