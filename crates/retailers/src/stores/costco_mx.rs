//! Costco MX (`www.costco.com.mx`), an SAP Commerce (Spartacus, Angular
//! SSR) store. Search pages have no product JSON (the `storefront-state`
//! script holds only site context), so search reads the server-rendered
//! tiles; product pages have JSON-LD. Buying needs a membership, so every
//! offer is flagged.

use brightdata::TargetUrl;
use domain::{Offer, RetailerSku};
use scraper::{ElementRef, Html};

use crate::adapter::{AdapterError, Page, ParseError, Retailer, clean_query, encode_query, target};
use crate::parse::css::{self, TileSpec};
use crate::parse::{Draft, jsonld_draft, jsonld_drafts, meta_draft, product_from, search_from};
use crate::registry::{COSTCO_MX, RetailerInfo};

/// The Costco MX adapter.
pub(crate) struct CostcoMx;

/// The numeric product code after `/p/` in a product link.
fn code_from_href(_: ElementRef<'_>, href: Option<&str>) -> Option<String> {
    let path = href?;
    let at = path.rfind("/p/")?;
    let code = css::last_segment(&path[at..])?;
    code.bytes()
        .all(|b| b.is_ascii_digit())
        .then(|| code.to_owned())
}

const TILES: TileSpec = TileSpec {
    tile: &["li.product-list-item"],
    link: &["a.thumb"],
    title: &[],
    brand: &[],
    price: &[".product-price-amount"],
    list_price: &[],
    price_attr: None,
    sku: code_from_href,
};

fn css_search_drafts(doc: &Html) -> Vec<Draft> {
    css::tile_drafts(doc, &TILES)
}

/// JSON-LD with the seller ("Costco", sent as `brand`) removed.
fn jsonld_product_draft(doc: &Html) -> Draft {
    let mut d = jsonld_draft(doc);
    if d.brand
        .as_deref()
        .is_some_and(|b| b.eq_ignore_ascii_case("costco"))
    {
        d.brand = None;
    }
    d
}

impl Retailer for CostcoMx {
    fn info(&self) -> &'static RetailerInfo {
        &COSTCO_MX
    }

    fn search_url(&self, query: &str) -> Result<TargetUrl, AdapterError> {
        let q = clean_query(query)?;
        target(&format!(
            "https://www.costco.com.mx/search?text={}",
            encode_query(&q)
        ))
    }

    /// Spartacus resolves `/p/<code>` without the category path.
    fn product_url(&self, sku: &RetailerSku) -> Result<TargetUrl, AdapterError> {
        let id = sku.as_str();
        if id.is_empty() || id.len() > 12 || !id.bytes().all(|b| b.is_ascii_digit()) {
            return Err(AdapterError::InvalidSku);
        }
        target(&format!("https://www.costco.com.mx/p/{id}"))
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

    const SEARCH: &str = include_str!("../../tests/fixtures/costco_mx/search.html");
    const SEARCH_URL: &str = "https://www.costco.com.mx/search?text=licuadora";
    const PRODUCT: &str = include_str!("../../tests/fixtures/costco_mx/product.html");
    const PRODUCT_URL: &str = "https://www.costco.com.mx/Electrodomesticos/Electrodomesticos-de-Cocina/Licuadoras-y-Procesadores/Nutribullet-SmartSense-Licuadora-1400W/p/687400";

    #[test]
    fn selectors_parse_and_urls_build() {
        assert!(TILES.lists().iter().all(|l| css::all_parse(l)));
        assert_eq!(
            CostcoMx.search_url("licuadora").unwrap().as_str(),
            SEARCH_URL
        );
        assert_eq!(
            CostcoMx
                .product_url(&RetailerSku::new("687400").unwrap())
                .unwrap()
                .as_str(),
            "https://www.costco.com.mx/p/687400"
        );
        assert_eq!(
            code_from_href(
                Html::parse_fragment("<p></p>").root_element(),
                Some("/a/p/12?x")
            ),
            Some("12".into())
        );
    }

    #[test]
    fn search_fixture_parses_tiles_and_every_offer_needs_membership() {
        let offers = CostcoMx.parse_search(&page(SEARCH_URL, SEARCH)).unwrap();
        assert!(offers.len() >= 10, "{}", offers.len());
        assert!(offers.iter().all(|o| o.membership_required));
        let o = offers
            .iter()
            .find(|o| o.retailer_sku.as_str() == "687400")
            .unwrap();
        assert_eq!(o.retailer.as_str(), "costco_mx");
        assert_eq!(o.title, "Nutribullet, SmartSense Licuadora 1400W");
        assert_eq!(o.price_mxn.centavos(), 209_900);
        assert_eq!(o.url, PRODUCT_URL);
    }

    #[test]
    fn product_fixture_parses_from_json_ld() {
        let o = CostcoMx.parse_product(&page(PRODUCT_URL, PRODUCT)).unwrap();
        assert!(o.membership_required);
        assert_eq!(o.retailer_sku.as_str(), "687400");
        assert_eq!(o.title, "Nutribullet, SmartSense Licuadora 1400W");
        assert_eq!(o.brand, None);
        assert_eq!(o.price_mxn.centavos(), 259_900);
        assert_eq!(o.availability, Availability::InStock);
        assert_eq!(o.url, PRODUCT_URL);
    }
}
