//! Sam's Club MX (`www.sams.com.mx`), on Walmart's Glass storefront (see
//! [`super::walmart_glass`]). robots.txt disallows `/search*` but allows
//! `/search?*z=1`. Buying needs a membership, so every offer is flagged.

use brightdata::TargetUrl;
use domain::{Offer, RetailerSku};

use crate::adapter::{AdapterError, Page, ParseError, Retailer};
use crate::parse::{jsonld_draft, jsonld_drafts, meta_draft, product_from, search_from};
use crate::registry::{RetailerInfo, SAMS_MX};
use crate::stores::walmart_glass as glass;
use crate::stores::walmart_mx::{glass_product_url, glass_search_url};

/// The Sam's Club MX adapter.
pub(crate) struct SamsMx;

impl Retailer for SamsMx {
    fn info(&self) -> &'static RetailerInfo {
        &SAMS_MX
    }

    fn search_url(&self, query: &str) -> Result<TargetUrl, AdapterError> {
        glass_search_url(SAMS_MX.hosts[0], query)
    }

    fn product_url(&self, sku: &RetailerSku) -> Result<TargetUrl, AdapterError> {
        glass_product_url(SAMS_MX.hosts[0], sku)
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
    use crate::parse::test_support::page;

    const SEARCH: &str = include_str!("../../tests/fixtures/sams_mx/search.html");
    const SEARCH_URL: &str = "https://www.sams.com.mx/search?q=licuadora&z=1";
    const PRODUCT: &str = include_str!("../../tests/fixtures/sams_mx/product.html");
    const PRODUCT_URL: &str =
        "https://www.sams.com.mx/ip/licuadora-ninja-2-1-l-1200w-bn700/980022073";

    #[test]
    fn urls() {
        assert_eq!(SamsMx.search_url("licuadora").unwrap().as_str(), SEARCH_URL);
        assert_eq!(
            SamsMx
                .product_url(&RetailerSku::new("980022073").unwrap())
                .unwrap()
                .as_str(),
            "https://www.sams.com.mx/ip/980022073"
        );
    }

    #[test]
    fn search_fixture_parses_and_every_offer_needs_membership() {
        let offers = SamsMx.parse_search(&page(SEARCH_URL, SEARCH)).unwrap();
        assert!(offers.len() >= 20, "{}", offers.len());
        assert!(offers.iter().all(|o| o.membership_required));
        let o = &offers[0];
        assert_eq!(o.retailer.as_str(), "sams_mx");
        assert_eq!(o.retailer_sku.as_str(), "980022073");
        assert!(o.title.starts_with("Licuadora Ninja"));
        assert_eq!(
            o.url,
            "https://www.sams.com.mx/ip/licuadora-ninja-2-1-l-1200w-bn700/980022073"
        );
        assert!(o.price_mxn.centavos() > 0);
    }

    #[test]
    fn product_fixture_parses_from_json_ld() {
        let o = SamsMx.parse_product(&page(PRODUCT_URL, PRODUCT)).unwrap();
        assert!(o.membership_required);
        assert_eq!(o.retailer_sku.as_str(), "980022073");
        assert_eq!(o.title, "Licuadora Ninja 2.1 L 1200W BN700");
        assert_eq!(o.brand.as_deref(), Some("Ninja"));
        assert_eq!(o.price_mxn.centavos(), 194_165);
        assert_eq!(
            o.gtin.as_ref().map(domain::Gtin::as_str),
            Some("00622356322386")
        );
        assert_eq!(o.availability, Availability::InStock);
    }
}
