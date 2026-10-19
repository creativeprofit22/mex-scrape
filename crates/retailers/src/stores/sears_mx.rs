//! Sears MX (`www.sears.com.mx`), a Claro Shop storefront (see
//! [`super::claro`]). robots.txt disallows `/resultados/*` (search), so
//! listings are category pages (`/cat/<path>?id=<id>`). They load their
//! products with JavaScript: the Unlocker HTML has none, so they are fetched
//! with the Browser API and read from the rendered product cards. Product
//! pages carry their data in `__NEXT_DATA__` and JSON-LD.

use brightdata::TargetUrl;
use domain::{Offer, RetailerSku};

use crate::adapter::{AdapterError, Page, ParseError, Retailer, clean_query};
use crate::parse::{jsonld_draft, jsonld_drafts, meta_draft, product_from, search_from};
use crate::registry::{RetailerInfo, SEARS_MX};
use crate::stores::claro;

/// The Sears MX adapter.
pub(crate) struct SearsMx;

impl Retailer for SearsMx {
    fn info(&self) -> &'static RetailerInfo {
        &SEARS_MX
    }

    /// Category-only: robots.txt disallows `/resultados/*`.
    fn search_url(&self, query: &str) -> Result<TargetUrl, AdapterError> {
        clean_query(query)?;
        Err(AdapterError::SearchUnavailable)
    }

    fn product_url(&self, sku: &RetailerSku) -> Result<TargetUrl, AdapterError> {
        claro::product_url(SEARS_MX.hosts[0], sku)
    }

    fn parse_search(&self, page: &Page) -> Result<Vec<Offer>, ParseError> {
        search_from(
            self.info(),
            page,
            &[
                jsonld_drafts,
                claro::next_search_drafts,
                claro::css_search_drafts,
            ],
        )
    }

    /// Page JSON before JSON-LD here: the JSON-LD `sku` is the seller's
    /// code (not the product id the URLs use) and its offer URL omits
    /// `/producto/`.
    fn parse_product(&self, page: &Page) -> Result<Offer, ParseError> {
        product_from(
            self.info(),
            page,
            &[claro::next_product_draft, jsonld_draft, meta_draft],
        )
    }
}

#[cfg(test)]
mod tests {
    use domain::Availability;

    use super::*;
    use crate::parse::test_support::page;

    /// Browser API capture of the category listing.
    const LISTING: &str = include_str!("../../tests/fixtures/sears_mx/search.html");
    /// Web Unlocker capture of the same listing (evidence, see the docs).
    const LISTING_UNLOCKER: &str =
        include_str!("../../tests/fixtures/sears_mx/search_unlocker.html");
    const LISTING_URL: &str = "https://www.sears.com.mx/cat/l%C3%ADnea%20blanca%20y%20electrodom%C3%A9sticos/electrodom%C3%A9sticos%20de%20cocina/licuadoras%20y%20picadoras?id=4714";
    const PRODUCT: &str = include_str!("../../tests/fixtures/sears_mx/product.html");
    const PRODUCT_URL: &str = "https://www.sears.com.mx/producto/306869/cafetera-programable-koblenz-ckm-212-pin-1-5-l-negro-end2";

    #[test]
    fn urls() {
        assert_eq!(
            SearsMx.search_url("licuadora oster"),
            Err(AdapterError::SearchUnavailable)
        );
        assert_eq!(SearsMx.search_url(" "), Err(AdapterError::EmptyQuery));
        assert_eq!(
            SearsMx.search_url(&"a".repeat(crate::adapter::MAX_QUERY_CHARS + 1)),
            Err(AdapterError::QueryTooLong)
        );
    }

    #[test]
    fn listing_fixture_has_no_product_data_without_javascript() {
        // Evidence for the Browser API: the Unlocker HTML of a category
        // listing has no product JSON-LD, page JSON or product cards.
        assert_eq!(
            SearsMx.parse_search(&page(LISTING_URL, LISTING_UNLOCKER)),
            Err(ParseError::NoOffers)
        );
    }

    #[test]
    fn rendered_listing_parses_from_product_cards() {
        let offers = SearsMx.parse_search(&page(LISTING_URL, LISTING)).unwrap();
        assert_eq!(offers.len(), 100);
        let o = &offers[0];
        assert_eq!(o.retailer.as_str(), "sears_mx");
        assert_eq!(o.retailer_sku.as_str(), "1003912");
        assert_eq!(
            o.title,
            "Licuadora Profesional Negro 1 Velocidad Oster Blstbps"
        );
        assert_eq!(o.price_mxn.centavos(), 199_900);
        assert_eq!(o.list_price_mxn.map(|p| p.centavos()), Some(235_900));
        assert_eq!(
            o.url,
            "https://www.sears.com.mx/producto/1003912/licuadora-profesional-negro-1-velocidad-oster-blstbps/"
        );
        assert!(!o.membership_required);
    }

    #[test]
    fn product_fixture_parses_from_next_data() {
        let o = SearsMx.parse_product(&page(PRODUCT_URL, PRODUCT)).unwrap();
        assert_eq!(o.retailer.as_str(), "sears_mx");
        assert_eq!(o.retailer_sku.as_str(), "306869");
        assert_eq!(
            o.title,
            "Cafetera Programable Koblenz CKM-212 PIN 1.5 L Negro END2"
        );
        assert_eq!(o.brand.as_deref(), Some("KOBLENZ"));
        assert_eq!(o.price_mxn.centavos(), 88_900);
        assert_eq!(o.list_price_mxn.map(|p| p.centavos()), Some(148_900));
        assert_eq!(o.availability, Availability::InStock);
        // The page's EAN is the all-zero placeholder, so no GTIN.
        assert_eq!(o.gtin, None);
        assert_eq!(o.url, PRODUCT_URL);
        assert!(!o.membership_required);
    }

    #[test]
    fn product_falls_back_to_json_ld() {
        let html = PRODUCT.replace("__NEXT_DATA__", "__GONE__");
        let o = SearsMx.parse_product(&page(PRODUCT_URL, &html)).unwrap();
        assert_eq!(o.price_mxn.centavos(), 88_900);
        assert_eq!(o.brand.as_deref(), Some("KOBLENZ"));
    }
}
