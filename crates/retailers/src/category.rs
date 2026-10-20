//! Configured category listing pages for stores without keyword search
//! (ADR 0008). The worker browses these on a schedule through the Browser
//! API; this module only validates the configuration.
//!
//! Format (`config/category_listings.json`):
//!
//! ```json
//! { "listings": [ { "retailer": "sears_mx", "url": "https://www.sears.com.mx/cat/...?id=4714" } ] }
//! ```
//!
//! Every entry must name a registered retailer whose
//! [`RetailerInfo::browser_pages`](crate::RetailerInfo) contains
//! [`PageKind::Category`], and its URL must be a full menu link: a valid
//! target URL on one of that retailer's hosts, with a path under `/cat/`
//! and a non-empty numeric `id=` query parameter. Error text names the
//! entry index only and never echoes the URL.

use brightdata::TargetUrl;
use serde_json::Value;

use crate::registry::{PageKind, RetailerInfo, all};

/// Most entries accepted in one configuration file.
pub const MAX_LISTINGS: usize = 500;

/// One validated category listing page.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CategoryListing {
    /// Registered retailer id.
    pub retailer: &'static str,
    /// The validated listing URL.
    pub url: TargetUrl,
}

/// Why a category configuration was rejected. Never contains the URL.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CategoryConfigError {
    /// The file is not a JSON object with a `listings` array.
    #[error("category listings: expected an object with a \"listings\" array")]
    Shape,
    /// More than [`MAX_LISTINGS`] entries.
    #[error("category listings: more than {MAX_LISTINGS} entries")]
    TooMany,
    /// An entry is not an object with exactly `retailer` and `url` strings.
    #[error("category listing {0}: expected {{\"retailer\", \"url\"}} strings")]
    Entry(usize),
    /// The retailer is not registered.
    #[error("category listing {0}: unknown retailer")]
    UnknownRetailer(usize),
    /// The retailer does not browse category listings through the Browser API.
    #[error("category listing {0}: retailer does not use Browser category listings")]
    NotCategoryRetailer(usize),
    /// The URL is not a valid target URL.
    #[error("category listing {0}: invalid URL")]
    InvalidUrl(usize),
    /// The URL host is not one of the retailer's hosts.
    #[error("category listing {0}: host does not belong to the retailer")]
    WrongHost(usize),
    /// The URL is not a full menu link (`/cat/...?id=<digits>`).
    #[error("category listing {0}: not a full menu link (/cat/...?id=<digits>)")]
    NotMenuLink(usize),
    /// The same URL appears twice.
    #[error("category listing {0}: duplicate URL")]
    Duplicate(usize),
}

/// Parses and validates a category configuration file. Entries keep file
/// order (the page cap takes the first ones per retailer).
///
/// # Errors
///
/// The first invalid entry, as a [`CategoryConfigError`].
pub fn parse_listings(json: &str) -> Result<Vec<CategoryListing>, CategoryConfigError> {
    let root: Value = serde_json::from_str(json).map_err(|_| CategoryConfigError::Shape)?;
    let obj = root.as_object().ok_or(CategoryConfigError::Shape)?;
    if obj.len() != 1 {
        return Err(CategoryConfigError::Shape);
    }
    let entries = obj
        .get("listings")
        .and_then(Value::as_array)
        .ok_or(CategoryConfigError::Shape)?;
    if entries.len() > MAX_LISTINGS {
        return Err(CategoryConfigError::TooMany);
    }
    let mut out: Vec<CategoryListing> = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let listing = parse_entry(index, entry)?;
        if out.iter().any(|l| l.url == listing.url) {
            return Err(CategoryConfigError::Duplicate(index));
        }
        out.push(listing);
    }
    Ok(out)
}

/// Retailers that may have category listings configured: those whose
/// category pages need the Browser API, in registry order.
#[must_use]
pub fn category_retailers() -> Vec<&'static RetailerInfo> {
    all()
        .iter()
        .filter(|r| r.browser_pages.contains(&PageKind::Category))
        .collect()
}

fn parse_entry(index: usize, entry: &Value) -> Result<CategoryListing, CategoryConfigError> {
    let obj = entry
        .as_object()
        .filter(|o| o.len() == 2)
        .ok_or(CategoryConfigError::Entry(index))?;
    let field = |name: &str| {
        obj.get(name)
            .and_then(Value::as_str)
            .ok_or(CategoryConfigError::Entry(index))
    };
    let retailer_id = field("retailer")?;
    let url_text = field("url")?;

    let info = all()
        .iter()
        .find(|r| r.id == retailer_id)
        .ok_or(CategoryConfigError::UnknownRetailer(index))?;
    if !info.browser_pages.contains(&PageKind::Category) {
        return Err(CategoryConfigError::NotCategoryRetailer(index));
    }
    let url = TargetUrl::parse(url_text).map_err(|_| CategoryConfigError::InvalidUrl(index))?;
    let host = url.host();
    if !info.hosts.iter().any(|h| h.eq_ignore_ascii_case(&host)) {
        return Err(CategoryConfigError::WrongHost(index));
    }
    if !is_menu_link(url.as_str()) {
        return Err(CategoryConfigError::NotMenuLink(index));
    }
    Ok(CategoryListing {
        retailer: info.id,
        url,
    })
}

/// True for `https://host/cat/<segment...>?...id=<digits>...` with no
/// fragment. `normalized` is a [`TargetUrl`] string (scheme and host
/// already validated and normalized).
fn is_menu_link(normalized: &str) -> bool {
    let Some((_, after_scheme)) = normalized.split_once("://") else {
        return false;
    };
    if !normalized.starts_with("https://") || after_scheme.contains('#') {
        return false;
    }
    let Some(path_start) = after_scheme.find('/') else {
        return false;
    };
    let rest = &after_scheme[path_start..];
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let Some(segments) = path.strip_prefix("/cat/") else {
        return false;
    };
    if segments.is_empty() || segments.split('/').any(str::is_empty) {
        return false;
    }
    query.split('&').any(|pair| {
        pair.strip_prefix("id=").is_some_and(|id| {
            !id.is_empty() && id.len() <= 12 && id.bytes().all(|b| b.is_ascii_digit())
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEARS: &str = "https://www.sears.com.mx/cat/videojuegos?id=16649";
    const SANBORNS: &str = "https://www.sanborns.com.mx/cat/videojuegos/xbox/consolas?id=120101";

    fn one(retailer: &str, url: &str) -> String {
        serde_json::json!({ "listings": [ { "retailer": retailer, "url": url } ] }).to_string()
    }

    #[test]
    fn accepts_full_menu_links_in_file_order() {
        let json = serde_json::json!({ "listings": [
            { "retailer": "sanborns", "url": SANBORNS },
            { "retailer": "sears_mx", "url": SEARS },
        ]})
        .to_string();
        let got = parse_listings(&json).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].retailer, "sanborns");
        assert_eq!(got[0].url.as_str(), SANBORNS);
        assert_eq!(got[1].retailer, "sears_mx");
    }

    #[test]
    fn the_checked_in_config_is_valid_and_covers_both_stores() {
        let json = include_str!("../../../config/category_listings.json");
        let got = parse_listings(json).unwrap();
        for id in ["sears_mx", "sanborns"] {
            assert!(got.iter().filter(|l| l.retailer == id).count() >= 3, "{id}");
        }
    }

    #[test]
    fn registry_samples_are_valid_menu_links() {
        for info in category_retailers() {
            let (_, url) = info
                .samples
                .iter()
                .find(|(k, _)| *k == PageKind::Category)
                .unwrap();
            assert!(parse_listings(&one(info.id, url)).is_ok(), "{}", info.id);
        }
    }

    #[test]
    fn category_retailers_are_sears_and_sanborns() {
        let ids: Vec<_> = category_retailers().iter().map(|r| r.id).collect();
        assert_eq!(ids, ["sears_mx", "sanborns"]);
    }

    #[test]
    fn rejects_a_host_of_another_retailer() {
        let err = parse_listings(&one("sears_mx", SANBORNS)).unwrap_err();
        assert_eq!(err, CategoryConfigError::WrongHost(0));
        let err = parse_listings(&one("sears_mx", "https://evil.example/cat/x?id=1")).unwrap_err();
        assert_eq!(err, CategoryConfigError::WrongHost(0));
    }

    #[test]
    fn rejects_links_that_are_not_full_menu_links() {
        for url in [
            "https://www.sears.com.mx/cat/videojuegos",
            "https://www.sears.com.mx/cat/videojuegos?id=",
            "https://www.sears.com.mx/cat/videojuegos?id=12a",
            "https://www.sears.com.mx/cat/videojuegos?ids=12",
            "https://www.sears.com.mx/c/videojuegos?id=12",
            "https://www.sears.com.mx/cat/?id=12",
            "https://www.sears.com.mx/cat//x?id=12",
            "https://www.sears.com.mx/cat/x?id=12#top",
            "http://www.sears.com.mx/cat/x?id=12",
            "https://www.sears.com.mx/resultados/q=licuadora/1",
        ] {
            assert_eq!(
                parse_listings(&one("sears_mx", url)).unwrap_err(),
                CategoryConfigError::NotMenuLink(0),
                "{url}"
            );
        }
        // Other query parameters are fine as long as id= is present.
        assert!(
            parse_listings(&one(
                "sears_mx",
                "https://www.sears.com.mx/cat/x?page=2&id=12"
            ))
            .is_ok()
        );
    }

    #[test]
    fn rejects_keyword_search_stores_and_unknown_retailers() {
        let err = parse_listings(&one("walmart_mx", "https://www.walmart.com.mx/cat/x?id=1"))
            .unwrap_err();
        assert_eq!(err, CategoryConfigError::NotCategoryRetailer(0));
        let err = parse_listings(&one("nope", SEARS)).unwrap_err();
        assert_eq!(err, CategoryConfigError::UnknownRetailer(0));
    }

    #[test]
    fn rejects_bad_shapes_duplicates_and_bad_urls() {
        assert_eq!(
            parse_listings("[]").unwrap_err(),
            CategoryConfigError::Shape
        );
        assert_eq!(
            parse_listings("not json").unwrap_err(),
            CategoryConfigError::Shape
        );
        assert_eq!(
            parse_listings(r#"{"listings":[],"extra":1}"#).unwrap_err(),
            CategoryConfigError::Shape
        );
        assert_eq!(
            parse_listings(r#"{"listings":[{"retailer":"sears_mx"}]}"#).unwrap_err(),
            CategoryConfigError::Entry(0)
        );
        assert_eq!(
            parse_listings(&one("sears_mx", "not a url")).unwrap_err(),
            CategoryConfigError::InvalidUrl(0)
        );
        let dup = serde_json::json!({ "listings": [
            { "retailer": "sears_mx", "url": SEARS },
            { "retailer": "sears_mx", "url": SEARS },
        ]})
        .to_string();
        assert_eq!(
            parse_listings(&dup).unwrap_err(),
            CategoryConfigError::Duplicate(1)
        );
        assert!(parse_listings(r#"{"listings":[]}"#).unwrap().is_empty());
    }

    #[test]
    fn errors_never_echo_the_url() {
        let secret = "https://www.sears.com.mx/c/token-abc123?id=1";
        let err = parse_listings(&one("sears_mx", secret)).unwrap_err();
        assert!(!err.to_string().contains("token-abc123"));
    }
}
