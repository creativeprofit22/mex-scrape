//! HEB MX (`www.heb.com.mx`), a Next.js App Router store (page data is an
//! RSC stream, not JSON). Listings (`/search?q=`, category pages) are read
//! from their server-rendered product links; product pages have JSON-LD.
//! The SKU is the product URL slug (`/<slug>/p`), so search and product
//! offers share it and [`Retailer::product_url`] can rebuild the URL.

use brightdata::TargetUrl;
use domain::{Offer, RetailerSku};
use scraper::{ElementRef, Html};

use crate::adapter::{AdapterError, Page, ParseError, Retailer, clean_query, encode_query, target};
use crate::parse::css::{self, TileSpec};
use crate::parse::{Draft, jsonld_draft, jsonld_drafts, meta_draft, product_from, search_from};
use crate::registry::{HEB_MX, RetailerInfo};
use crate::stores::chedraui::vtex_product_url;

/// The HEB MX adapter.
pub(crate) struct HebMx;

/// `<slug>` of a `/<slug>/p` product path.
fn slug_of(href: &str) -> Option<String> {
    let end = href.find(['?', '#']).unwrap_or(href.len());
    let path = href[..end].strip_suffix("/p")?;
    let slug = path.rsplit('/').next()?;
    let ok = !slug.is_empty()
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    ok.then(|| slug.to_owned())
}

fn slug_from_href(_: ElementRef<'_>, href: Option<&str>) -> Option<String> {
    href.and_then(slug_of)
}

const TILES: TileSpec = TileSpec {
    tile: &[r#"a[href$="/p"]"#],
    link: &[],
    title: &["span.line-clamp-3"],
    brand: &[],
    price: &["span.text-xl"],
    list_price: &["span.line-through"],
    price_attr: None,
    sku: slug_from_href,
};

const CANONICAL: &[&str] = &[r#"link[rel="canonical"]"#];

fn css_search_drafts(doc: &Html) -> Vec<Draft> {
    css::tile_drafts(doc, &TILES)
}

/// JSON-LD `Product`, with the SKU replaced by the canonical URL slug.
fn jsonld_product_draft(doc: &Html) -> Draft {
    let mut d = jsonld_draft(doc);
    let canonical = css::first_attr(doc, CANONICAL, "href");
    if let Some(slug) = canonical.as_deref().and_then(slug_of) {
        d.sku = Some(slug);
        d.url = canonical;
    }
    d
}

impl Retailer for HebMx {
    fn info(&self) -> &'static RetailerInfo {
        &HEB_MX
    }

    fn search_url(&self, query: &str) -> Result<TargetUrl, AdapterError> {
        let q = clean_query(query)?;
        target(&format!(
            "https://www.heb.com.mx/search?q={}",
            encode_query(&q)
        ))
    }

    fn product_url(&self, sku: &RetailerSku) -> Result<TargetUrl, AdapterError> {
        vtex_product_url(HEB_MX.hosts[0], sku)
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

    const LISTING: &str = include_str!("../../tests/fixtures/heb_mx/search.html");
    const LISTING_URL: &str = "https://www.heb.com.mx/cafe";
    const PRODUCT: &str = include_str!("../../tests/fixtures/heb_mx/product.html");
    const PRODUCT_URL: &str =
        "https://www.heb.com.mx/tanto-mucho-cafe-de-olla-mezcla-1910-molido-500-gr-787233/p";
    const SLUG: &str = "tanto-mucho-cafe-de-olla-mezcla-1910-molido-500-gr-787233";

    #[test]
    fn selectors_parse_and_urls_build() {
        assert!(TILES.lists().iter().all(|l| css::all_parse(l)));
        assert!(css::all_parse(CANONICAL));
        assert_eq!(
            HebMx.search_url("café molido").unwrap().as_str(),
            "https://www.heb.com.mx/search?q=caf%C3%A9+molido"
        );
        assert_eq!(
            HebMx
                .product_url(&RetailerSku::new(SLUG).unwrap())
                .unwrap()
                .as_str(),
            PRODUCT_URL
        );
        assert_eq!(slug_of("/a/b-1/p?x").as_deref(), Some("b-1"));
        assert_eq!(slug_of("/cafe"), None);
    }

    #[test]
    fn listing_fixture_parses_tiles() {
        let offers = HebMx.parse_search(&page(LISTING_URL, LISTING)).unwrap();
        assert!(offers.len() >= 10, "{}", offers.len());
        let o = offers
            .iter()
            .find(|o| o.retailer_sku.as_str() == SLUG)
            .unwrap();
        assert_eq!(o.retailer.as_str(), "heb_mx");
        assert_eq!(
            o.title,
            "Café de Olla Tanto Mucho Mezcla 1910 Piloncillo, Canela, Anís y Chocolate 500 g"
        );
        assert_eq!(o.price_mxn.centavos(), 20_900);
        assert_eq!(o.url, PRODUCT_URL);
    }

    #[test]
    fn product_fixture_parses_from_json_ld() {
        let o = HebMx.parse_product(&page(PRODUCT_URL, PRODUCT)).unwrap();
        assert_eq!(o.retailer_sku.as_str(), SLUG);
        assert_eq!(o.brand.as_deref(), Some("TANTO MUCHO"));
        assert_eq!(o.price_mxn.centavos(), 20_900);
        assert_eq!(
            o.gtin.as_ref().map(domain::Gtin::as_str),
            Some("7500464373908")
        );
        assert_eq!(o.availability, Availability::InStock);
        assert_eq!(o.url, PRODUCT_URL);
    }
}
