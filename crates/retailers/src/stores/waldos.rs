//! Waldo's (`waldos.com.mx`), a Magento 2 store. robots.txt disallows
//! `/catalogsearch/` (search), so listings come from category pages, read
//! from Magento's product tiles (prices from `data-price-amount`); product
//! pages have JSON-LD. The SKU is the URL key (`/<url-key>`), which search
//! and product pages share.

use brightdata::TargetUrl;
use domain::{Offer, RetailerSku};
use scraper::{ElementRef, Html};

use crate::adapter::{AdapterError, Page, ParseError, Retailer, clean_query, target};
use crate::parse::css::{self, TileSpec};
use crate::parse::{Draft, jsonld_draft, jsonld_drafts, meta_draft, product_from, search_from};
use crate::registry::{RetailerInfo, WALDOS};

/// The Waldo's adapter.
pub(crate) struct Waldos;

/// A Magento URL key: lowercase letters, digits and `-`.
fn url_key(href: &str) -> Option<String> {
    let key = css::last_segment(href)?;
    let key = key.strip_suffix(".html").unwrap_or(key);
    let ok = !key.is_empty()
        && key
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    ok.then(|| key.to_owned())
}

fn key_from_href(_: ElementRef<'_>, href: Option<&str>) -> Option<String> {
    href.and_then(url_key)
}

const TILES: TileSpec = TileSpec {
    tile: &["li.product-item"],
    link: &["a.product-item-link"],
    title: &["a.product-item-link"],
    brand: &[],
    price: &[r#"[data-price-type="finalPrice"]"#],
    list_price: &[r#"[data-price-type="oldPrice"]"#],
    price_attr: Some("data-price-amount"),
    sku: key_from_href,
};

const CANONICAL: &[&str] = &[r#"link[rel="canonical"]"#];

fn css_search_drafts(doc: &Html) -> Vec<Draft> {
    css::tile_drafts(doc, &TILES)
}

/// JSON-LD `Product`, with the SKU replaced by the canonical URL key.
fn jsonld_product_draft(doc: &Html) -> Draft {
    let mut d = jsonld_draft(doc);
    let canonical = css::first_attr(doc, CANONICAL, "href");
    if let Some(key) = canonical.as_deref().and_then(url_key) {
        d.sku = Some(key);
        d.url = canonical;
    }
    d
}

impl Retailer for Waldos {
    fn info(&self) -> &'static RetailerInfo {
        &WALDOS
    }

    /// Category-only: robots.txt disallows `/catalogsearch/`.
    fn search_url(&self, query: &str) -> Result<TargetUrl, AdapterError> {
        clean_query(query)?;
        Err(AdapterError::SearchUnavailable)
    }

    fn product_url(&self, sku: &RetailerSku) -> Result<TargetUrl, AdapterError> {
        let key = url_key(sku.as_str()).ok_or(AdapterError::InvalidSku)?;
        if key != sku.as_str() {
            return Err(AdapterError::InvalidSku);
        }
        target(&format!("https://waldos.com.mx/{key}"))
    }

    fn parse_search(&self, page: &Page) -> Result<Vec<Offer>, ParseError> {
        search_from(self.info(), page, &[jsonld_drafts, css_search_drafts])
    }

    fn parse_product(&self, page: &Page) -> Result<Offer, ParseError> {
        product_from(self.info(), page, &[jsonld_product_draft, meta_draft])
    }
}

#[cfg(test)]
mod tests {
    use domain::Availability;

    use super::*;
    use crate::parse::test_support::page;

    const LISTING: &str = include_str!("../../tests/fixtures/waldos/search.html");
    const LISTING_URL: &str = "https://waldos.com.mx/electrodomesticos/electrodomesticos-de-cocina";
    const PRODUCT: &str = include_str!("../../tests/fixtures/waldos/product.html");
    const PRODUCT_URL: &str = "https://waldos.com.mx/crepera-electrica-turmix-gris-cp-3";

    #[test]
    fn selectors_parse_and_urls_build() {
        assert!(TILES.lists().iter().all(|l| css::all_parse(l)));
        assert_eq!(
            Waldos.search_url("crepera"),
            Err(AdapterError::SearchUnavailable)
        );
        assert_eq!(Waldos.search_url(""), Err(AdapterError::EmptyQuery));
        assert_eq!(
            Waldos
                .product_url(&RetailerSku::new("crepera-electrica-turmix-gris-cp-3").unwrap())
                .unwrap()
                .as_str(),
            PRODUCT_URL
        );
        assert!(
            Waldos
                .product_url(&RetailerSku::new("a/b").unwrap())
                .is_err()
        );
        assert_eq!(
            url_key("https://waldos.com.mx/x-1.html?y").as_deref(),
            Some("x-1")
        );
    }

    #[test]
    fn listing_fixture_parses_tiles() {
        let offers = Waldos.parse_search(&page(LISTING_URL, LISTING)).unwrap();
        assert!(offers.len() >= 20, "{}", offers.len());
        let o = &offers[0];
        assert_eq!(o.retailer.as_str(), "waldos");
        assert_eq!(
            o.retailer_sku.as_str(),
            "crepera-electrica-turmix-gris-cp-3"
        );
        assert_eq!(o.title, "Crepera Eléctrica Turmix Gris CP-3");
        assert_eq!(o.price_mxn.centavos(), 104_699);
        assert_eq!(o.url, PRODUCT_URL);
        assert!(offers.iter().any(|o| o.list_price_mxn.is_some()));
    }

    #[test]
    fn product_fixture_parses_from_json_ld() {
        let o = Waldos.parse_product(&page(PRODUCT_URL, PRODUCT)).unwrap();
        assert_eq!(
            o.retailer_sku.as_str(),
            "crepera-electrica-turmix-gris-cp-3"
        );
        assert_eq!(o.title, "Crepera Eléctrica Turmix Gris CP-3");
        assert_eq!(o.price_mxn.centavos(), 104_699);
        assert_eq!(o.availability, Availability::InStock);
        assert_eq!(o.url, PRODUCT_URL);
    }
}
