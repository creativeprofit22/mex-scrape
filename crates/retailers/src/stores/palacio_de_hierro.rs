//! El Palacio de Hierro (`www.elpalaciodehierro.com`), a Salesforce B2C
//! (SFCC) store. robots.txt disallows every URL with a query string, so
//! search (`/buscar?q=`) is refused and listings come from category pages,
//! which carry a JSON-LD `ItemList`. Product pages have no JSON-LD
//! `Product`, so they are read with CSS (then `og:` meta tags). The SKU is
//! the numeric id that ends every product URL (`...-45555106.html`).

use brightdata::TargetUrl;
use domain::{Offer, RetailerSku};
use scraper::Html;

use crate::adapter::{AdapterError, Page, ParseError, Retailer, clean_query, encode_query, target};
use crate::parse::css;
use crate::parse::{Draft, jsonld_draft, jsonld_drafts, meta_draft, product_from, search_from};
use crate::registry::{PALACIO_DE_HIERRO, RetailerInfo};

/// The El Palacio de Hierro adapter.
pub(crate) struct PalacioDeHierro;

/// The trailing numeric id of a product URL (`...-45555106.html`).
fn id_from_url(url: &str) -> Option<String> {
    let last = css::last_segment(url)?.strip_suffix(".html")?;
    let id = last.rsplit('-').next()?;
    (!id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())).then(|| id.to_owned())
}

/// JSON-LD `ItemList` products, with the SKU taken from each URL.
fn listing_drafts(doc: &Html) -> Vec<Draft> {
    jsonld_drafts(doc)
        .into_iter()
        .map(|mut d| {
            if d.sku.is_none() {
                d.sku = d.url.as_deref().and_then(id_from_url);
            }
            d
        })
        .collect()
}

const PDP_ROOT: &[&str] = &["[data-pid]"];
const PDP_TITLE: &[&str] = &["h1.b-product_main_info-name", "h1"];
const PDP_BRAND: &[&str] = &[".b-product_main_info-brand a", ".b-product_main_info-brand"];
const PDP_PRICE: &[&str] = &[".b-product_price-sales .b-product_price-value"];
const PDP_LIST_PRICE: &[&str] = &[".b-product_price-old .b-product_price-value"];
const CANONICAL: &[&str] = &[r#"link[rel="canonical"]"#];

/// CSS product source: the PDP info block (prices from `content`).
fn css_product_draft(doc: &Html) -> Draft {
    let root = doc.root_element();
    let price = css::child_price(root, PDP_PRICE, Some("content"));
    let list_price = css::child_price(root, PDP_LIST_PRICE, Some("content"))
        .filter(|l| price.is_some_and(|p| *l > p));
    let url = css::first_attr(doc, CANONICAL, "href");
    Draft {
        title: css::first_text(doc, PDP_TITLE),
        brand: css::first_text(doc, PDP_BRAND),
        price,
        list_price,
        sku: css::first_attr(doc, PDP_ROOT, "data-pid")
            .or_else(|| url.as_deref().and_then(id_from_url)),
        url,
        ..Draft::default()
    }
}

impl Retailer for PalacioDeHierro {
    fn info(&self) -> &'static RetailerInfo {
        &PALACIO_DE_HIERRO
    }

    /// Built for completeness; robots.txt disallows it, so the runner
    /// refuses it before any request.
    fn search_url(&self, query: &str) -> Result<TargetUrl, AdapterError> {
        let q = clean_query(query)?;
        target(&format!(
            "https://www.elpalaciodehierro.com/buscar?q={}",
            encode_query(&q)
        ))
    }

    /// SFCC resolves `/<id>.html` to the product.
    fn product_url(&self, sku: &RetailerSku) -> Result<TargetUrl, AdapterError> {
        let id = sku.as_str();
        if id.is_empty() || id.len() > 12 || !id.bytes().all(|b| b.is_ascii_digit()) {
            return Err(AdapterError::InvalidSku);
        }
        target(&format!("https://www.elpalaciodehierro.com/{id}.html"))
    }

    fn parse_search(&self, page: &Page) -> Result<Vec<Offer>, ParseError> {
        search_from(self.info(), page, &[listing_drafts])
    }

    fn parse_product(&self, page: &Page) -> Result<Offer, ParseError> {
        product_from(
            self.info(),
            page,
            &[jsonld_draft, css_product_draft, meta_draft],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::test_support::page;

    const LISTING: &str = include_str!("../../tests/fixtures/palacio_de_hierro/search.html");
    const LISTING_URL: &str = "https://www.elpalaciodehierro.com/hogar/electrodomesticos/";
    const PRODUCT: &str = include_str!("../../tests/fixtures/palacio_de_hierro/product.html");
    const PRODUCT_URL: &str = "https://www.elpalaciodehierro.com/kitchenaid-batidora-de-pedestal-sun-dried-tomato-artisan-plus-45555106.html";

    #[test]
    fn selectors_parse_and_urls_build() {
        for list in [
            PDP_ROOT,
            PDP_TITLE,
            PDP_BRAND,
            PDP_PRICE,
            PDP_LIST_PRICE,
            CANONICAL,
        ] {
            assert!(css::all_parse(list));
        }
        assert_eq!(id_from_url(PRODUCT_URL).as_deref(), Some("45555106"));
        assert_eq!(id_from_url("https://x/hogar/"), None);
        assert_eq!(
            PalacioDeHierro
                .product_url(&RetailerSku::new("45555106").unwrap())
                .unwrap()
                .as_str(),
            "https://www.elpalaciodehierro.com/45555106.html"
        );
    }

    #[test]
    fn listing_fixture_parses_from_json_ld_item_list() {
        let offers = PalacioDeHierro
            .parse_search(&page(LISTING_URL, LISTING))
            .unwrap();
        assert!(offers.len() >= 20, "{}", offers.len());
        let o = &offers[0];
        assert_eq!(o.retailer.as_str(), "palacio_de_hierro");
        assert_eq!(o.retailer_sku.as_str(), "45555106");
        assert_eq!(
            o.title,
            "Batidora de Pedestal Sun Dried Tomato Artisan Plus"
        );
        assert_eq!(o.brand.as_deref(), Some("KitchenAid"));
        assert_eq!(o.price_mxn.centavos(), 929_940);
        assert_eq!(o.url, PRODUCT_URL);
    }

    #[test]
    fn product_fixture_parses_with_css() {
        let o = PalacioDeHierro
            .parse_product(&page(PRODUCT_URL, PRODUCT))
            .unwrap();
        assert_eq!(o.retailer_sku.as_str(), "45555106");
        assert_eq!(
            o.title,
            "Batidora de Pedestal Sun Dried Tomato Artisan Plus"
        );
        assert_eq!(o.brand.as_deref(), Some("KitchenAid"));
        assert_eq!(o.price_mxn.centavos(), 929_940);
        assert_eq!(o.list_price_mxn.map(|p| p.centavos()), Some(1_549_900));
        assert_eq!(o.url, PRODUCT_URL);
    }
}
