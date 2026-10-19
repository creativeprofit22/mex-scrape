//! Core domain types and pure business logic for mex-scrape.
//!
//! This crate performs no I/O and depends on no other workspace crate; every
//! other crate may depend on it.

mod amazon;
mod history;
mod matching;
mod offer;
mod roi;

pub use amazon::{Asin, AsinError};
pub use history::{DAY_MS, PriceSample, PriceWindow, STATS_WINDOWS, window_stats};
pub use matching::{Confidence, ConfidenceError, MatchMethod, MatchStatus, ProductMatch};
pub use offer::{
    Availability, Gtin, Offer, OfferError, PriceMxn, RetailerId, RetailerSku, UnixMillis,
};
pub use roi::{RoiError, RoiInput, RoiResult, calculate_roi};

/// Name of the product, used in logs and the API.
pub const PRODUCT_NAME: &str = "mex-scrape";
