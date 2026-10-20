//! Store adapters for Mexican retailers (Walmart MX, Liverpool, Chedraui,
//! and others) that turn fetched pages into normalized `domain::Offer`s.
//!
//! Each store implements [`Retailer`]: it builds search/product URLs and
//! parses pages, preferring JSON-LD, then page JSON, then CSS selectors.
//! Pages are fetched by the caller through `brightdata`; this crate does no
//! I/O. The registry lists hosts and sample pages for the access check.
//! [`RetailerOfferSource`] implements the `app::OfferSource` port over the
//! adapters with an injected [`PageFetcher`].

mod adapter;
pub mod category;
pub mod fixture;
pub mod health;
pub mod parse;
mod registry;
pub mod source;
mod stores;

pub use adapter::{
    AdapterError, MAX_QUERY_CHARS, Page, ParseError, ParseKind, Retailer, clean_query,
    encode_path_segment, encode_query, target,
};
pub use health::{HealthConfig, HealthConfigError, HealthStatus, ParseHealth, parse_page};
pub use registry::{AccessMode, PageKind, RetailerInfo, all, hosts, retailer_for_host};
pub use source::{FetchError, PageFetcher, RetailerOfferSource};

/// Every store adapter, in registry order.
#[must_use]
pub fn adapters() -> &'static [&'static dyn Retailer] {
    stores::ADAPTERS
}

/// The adapter for a retailer id, if any.
#[must_use]
pub fn adapter_for(id: &str) -> Option<&'static dyn Retailer> {
    adapters().iter().copied().find(|a| a.info().id == id)
}
