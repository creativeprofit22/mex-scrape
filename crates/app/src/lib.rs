//! Application layer for mex-scrape (ADR 0009): use cases and the ports
//! (traits) they need from the outside world.
//!
//! This crate does no I/O and depends only on `domain` plus non-I/O
//! libraries. Infrastructure crates (`db`, `retailers`, `amazon`, ...)
//! implement the ports; the entry points (`api`, `worker`) build those
//! adapters and inject them into the use cases.

pub mod access;
pub mod ports;

pub use access::{AccessReport, AccessReportError, HostAccessReport};
pub use ports::{
    AccessCountsRepo, AccessTotals, AmazonListing, AmazonSource, AmazonSourceError, Clock,
    HostAccess, OfferSource, OfferSourceError, PortFuture, PriceHistoryRepo, PricePoint, RepoError,
};
