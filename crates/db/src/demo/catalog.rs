//! The demo catalog (`config/demo_catalog.json`, embedded at build time):
//! realistic Mexican retail products, the store listings that sell them and
//! the Amazon.com.mx product each one matches (or none).
//!
//! Two readers share it, both for local demos only: `seed_demo` writes a
//! year of price history for every listing and queues one search per
//! keyword group, and the worker's demo mode (`DEMO_DATA=true`) serves the
//! listings as store pages and the Amazon products as Bright Data dataset
//! records. Nothing here does I/O.

use std::collections::BTreeSet;

use domain::{Asin, Gtin, RetailerSku};
use serde::Deserialize;

/// The checked-in catalog.
pub const DEMO_CATALOG_JSON: &str = include_str!("../../../../config/demo_catalog.json");

/// How the worker is expected to match a product's listings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Expected {
    /// Same GTIN on the store page and on Amazon: auto-accepted.
    Accepted,
    /// No GTIN on the store page; the title is close but not certain.
    NeedsReview,
    /// No Amazon product at all.
    NoMatch,
}

/// The Amazon.com.mx side of a product.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DemoAmazon {
    /// Amazon product id.
    pub asin: String,
    /// Amazon title.
    pub title: String,
    /// Amazon brand.
    pub brand: String,
    /// Buy-box price in centavos.
    pub price_centavos: i64,
    /// FBA fulfilment fee in centavos (the referral fee is computed).
    pub fba_fee_centavos: i64,
}

/// Whether a store page shows the listing as buyable today.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListingStock {
    /// Buyable (the default when the catalog omits `availability`).
    #[default]
    InStock,
    /// Shown, but sold out.
    OutOfStock,
}

/// One store listing of a product.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DemoListing {
    /// Retailer id (`retailers` registry).
    pub retailer: String,
    /// The store's SKU.
    pub sku: String,
    /// Title on the store page.
    pub title: String,
    /// Today's price in centavos.
    pub price_centavos: i64,
    /// Stock on the store page today; its history ends out of stock too.
    #[serde(default)]
    pub availability: ListingStock,
}

/// One product.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DemoProduct {
    /// Stable id inside the catalog.
    pub key: String,
    /// Search words that find it (lowercase, no accents).
    pub keywords: Vec<String>,
    /// Brand on the store pages.
    pub brand: String,
    /// GTIN-13 on the store pages, when they show one.
    pub gtin: Option<String>,
    /// The Amazon product, if Amazon sells it.
    pub amazon: Option<DemoAmazon>,
    /// Expected match outcome (checked by the worker's tests).
    pub expected: Expected,
    /// Store listings.
    pub listings: Vec<DemoListing>,
}

/// The whole catalog.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DemoCatalog {
    /// Keyword groups (`cocina`, `electronica`, …): `seed_demo` queues one
    /// demo-account search per group. Each is a keyword of some product.
    pub groups: Vec<String>,
    /// Products in file order.
    pub products: Vec<DemoProduct>,
}

/// Why the catalog is unusable; names the product key only.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DemoCatalogError {
    /// Not valid JSON for [`DemoCatalog`].
    #[error("demo catalog is not valid JSON: {0}")]
    Json(String),
    /// A product breaks a rule.
    #[error("demo catalog product {key}: {problem}")]
    Invalid {
        /// The product's `key`.
        key: String,
        /// What is wrong.
        problem: &'static str,
    },
    /// A keyword group breaks a rule.
    #[error("demo catalog group {group}: {problem}")]
    Group {
        /// The group as written.
        group: String,
        /// What is wrong.
        problem: &'static str,
    },
}

impl DemoCatalog {
    /// Parses and validates the embedded catalog.
    ///
    /// # Errors
    ///
    /// [`DemoCatalogError`].
    pub fn embedded() -> Result<Self, DemoCatalogError> {
        Self::parse(DEMO_CATALOG_JSON)
    }

    /// Parses and validates `json`: unique keys and listings, valid GTINs,
    /// ASINs and SKUs, positive prices, an Amazon product unless the
    /// product is `no_match`, a GTIN exactly for `accepted` ones, and
    /// unique, trimmed groups that are each some product's keyword.
    ///
    /// # Errors
    ///
    /// [`DemoCatalogError`].
    pub fn parse(json: &str) -> Result<Self, DemoCatalogError> {
        let catalog: Self =
            serde_json::from_str(json).map_err(|e| DemoCatalogError::Json(e.to_string()))?;
        let mut keys = BTreeSet::new();
        let mut listings = BTreeSet::new();
        let mut asins = BTreeSet::new();
        for p in &catalog.products {
            let bad = |problem| DemoCatalogError::Invalid {
                key: p.key.clone(),
                problem,
            };
            if !keys.insert(p.key.as_str()) {
                return Err(bad("duplicate key"));
            }
            if p.keywords.is_empty() || p.listings.is_empty() || p.brand.trim().is_empty() {
                return Err(bad("needs keywords, a brand and listings"));
            }
            if p.gtin.as_deref().is_some_and(|g| Gtin::new(g).is_err()) {
                return Err(bad("invalid GTIN"));
            }
            if (p.expected == Expected::Accepted) != p.gtin.is_some() {
                return Err(bad("only accepted products carry a GTIN"));
            }
            match (&p.amazon, p.expected) {
                (None, Expected::NoMatch) => {}
                (Some(a), Expected::Accepted | Expected::NeedsReview) => {
                    if Asin::new(&a.asin).is_err() || !asins.insert(a.asin.as_str()) {
                        return Err(bad("invalid or duplicate ASIN"));
                    }
                    if a.price_centavos <= 0 || a.fba_fee_centavos < 0 {
                        return Err(bad("invalid Amazon price or fee"));
                    }
                }
                _ => return Err(bad("an Amazon product exactly unless no_match")),
            }
            for l in &p.listings {
                if RetailerSku::new(&l.sku).is_err() || l.price_centavos <= 0 {
                    return Err(bad("invalid listing SKU or price"));
                }
                if !listings.insert((l.retailer.as_str(), l.sku.as_str())) {
                    return Err(bad("duplicate listing"));
                }
            }
        }
        let mut groups = BTreeSet::new();
        for g in &catalog.groups {
            let bad = |problem| DemoCatalogError::Group {
                group: g.clone(),
                problem,
            };
            if g.is_empty() || g.trim() != g {
                return Err(bad("must be a non-empty trimmed word"));
            }
            if !groups.insert(g.as_str()) {
                return Err(bad("duplicate group"));
            }
            if catalog.group_products(g).next().is_none() {
                return Err(bad("no product has it as a keyword"));
            }
        }
        Ok(catalog)
    }

    /// The products whose keywords include `group`, in file order.
    pub fn group_products<'a>(&'a self, group: &'a str) -> impl Iterator<Item = &'a DemoProduct> {
        self.products
            .iter()
            .filter(move |p| p.keywords.iter().any(|k| k == group))
    }

    /// Every listing with its product, in file order.
    pub fn listings(&self) -> impl Iterator<Item = (&DemoProduct, &DemoListing)> {
        self.products
            .iter()
            .flat_map(|p| p.listings.iter().map(move |l| (p, l)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_checked_in_catalog_is_valid_and_mixed() {
        let catalog = DemoCatalog::embedded().unwrap();
        assert!(catalog.products.len() >= 20);
        for expected in [Expected::Accepted, Expected::NeedsReview, Expected::NoMatch] {
            assert!(catalog.products.iter().any(|p| p.expected == expected));
        }
        let stores: BTreeSet<&str> = catalog
            .listings()
            .map(|(_, l)| l.retailer.as_str())
            .collect();
        assert!(stores.len() >= 6, "{stores:?}");
        for group in ["cocina", "electronica", "hogar", "herramientas", "despensa"] {
            assert!(catalog.groups.iter().any(|g| g == group), "{group}");
        }
        let out = catalog
            .listings()
            .filter(|(_, l)| l.availability == ListingStock::OutOfStock)
            .count();
        assert!(out >= 3, "{out} out-of-stock listings");
    }

    #[test]
    fn groups_must_be_unique_keywords() {
        let with_groups = |groups: &str| {
            format!(
                r#"{{"groups":{groups},"products":[{{"key":"k","keywords":["x"],"brand":"B","gtin":null,
                "amazon":null,"expected":"no_match","listings":[{{"retailer":"liverpool","sku":"1","title":"t","price_centavos":100,"availability":"out_of_stock"}}]}}]}}"#
            )
        };
        let ok = DemoCatalog::parse(&with_groups(r#"["x"]"#)).unwrap();
        let (_, listing) = ok.listings().next().unwrap();
        assert_eq!(listing.availability, ListingStock::OutOfStock);
        assert_eq!(ok.group_products("x").count(), 1);
        for groups in [r#"["y"]"#, r#"["x","x"]"#, r#"[""]"#, r#"[" x"]"#] {
            let error = DemoCatalog::parse(&with_groups(groups)).unwrap_err();
            assert!(matches!(error, DemoCatalogError::Group { .. }), "{groups}");
        }
    }

    #[test]
    fn rule_breaks_are_rejected_by_key() {
        let one = |gtin: &str, expected: &str| {
            format!(
                r#"{{"groups":[],"products":[{{"key":"k","keywords":["x"],"brand":"B","gtin":{gtin},
                "amazon":{{"asin":"B0DEMO0001","title":"t","brand":"B","price_centavos":100,"fba_fee_centavos":0}},
                "expected":"{expected}","listings":[{{"retailer":"liverpool","sku":"1","title":"t","price_centavos":100}}]}}]}}"#
            )
        };
        assert!(DemoCatalog::parse(&one(r#""7509010001012""#, "accepted")).is_ok());
        for (gtin, expected) in [
            (r#""7509010001013""#, "accepted"),
            ("null", "accepted"),
            (r#""7509010001012""#, "needs_review"),
            ("null", "no_match"),
        ] {
            let error = DemoCatalog::parse(&one(gtin, expected)).unwrap_err();
            assert!(matches!(error, DemoCatalogError::Invalid { ref key, .. } if key == "k"));
        }
        assert!(matches!(
            DemoCatalog::parse(r#"{"groups":[],"products":[],"extra":1}"#),
            Err(DemoCatalogError::Json(_))
        ));
    }
}
