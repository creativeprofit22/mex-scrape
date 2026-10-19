//! Sanborns (`www.sanborns.com.mx`), a Claro Shop storefront (see
//! [`super::claro`]). robots.txt disallows `/resultados/*` (search), so
//! listings are category pages (`/cat/<path>?id=<id>`). As on Sears, they
//! load their products with JavaScript: they are fetched with the Browser
//! API and read from the rendered product cards. The home page and product
//! pages carry product objects in `__NEXT_DATA__`.

use brightdata::TargetUrl;
use domain::{Offer, RetailerSku};

use crate::adapter::{AdapterError, Page, ParseError, Retailer, clean_query};
use crate::parse::{jsonld_draft, jsonld_drafts, meta_draft, product_from, search_from};
use crate::registry::{RetailerInfo, SANBORNS};
use crate::stores::claro;

/// The Sanborns adapter.
pub(crate) struct Sanborns;

impl Retailer for Sanborns {
    fn info(&self) -> &'static RetailerInfo {
        &SANBORNS
    }

    /// Category-only: robots.txt disallows `/resultados/*`.
    fn search_url(&self, query: &str) -> Result<TargetUrl, AdapterError> {
        clean_query(query)?;
        Err(AdapterError::SearchUnavailable)
    }

    fn product_url(&self, sku: &RetailerSku) -> Result<TargetUrl, AdapterError> {
        claro::product_url(SANBORNS.hosts[0], sku)
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

    /// Page JSON before JSON-LD, as for Sears: the JSON-LD `sku` is an
    /// internal code and its offer URL omits `/producto/`.
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

    /// The home page: its product carousels carry server-rendered data.
    const HOME: &str = include_str!("../../tests/fixtures/sanborns/home.html");
    const HOME_URL: &str = "https://www.sanborns.com.mx/";
    /// Browser API capture of a category listing.
    const LISTING: &str = include_str!("../../tests/fixtures/sanborns/search.html");
    /// Web Unlocker capture of the same listing (evidence, see the docs).
    const LISTING_UNLOCKER: &str =
        include_str!("../../tests/fixtures/sanborns/search_unlocker.html");
    const LISTING_URL: &str = "https://www.sanborns.com.mx/cat/tecnolog%C3%ADa%20y%20electr%C3%B3nica/tv%20y%20video/pantallas?id=100609";
    const PRODUCT: &str = include_str!("../../tests/fixtures/sanborns/product.html");
    const PRODUCT_URL: &str = "https://www.sanborns.com.mx/producto/968461/pantalla-samsung-de-75-pulgadas-un75u8200hf-uhd";

    #[test]
    fn urls() {
        assert_eq!(
            Sanborns.search_url("pantalla"),
            Err(AdapterError::SearchUnavailable)
        );
        assert_eq!(Sanborns.search_url(" "), Err(AdapterError::EmptyQuery));
    }

    #[test]
    fn listing_fixture_has_no_product_data_without_javascript() {
        assert_eq!(
            Sanborns.parse_search(&page(LISTING_URL, LISTING_UNLOCKER)),
            Err(ParseError::NoOffers)
        );
    }

    #[test]
    fn rendered_listing_parses_from_product_cards() {
        let offers = Sanborns.parse_search(&page(LISTING_URL, LISTING)).unwrap();
        assert_eq!(offers.len(), 100);
        let o = &offers[0];
        assert_eq!(o.retailer.as_str(), "sanborns");
        assert_eq!(o.retailer_sku.as_str(), "309566");
        assert_eq!(o.title, "Pantalla Samsung de 65 Pulgadas NEO QN65QN90CAF");
        assert_eq!(o.price_mxn.centavos(), 2_999_200);
        assert_eq!(o.list_price_mxn.map(|p| p.centavos()), Some(5_285_600));
        assert_eq!(
            o.url,
            "https://www.sanborns.com.mx/producto/309566/pantalla-samsung-de-65-pulgadas-neo-qn65qn90caf/"
        );
        // The grey card label is the seller, not the brand.
        assert_eq!(o.brand, None);
        assert!(!o.membership_required);
    }

    #[test]
    fn home_page_parses_from_next_data() {
        let offers = Sanborns.parse_search(&page(HOME_URL, HOME)).unwrap();
        assert!(offers.len() >= 20, "{}", offers.len());
        let o = &offers[0];
        assert_eq!(o.retailer.as_str(), "sanborns");
        assert!(o.url.starts_with("https://www.sanborns.com.mx/producto/"));
        assert!(o.retailer_sku.as_str().bytes().all(|b| b.is_ascii_digit()));
        assert!(o.price_mxn.centavos() > 0);
        let barbie = offers
            .iter()
            .find(|o| o.retailer_sku.as_str() == "999341")
            .unwrap();
        assert_eq!(
            barbie.title,
            "Muñeca de Colección Mattel Barbie Dia de Muertos Premium"
        );
        assert_eq!(barbie.brand.as_deref(), Some("BARBIE"));
        assert_eq!(barbie.price_mxn.centavos(), 299_900);
        assert_eq!(barbie.list_price_mxn, None);
        assert_eq!(
            barbie.gtin.as_ref().map(domain::Gtin::as_str),
            Some("194735343546")
        );
        assert_eq!(barbie.availability, Availability::InStock);
    }

    #[test]
    fn product_fixture_parses_from_next_data() {
        let o = Sanborns.parse_product(&page(PRODUCT_URL, PRODUCT)).unwrap();
        assert_eq!(o.retailer_sku.as_str(), "968461");
        assert_eq!(o.title, "Pantalla Samsung de 75 Pulgadas Un75u8200hf UHD");
        assert_eq!(o.brand.as_deref(), Some("SAMSUNG"));
        assert_eq!(o.price_mxn.centavos(), 1_149_500);
        assert_eq!(o.url, PRODUCT_URL);
        assert!(!o.membership_required);
    }
}
