//! Core domain types and pure business logic for mex-scrape.
//!
//! This crate performs no I/O and depends on no other workspace crate; every
//! other crate may depend on it.

mod account;
mod amazon;
mod history;
mod matching;
mod offer;
mod roi;
mod search;

pub use account::{
    COSTCO_RETAILER_ID, Email, EmailError, Locale, MAX_EMAIL_BYTES, MAX_PASSWORD_CHARS,
    MIN_PASSWORD_CHARS, Memberships, PasswordPolicyError, Plan, SAMS_RETAILER_ID, UserId,
    check_new_password, is_membership_retailer,
};
pub use amazon::{Asin, AsinError};
pub use history::{DAY_MS, PriceSample, PriceWindow, STATS_WINDOWS, window_stats};
pub use matching::{Confidence, ConfidenceError, MatchMethod, MatchStatus, ProductMatch};
pub use offer::{
    Availability, Gtin, Offer, OfferError, PriceMxn, RetailerId, RetailerSku, UnixMillis,
};
pub use roi::{RoiError, RoiInput, RoiResult, calculate_roi};
pub use search::{
    MAX_SEARCH_QUERY_CHARS, RetailerSearchOutcome, SearchQuery, SearchQueryError, SearchStatus,
    TenantId, TenantIdError,
};

/// Name of the product, used in logs and the API.
pub const PRODUCT_NAME: &str = "mex-scrape";
