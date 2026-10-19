//! The `Retailer` adapter trait, the fetched `Page` it parses, and errors.

use brightdata::TargetUrl;
use domain::{Offer, RetailerSku, UnixMillis};

use crate::registry::RetailerInfo;

/// A fetched page handed to a parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    /// The URL that was fetched (used to resolve relative links).
    pub url: TargetUrl,
    /// The HTML body.
    pub body: String,
    /// When it was fetched.
    pub fetched_at: UnixMillis,
}

/// Which parser a page goes through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ParseKind {
    /// A search or category listing (many offers).
    Search,
    /// A single product page (one offer).
    Product,
}

impl ParseKind {
    /// Stable lowercase name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Search => "search",
            Self::Product => "product",
        }
    }
}

/// Why an adapter could not build a URL.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AdapterError {
    /// The search query was blank.
    #[error("search query is empty")]
    EmptyQuery,
    /// The search query was longer than [`MAX_QUERY_CHARS`].
    #[error("search query is too long")]
    QueryTooLong,
    /// The SKU does not have this store's shape.
    #[error("SKU does not match this retailer's format")]
    InvalidSku,
    /// The built URL failed validation.
    #[error("built URL is invalid")]
    InvalidUrl,
    /// The store has no robots-allowed keyword search; use category listings.
    #[error("keyword search is not available for this retailer")]
    SearchUnavailable,
}

/// Why a page could not be parsed. Messages never echo page content.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    /// The page URL is not on one of the retailer's hosts.
    #[error("page host does not belong to this retailer")]
    WrongHost,
    /// No offer could be extracted.
    #[error("no offers found")]
    NoOffers,
    /// A required field was missing everywhere it was looked for.
    #[error("missing field: {0}")]
    MissingField(&'static str),
    /// A field was present but invalid.
    #[error("invalid field: {0}")]
    InvalidField(&'static str),
}

/// Longest accepted search query, in characters.
pub const MAX_QUERY_CHARS: usize = 100;

/// One store adapter: builds URLs and parses that store's pages into offers.
///
/// Parsers prefer embedded JSON-LD, then page JSON, then CSS selectors.
pub trait Retailer: Send + Sync {
    /// Registry entry (id, hosts, access mode, membership flag).
    fn info(&self) -> &'static RetailerInfo;

    /// URL of the search page for `query`, or a listing page where search is
    /// blocked. Returns [`AdapterError::SearchUnavailable`] when the store has
    /// neither (category listings only).
    ///
    /// # Errors
    /// [`AdapterError`] for a blank or too-long query, or
    /// [`AdapterError::SearchUnavailable`] for a category-only store.
    fn search_url(&self, query: &str) -> Result<TargetUrl, AdapterError>;

    /// URL of the product page for a retailer SKU.
    ///
    /// # Errors
    /// [`AdapterError::InvalidSku`] if the SKU has the wrong shape.
    fn product_url(&self, sku: &RetailerSku) -> Result<TargetUrl, AdapterError>;

    /// Every offer on a search/listing page (items that fail to parse are skipped).
    ///
    /// # Errors
    /// [`ParseError::NoOffers`] if none parse, [`ParseError::WrongHost`].
    fn parse_search(&self, page: &Page) -> Result<Vec<Offer>, ParseError>;

    /// The offer on a product page.
    ///
    /// # Errors
    /// [`ParseError`] if a required field is missing.
    fn parse_product(&self, page: &Page) -> Result<Offer, ParseError>;
}

/// Trims a query and checks it is non-empty and short enough.
///
/// # Errors
/// [`AdapterError::EmptyQuery`] or [`AdapterError::QueryTooLong`].
pub fn clean_query(query: &str) -> Result<String, AdapterError> {
    let q = query.split_whitespace().collect::<Vec<_>>().join(" ");
    if q.is_empty() {
        Err(AdapterError::EmptyQuery)
    } else if q.chars().count() > MAX_QUERY_CHARS {
        Err(AdapterError::QueryTooLong)
    } else {
        Ok(q)
    }
}

/// Percent-encodes a query-string value (`application/x-www-form-urlencoded`).
#[must_use]
pub fn encode_query(value: &str) -> String {
    encode(value, true)
}

/// Percent-encodes one URL path segment.
#[must_use]
pub fn encode_path_segment(value: &str) -> String {
    encode(value, false)
}

fn encode(value: &str, plus_for_space: bool) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(b));
            }
            b' ' if plus_for_space => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Builds a [`TargetUrl`], mapping failure to [`AdapterError::InvalidUrl`].
///
/// # Errors
/// [`AdapterError::InvalidUrl`].
pub fn target(url: &str) -> Result<TargetUrl, AdapterError> {
    TargetUrl::parse(url).map_err(|_| AdapterError::InvalidUrl)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_is_trimmed_collapsed_and_bounded() {
        assert_eq!(
            clean_query("  licuadora   oster ").unwrap(),
            "licuadora oster"
        );
        assert_eq!(clean_query(" \t"), Err(AdapterError::EmptyQuery));
        assert_eq!(
            clean_query(&"a".repeat(MAX_QUERY_CHARS + 1)),
            Err(AdapterError::QueryTooLong)
        );
    }

    #[test]
    fn encoding_escapes_reserved_and_unicode() {
        assert_eq!(encode_query("café & té/1"), "caf%C3%A9+%26+t%C3%A9%2F1");
        assert_eq!(encode_path_segment("a b?#"), "a%20b%3F%23");
    }
}
