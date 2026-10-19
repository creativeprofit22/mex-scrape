//! Liverpool (`www.liverpool.com.mx`). Next.js App Router: page data is an
//! RSC stream (not JSON), so search falls through to CSS product cards.

use brightdata::TargetUrl;
use domain::{Offer, RetailerSku};
use scraper::Html;

use crate::adapter::{AdapterError, Page, ParseError, Retailer, clean_query, encode_query, target};
use crate::parse::{
    Draft, clean_text, compact, css, jsonld_draft, jsonld_drafts, meta_draft, price, product_from,
    search_from,
};
use crate::registry::{LIVERPOOL, RetailerInfo};

/// The Liverpool adapter.
pub(crate) struct Liverpool;

const CARD: &[&str] = &[r#"a[data-testid$="-card-card-link"]"#];
const TITLE: &[&str] = &["h3"];
const BRAND: &[&str] = &["h4"];
const PRICE: &[&str] = &[
    r#"[data-testid="discounted"]"#,
    r#"[data-testid$="-price"]"#,
];
const LIST_PRICE: &[&str] = &[r#"[data-testid="original"]"#];

fn amount(el: scraper::ElementRef<'_>, list: &[&str]) -> Option<domain::PriceMxn> {
    css::selectors(list)
        .iter()
        .find_map(|s| el.select(s).next())
        .and_then(|p| price::first_in_text(&compact(&p.text().collect::<String>())))
}

fn css_search_drafts(doc: &Html) -> Vec<Draft> {
    css::items(doc, CARD)
        .into_iter()
        .map(|card| {
            let sku = card
                .value()
                .attr("data-testid")
                .and_then(|t| t.strip_suffix("-card-card-link"))
                .and_then(clean_text);
            Draft {
                title: css::child_text(card, TITLE),
                brand: css::child_text(card, BRAND),
                price: amount(card, PRICE),
                list_price: amount(card, LIST_PRICE),
                sku,
                url: card.value().attr("href").and_then(clean_text),
                ..Draft::default()
            }
        })
        .collect()
}

const PDP_PRICE: &[&str] = &[
    r#"[data-testid$="-configurator-price"] [data-testid="discounted"]"#,
    r#"[data-testid$="-configurator-price"]"#,
];
const PDP_LIST_PRICE: &[&str] =
    &[r#"[data-testid$="-configurator-price"] [data-testid="original"]"#];
const PDP_TITLE: &[&str] = &["h1"];
const PDP_BRAND: &[&str] = &[r#"a[data-testid$="-brand-link"]"#];
const CANONICAL: &[&str] = &[r#"link[rel="canonical"]"#];

/// CSS fallback for product pages: configurator block plus canonical URL
/// (`/tienda/pdp/<slug>/<id>`, whose last segment is the SKU).
fn css_product_draft(doc: &Html) -> Draft {
    let root = doc.root_element();
    let url = css::first_attr(doc, CANONICAL, "href");
    let sku = url
        .as_deref()
        .and_then(|u| u.rsplit('/').next())
        .filter(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
        .map(str::to_owned);
    Draft {
        title: css::first_text(doc, PDP_TITLE),
        brand: css::first_text(doc, PDP_BRAND),
        price: amount(root, PDP_PRICE),
        list_price: amount(root, PDP_LIST_PRICE),
        sku,
        url,
        ..Draft::default()
    }
}

impl Retailer for Liverpool {
    fn info(&self) -> &'static RetailerInfo {
        &LIVERPOOL
    }

    fn search_url(&self, query: &str) -> Result<TargetUrl, AdapterError> {
        let q = clean_query(query)?;
        target(&format!(
            "https://www.liverpool.com.mx/tienda?s={}",
            encode_query(&q)
        ))
    }

    /// Liverpool routes `/tienda/pdp/<slug>/<id>` by id; the slug is cosmetic.
    fn product_url(&self, sku: &RetailerSku) -> Result<TargetUrl, AdapterError> {
        let id = sku.as_str();
        if id.is_empty() || id.len() > 20 || !id.bytes().all(|b| b.is_ascii_digit()) {
            return Err(AdapterError::InvalidSku);
        }
        target(&format!(
            "https://www.liverpool.com.mx/tienda/pdp/producto/{id}"
        ))
    }

    fn parse_search(&self, page: &Page) -> Result<Vec<Offer>, ParseError> {
        search_from(self.info(), page, &[jsonld_drafts, css_search_drafts])
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
    use crate::parse::test_support::{page, strip_json};

    const SEARCH: &str = include_str!("../../tests/fixtures/liverpool/search.html");
    const SEARCH_URL: &str = "https://www.liverpool.com.mx/tienda?s=licuadora";

    #[test]
    fn selectors_parse_and_urls_build() {
        for list in [
            CARD,
            TITLE,
            BRAND,
            PRICE,
            LIST_PRICE,
            PDP_PRICE,
            PDP_LIST_PRICE,
            PDP_TITLE,
            PDP_BRAND,
            CANONICAL,
        ] {
            assert!(css::all_parse(list));
        }
        assert_eq!(
            Liverpool.search_url("licuadora").unwrap().as_str(),
            SEARCH_URL
        );
        assert_eq!(
            Liverpool
                .product_url(&RetailerSku::new("1185461820").unwrap())
                .unwrap()
                .as_str(),
            "https://www.liverpool.com.mx/tienda/pdp/producto/1185461820"
        );
        assert!(
            Liverpool
                .product_url(&RetailerSku::new("x/1").unwrap())
                .is_err()
        );
    }

    #[test]
    fn search_fixture_parses_cards() {
        let offers = Liverpool.parse_search(&page(SEARCH_URL, SEARCH)).unwrap();
        assert!(offers.len() >= 50, "{}", offers.len());
        let o = &offers[0];
        assert_eq!(o.retailer.as_str(), "liverpool");
        assert_eq!(o.retailer_sku.as_str(), "1185461820");
        assert_eq!(o.title, "Licuadora TURBO MAX 10 velocidades");
        assert_eq!(o.brand.as_deref(), Some("TURMIX"));
        assert_eq!(o.price_mxn.centavos(), 187_425);
        assert_eq!(o.list_price_mxn.map(|p| p.centavos()), Some(249_900));
        assert_eq!(
            o.url,
            "https://www.liverpool.com.mx/tienda/pdp/licuadora-turbo-max-10-velocidades/1185461820"
        );
        let plain = offers
            .iter()
            .find(|o| o.retailer_sku.as_str() == "1188741050")
            .unwrap();
        assert_eq!(plain.price_mxn.centavos(), 55_900);
        assert_eq!(plain.list_price_mxn, None);
    }

    const PRODUCT: &str = include_str!("../../tests/fixtures/liverpool/product.html");
    const PRODUCT_URL: &str =
        "https://www.liverpool.com.mx/tienda/pdp/licuadora-turbo-max-10-velocidades/1185461820";

    #[test]
    fn product_fixture_parses_with_css() {
        let o = Liverpool
            .parse_product(&page(PRODUCT_URL, PRODUCT))
            .unwrap();
        assert_eq!(o.retailer_sku.as_str(), "1185461820");
        assert_eq!(o.title, "Licuadora TURBO MAX 10 velocidades");
        assert_eq!(o.brand.as_deref(), Some("TURMIX"));
        assert_eq!(o.price_mxn.centavos(), 187_425);
        assert_eq!(o.list_price_mxn.map(|p| p.centavos()), Some(249_900));
        assert_eq!(o.url, PRODUCT_URL);
        assert_eq!(o.gtin, None);
    }

    #[test]
    fn css_is_used_when_no_json_exists() {
        // The fixture has no JSON at all; stripping is a no-op guard.
        let html = strip_json(SEARCH);
        assert_eq!(
            Liverpool
                .parse_search(&page(SEARCH_URL, &html))
                .unwrap()
                .len(),
            Liverpool
                .parse_search(&page(SEARCH_URL, SEARCH))
                .unwrap()
                .len()
        );
    }
}
