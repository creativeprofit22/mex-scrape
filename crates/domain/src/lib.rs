//! Core domain types and pure business logic for mex-scrape.
//!
//! This crate performs no I/O and depends on no other workspace crate; every
//! other crate may depend on it.

/// Name of the product, used in logs and the API.
pub const PRODUCT_NAME: &str = "mex-scrape";
