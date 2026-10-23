//! Demo mode (`DEMO_DATA=true`, ADR 0016): a fixture-backed stand-in for
//! the Bright Data connector, so search jobs run the real worker → db →
//! API path against the products in `config/demo_catalog.json` with no
//! network call at all.
//!
//! - [`DemoTransport`] (`transport.rs`) implements [`brightdata::Transport`]. Web Unlocker
//!   and Browser fetches of a registered store's search URL return a page
//!   with one JSON-LD `Product` (in or out of stock, as the catalog says) per catalog listing of that store whose
//!   keyword appears in the URL; a product URL returns that listing's page.
//!   Datasets v3 triggers for the Amazon fallback answer at once with
//!   Amazon.com.mx records (GTIN or keyword discovery, product URL).
//! - [`DemoFees`] (`fees.rs`) answers Amazon fee estimates, which the Bright Data
//!   fallback cannot (`Unsupported`): a 15 % referral fee plus the
//!   catalog's FBA fee. Every other lookup is `NotConfigured`, so
//!   [`app::FallbackAmazonSource`] sends it on to the Bright Data fallback.
//!
//! Runs still go through `brightdata::runs::Runner`, so the kill switch,
//! the connector flag, the caps and the run records all apply.

mod fees;
#[cfg(test)]
mod tests;
mod transport;

pub use fees::{DemoFees, REFERRAL_FEE_PERCENT};
pub use transport::DemoTransport;
