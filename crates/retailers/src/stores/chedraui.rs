//! Chedraui (`www.chedraui.com.mx`), a VTEX IO store: data comes from the
//! `__STATE__` template; product SKUs are VTEX URL slugs (`/<slug>/p`).
//! The server HTML has no rendered DOM (the body is an empty render
//! container), so the CSS fallback ([`vtex::css_search_drafts`],
//! [`vtex::css_product_draft`]) only matches browser-rendered pages.

use brightdata::TargetUrl;
use domain::{Offer, RetailerSku};

use crate::adapter::{
    AdapterError, Page, ParseError, Retailer, clean_query, encode_path_segment, encode_query,
    target,
};
use crate::parse::{jsonld_draft, jsonld_drafts, meta_draft, product_from, search_from, vtex};
use crate::registry::{CHEDRAUI, RetailerInfo};

/// The Chedraui adapter.
pub(crate) struct Chedraui;

/// VTEX full-text search: `https://{host}/{q}?_q={q}&map=ft`.
pub(crate) fn vtex_search_url(host: &str, query: &str) -> Result<TargetUrl, AdapterError> {
    let q = clean_query(query)?;
    target(&format!(
        "https://{host}/{}?_q={}&map=ft",
        encode_path_segment(&q),
        encode_query(&q)
    ))
}

/// VTEX product page: `https://{host}/{slug}/p` for a lowercase slug.
pub(crate) fn vtex_product_url(host: &str, sku: &RetailerSku) -> Result<TargetUrl, AdapterError> {
    let slug = sku.as_str();
    let ok = !slug.is_empty()
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if !ok {
        return Err(AdapterError::InvalidSku);
    }
    target(&format!("https://{host}/{slug}/p"))
}

impl Retailer for Chedraui {
    fn info(&self) -> &'static RetailerInfo {
        &CHEDRAUI
    }

    fn search_url(&self, query: &str) -> Result<TargetUrl, AdapterError> {
        vtex_search_url(CHEDRAUI.hosts[0], query)
    }

    fn product_url(&self, sku: &RetailerSku) -> Result<TargetUrl, AdapterError> {
        vtex_product_url(CHEDRAUI.hosts[0], sku)
    }

    fn parse_search(&self, page: &Page) -> Result<Vec<Offer>, ParseError> {
        search_from(
            self.info(),
            page,
            &[jsonld_drafts, vtex::search_drafts, vtex::css_search_drafts],
        )
    }

    fn parse_product(&self, page: &Page) -> Result<Offer, ParseError> {
        product_from(
            self.info(),
            page,
            &[
                jsonld_draft,
                vtex::product_draft_from,
                vtex::css_product_draft,
                meta_draft,
            ],
        )
    }
}

#[cfg(test)]
mod tests {
    use domain::Availability;

    use super::*;
    use crate::parse::test_support::page;

    const SEARCH: &str = include_str!("../../tests/fixtures/chedraui/search.html");
    const SEARCH_URL: &str = "https://www.chedraui.com.mx/licuadora?_q=licuadora&map=ft";

    #[test]
    fn urls() {
        assert_eq!(
            Chedraui.search_url("licuadora").unwrap().as_str(),
            SEARCH_URL
        );
        assert_eq!(
            Chedraui.search_url("café oster").unwrap().as_str(),
            "https://www.chedraui.com.mx/caf%C3%A9%20oster?_q=caf%C3%A9+oster&map=ft"
        );
        let sku = RetailerSku::new("licuadora-home-line-3628102").unwrap();
        assert_eq!(
            Chedraui.product_url(&sku).unwrap().as_str(),
            "https://www.chedraui.com.mx/licuadora-home-line-3628102/p"
        );
        assert!(
            Chedraui
                .product_url(&RetailerSku::new("A/b").unwrap())
                .is_err()
        );
    }

    const PRODUCT: &str = include_str!("../../tests/fixtures/chedraui/product.html");
    const PRODUCT_URL: &str =
        "https://www.chedraui.com.mx/licuadora-hamilton-beach-850wmax-125l-12-vel-58160-3532350/p";

    #[test]
    fn product_fixture_parses_vtex_state() {
        let o = Chedraui.parse_product(&page(PRODUCT_URL, PRODUCT)).unwrap();
        assert_eq!(
            o.retailer_sku.as_str(),
            "licuadora-hamilton-beach-850wmax-125l-12-vel-58160-3532350"
        );
        assert_eq!(
            o.title,
            "Licuadora Hamilton Beach 850WMax 1.25L 12 Vel 58160"
        );
        assert_eq!(o.brand.as_deref(), Some("Hamilton Beach"));
        assert_eq!(o.url, PRODUCT_URL);
        assert_eq!(o.price_mxn.centavos(), 71_500);
        assert_eq!(o.availability, domain::Availability::InStock);
        // The page's EAN ("4009493540") is not a valid GTIN, so it is dropped.
        assert_eq!(o.gtin, None);
    }

    #[test]
    fn search_fixture_parses_vtex_state() {
        let offers = Chedraui.parse_search(&page(SEARCH_URL, SEARCH)).unwrap();
        assert!(offers.len() >= 15, "{}", offers.len());
        let o = offers
            .iter()
            .find(|o| {
                o.retailer_sku.as_str()
                    == "licuadora-home-line-3-velocidades-350w-negra-1114-3628102"
            })
            .unwrap();
        assert_eq!(o.title, "Licuadora Home Line 3 Velocidades 350W Negra 1114");
        assert_eq!(o.brand.as_deref(), Some("Home Line"));
        assert_eq!(o.price_mxn.centavos(), 34_500);
        assert_eq!(o.list_price_mxn, None);
        assert_eq!(o.availability, Availability::InStock);
        assert_eq!(o.gtin.as_ref().map(|g| g.as_str()), Some("7501139511144"));
        assert_eq!(
            o.url,
            "https://www.chedraui.com.mx/licuadora-home-line-3-velocidades-350w-negra-1114-3628102/p"
        );
    }

    /// Rendered VTEX IO markup (class names of the `vtex.product-summary`,
    /// `vtex.product-price` and `vtex.store-components` blocks) with this
    /// fixture's products. A hand-written sample, not a capture: the
    /// Unlocker HTML has no rendered DOM to test against.
    const RENDERED_SEARCH: &str = r#"<html><body>
<section class="vtex-product-summary-2-x-container"><a class="vtex-product-summary-2-x-clearLink" href="/licuadora-home-line-3-velocidades-350w-negra-1114-3628102/p">
<span class="vtex-product-summary-2-x-productBrand">Licuadora Home Line 3 Velocidades 350W Negra 1114</span>
<span class="vtex-product-summary-2-x-productBrandName">Home Line</span>
<span class="vtex-product-price-1-x-listPriceValue">$399.00</span>
<span class="vtex-product-price-1-x-sellingPriceValue"><span>$</span><span>345</span><span>.00</span></span>
</a></section>
</body></html>"#;
    const RENDERED_PRODUCT: &str = r#"<html><head>
<link rel="canonical" href="https://www.chedraui.com.mx/licuadora-hamilton-beach-850wmax-125l-12-vel-58160-3532350/p">
</head><body>
<span class="vtex-store-components-3-x-productBrand">Licuadora Hamilton Beach 850WMax 1.25L 12 Vel 58160</span>
<span class="vtex-store-components-3-x-productBrandName">Hamilton Beach</span>
<span class="vtex-product-price-1-x-sellingPriceValue">$715.00</span>
</body></html>"#;

    #[test]
    fn css_selectors_parse() {
        assert!(
            vtex::TILES
                .lists()
                .iter()
                .all(|l| crate::parse::css::all_parse(l))
        );
        for list in [
            vtex::PRODUCT_TITLE,
            vtex::PRODUCT_BRAND,
            vtex::PRODUCT_PRICE,
            vtex::PRODUCT_LIST_PRICE,
        ] {
            assert!(crate::parse::css::all_parse(list));
        }
    }

    #[test]
    fn unlocker_html_without_json_has_no_rendered_products() {
        use crate::parse::test_support::strip_json;
        assert!(
            Chedraui
                .parse_search(&page(SEARCH_URL, &strip_json(SEARCH)))
                .is_err()
        );
        assert!(
            Chedraui
                .parse_product(&page(PRODUCT_URL, &strip_json(PRODUCT)))
                .is_err()
        );
    }

    #[test]
    fn search_falls_back_to_rendered_css_tiles() {
        let offers = Chedraui
            .parse_search(&page(SEARCH_URL, RENDERED_SEARCH))
            .unwrap();
        assert_eq!(offers.len(), 1);
        let o = &offers[0];
        assert_eq!(
            o.retailer_sku.as_str(),
            "licuadora-home-line-3-velocidades-350w-negra-1114-3628102"
        );
        assert_eq!(o.title, "Licuadora Home Line 3 Velocidades 350W Negra 1114");
        assert_eq!(o.brand.as_deref(), Some("Home Line"));
        assert_eq!(o.price_mxn.centavos(), 34_500);
        assert_eq!(o.list_price_mxn.map(|p| p.centavos()), Some(39_900));
        assert_eq!(
            o.url,
            "https://www.chedraui.com.mx/licuadora-home-line-3-velocidades-350w-negra-1114-3628102/p"
        );
    }

    #[test]
    fn product_falls_back_to_rendered_css() {
        let o = Chedraui
            .parse_product(&page(PRODUCT_URL, RENDERED_PRODUCT))
            .unwrap();
        assert_eq!(
            o.retailer_sku.as_str(),
            "licuadora-hamilton-beach-850wmax-125l-12-vel-58160-3532350"
        );
        assert_eq!(
            o.title,
            "Licuadora Hamilton Beach 850WMax 1.25L 12 Vel 58160"
        );
        assert_eq!(o.brand.as_deref(), Some("Hamilton Beach"));
        assert_eq!(o.price_mxn.centavos(), 71_500);
        assert_eq!(o.url, PRODUCT_URL);
    }
}
