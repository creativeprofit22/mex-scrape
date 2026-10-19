//! Walmart MX (`www.walmart.com.mx`). Search: robots.txt disallows
//! `/search*` but allows `/search?*z=1`, so search URLs carry `z=1`.

use brightdata::TargetUrl;
use domain::{Offer, RetailerSku};

use crate::adapter::{AdapterError, Page, ParseError, Retailer, clean_query, encode_query, target};
use crate::parse::{jsonld_draft, jsonld_drafts, meta_draft, product_from, search_from};
use crate::registry::{RetailerInfo, WALMART_MX};
use crate::stores::walmart_glass as glass;

/// The Walmart MX adapter.
pub(crate) struct WalmartMx;

/// `https://{host}/ip/{sku}` for an all-digit Glass item id.
pub(crate) fn glass_product_url(host: &str, sku: &RetailerSku) -> Result<TargetUrl, AdapterError> {
    let id = sku.as_str();
    if id.is_empty() || id.len() > 20 || !id.bytes().all(|b| b.is_ascii_digit()) {
        return Err(AdapterError::InvalidSku);
    }
    target(&format!("https://{host}/ip/{id}"))
}

/// `https://{host}/search?q={query}&z=1`.
pub(crate) fn glass_search_url(host: &str, query: &str) -> Result<TargetUrl, AdapterError> {
    let q = clean_query(query)?;
    target(&format!("https://{host}/search?q={}&z=1", encode_query(&q)))
}

impl Retailer for WalmartMx {
    fn info(&self) -> &'static RetailerInfo {
        &WALMART_MX
    }

    fn search_url(&self, query: &str) -> Result<TargetUrl, AdapterError> {
        glass_search_url(WALMART_MX.hosts[0], query)
    }

    fn product_url(&self, sku: &RetailerSku) -> Result<TargetUrl, AdapterError> {
        glass_product_url(WALMART_MX.hosts[0], sku)
    }

    fn parse_search(&self, page: &Page) -> Result<Vec<Offer>, ParseError> {
        search_from(
            self.info(),
            page,
            &[
                jsonld_drafts,
                glass::next_search_drafts,
                glass::css_search_drafts,
            ],
        )
    }

    fn parse_product(&self, page: &Page) -> Result<Offer, ParseError> {
        product_from(
            self.info(),
            page,
            &[jsonld_draft, glass::next_product_draft, meta_draft],
        )
    }
}

#[cfg(test)]
mod tests {
    use domain::Availability;

    use super::*;
    use crate::parse::test_support::{page, strip_json};

    const SEARCH: &str = include_str!("../../tests/fixtures/walmart_mx/search.html");
    const SEARCH_URL: &str = "https://www.walmart.com.mx/search?q=licuadora&z=1";

    #[test]
    fn urls() {
        assert_eq!(
            WalmartMx.search_url(" licuadora  oster ").unwrap().as_str(),
            "https://www.walmart.com.mx/search?q=licuadora+oster&z=1"
        );
        let sku = RetailerSku::new("00750163870041").unwrap();
        assert_eq!(
            WalmartMx.product_url(&sku).unwrap().as_str(),
            "https://www.walmart.com.mx/ip/00750163870041"
        );
        assert!(
            WalmartMx
                .product_url(&RetailerSku::new("../x").unwrap())
                .is_err()
        );
    }

    #[test]
    fn search_fixture_parses_from_next_data() {
        let offers = WalmartMx.parse_search(&page(SEARCH_URL, SEARCH)).unwrap();
        assert!(offers.len() >= 40, "{}", offers.len());
        let o = &offers[0];
        assert_eq!(o.retailer.as_str(), "walmart_mx");
        assert_eq!(o.retailer_sku.as_str(), "00061723003275");
        assert!(o.title.starts_with("Licuadora HoninJoy 10-en-1"));
        assert_eq!(o.brand.as_deref(), Some("HoninJoy"));
        assert_eq!(o.price_mxn.centavos(), 299_900);
        assert_eq!(o.list_price_mxn.map(|p| p.centavos()), Some(549_999));
        assert_eq!(o.availability, Availability::InStock);
        assert!(
            o.url
                .starts_with("https://www.walmart.com.mx/ip/licuadora-honinjoy-")
        );
        assert!(o.url.ends_with("/00061723003275"));
        assert!(!o.membership_required);
        assert!(offers.iter().all(|o| o.url.contains("/ip/")));
    }

    #[test]
    fn search_fixture_falls_back_to_css_tiles() {
        let html = strip_json(SEARCH);
        let offers = WalmartMx.parse_search(&page(SEARCH_URL, &html)).unwrap();
        assert!(offers.len() >= 5, "{}", offers.len());
        let o = &offers[0];
        assert_eq!(o.retailer_sku.as_str(), "00061723003275");
        assert_eq!(o.price_mxn.centavos(), 299_900);
        assert_eq!(o.list_price_mxn.map(|p| p.centavos()), Some(549_999));
        assert!(o.url.starts_with("https://www.walmart.com.mx/ip/"));
        assert!(o.title.starts_with("Licuadora HoninJoy"));
    }

    const PRODUCT: &str = include_str!("../../tests/fixtures/walmart_mx/product.html");
    const PRODUCT_URL: &str = "https://www.walmart.com.mx/ip/licuadora-honinjoy-10-en-1-procesador-professional-2-litros-1300-watts-molinillo-con-vaso-deportivo/00061723003275";

    #[test]
    fn product_fixture_prefers_json_ld_and_fills_from_next_data() {
        let o = WalmartMx
            .parse_product(&page(PRODUCT_URL, PRODUCT))
            .unwrap();
        assert_eq!(o.retailer_sku.as_str(), "00061723003275");
        assert!(o.title.starts_with("Licuadora HoninJoy 10-en-1"));
        assert_eq!(o.brand.as_deref(), Some("HoninJoy"));
        assert_eq!(o.price_mxn.centavos(), 299_900);
        // JSON-LD has no list price; __NEXT_DATA__ fills it.
        assert_eq!(o.list_price_mxn.map(|p| p.centavos()), Some(549_999));
        assert_eq!(o.availability, Availability::InStock);
        assert_eq!(o.url, PRODUCT_URL);
    }

    #[test]
    fn product_fixture_falls_back_to_meta_tags() {
        let html = strip_json(PRODUCT);
        // Walmart's product HTML has no price meta tag: CSS alone cannot price it.
        assert_eq!(
            WalmartMx.parse_product(&page(PRODUCT_URL, &html)),
            Err(ParseError::MissingField("price"))
        );
    }

    #[test]
    fn wrong_host_is_rejected() {
        assert_eq!(
            WalmartMx.parse_search(&page("https://www.liverpool.com.mx/", SEARCH)),
            Err(ParseError::WrongHost)
        );
    }
}
