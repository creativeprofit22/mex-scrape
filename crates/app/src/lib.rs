//! Application layer for mex-scrape (ADR 0009): use cases and the ports
//! (traits) they need from the outside world.
//!
//! This crate does no I/O and depends only on `domain` plus non-I/O
//! libraries. Infrastructure crates (`db`, `retailers`, `amazon`, ...)
//! implement the ports; the entry points (`api`, `worker`) build those
//! adapters and inject them into the use cases.

pub mod access;
pub mod amazon;
pub mod ports;
pub mod price_stats;

pub use access::{AccessReport, AccessReportError, HostAccessReport};
pub use amazon::FallbackAmazonSource;
pub use ports::{
    AccessCountsRepo, AccessTotals, AmazonDataSource, AmazonSource, AmazonSourceError, CatalogItem,
    CatalogResult, Clock, FeeDetail, FeeEstimate, Fulfillment, HostAccess, MatchRepo, OfferSource,
    OfferSourceError, OfferSummary, PortFuture, PriceHistoryRepo, PricePoint, RepoError, SalesRank,
    UpsertOutcome,
};
pub use price_stats::{ListingPriceStats, PriceStats, PriceStatsError, ProductPriceStats};
