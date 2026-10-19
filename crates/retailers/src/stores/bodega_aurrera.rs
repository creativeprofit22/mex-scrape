//! Bodega Aurrera (`www.bodegaaurrera.com.mx`), on Walmart's Glass
//! storefront (see [`super::walmart_glass`]). robots.txt disallows every
//! `/search*` URL, so "search" uses the store's keyword listing pages
//! (`/c/kp/<slug>`, listed in its `topicSitemap.xml`).

use brightdata::TargetUrl;
use domain::{Offer, RetailerSku};

use crate::adapter::{AdapterError, Page, ParseError, Retailer, clean_query, target};
use crate::parse::{jsonld_draft, jsonld_drafts, meta_draft, product_from, search_from};
use crate::registry::{BODEGA_AURRERA, RetailerInfo};
use crate::stores::walmart_glass as glass;
use crate::stores::walmart_mx::glass_product_url;

/// The Bodega Aurrera adapter.
pub(crate) struct BodegaAurrera;

/// Lowercase ASCII slug: letters and digits, everything else a single `-`
/// (accents are folded for the Spanish vowels and `ñ`).
pub(crate) fn slug(query: &str) -> Result<String, AdapterError> {
    let q = clean_query(query)?;
    let mut out = String::with_capacity(q.len());
    for c in q.to_lowercase().chars() {
        let c = match c {
            'á' | 'à' | 'ä' => 'a',
            'é' | 'è' | 'ë' => 'e',
            'í' | 'ì' | 'ï' => 'i',
            'ó' | 'ò' | 'ö' => 'o',
            'ú' | 'ù' | 'ü' => 'u',
            'ñ' => 'n',
            c => c,
        };
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let s = out.trim_end_matches('-').to_owned();
    if s.is_empty() {
        Err(AdapterError::EmptyQuery)
    } else {
        Ok(s)
    }
}

impl Retailer for BodegaAurrera {
    fn info(&self) -> &'static RetailerInfo {
        &BODEGA_AURRERA
    }

    fn search_url(&self, query: &str) -> Result<TargetUrl, AdapterError> {
        target(&format!(
            "https://{}/c/kp/{}",
            BODEGA_AURRERA.hosts[0],
            slug(query)?
        ))
    }

    fn product_url(&self, sku: &RetailerSku) -> Result<TargetUrl, AdapterError> {
        glass_product_url(BODEGA_AURRERA.hosts[0], sku)
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

    const SEARCH: &str = include_str!("../../tests/fixtures/bodega_aurrera/search.html");
    const SEARCH_URL: &str = "https://www.bodegaaurrera.com.mx/c/kp/licuadoras-de-inmersion";
    const PRODUCT: &str = include_str!("../../tests/fixtures/bodega_aurrera/product.html");
    const PRODUCT_URL: &str = "https://www.bodegaaurrera.com.mx/ip/Licuadora-de-inmersion-Ganiza-6-en-1-1100W-15-velocidades-con-accesorios/00045047183451";

    #[test]
    fn urls() {
        assert_eq!(
            BodegaAurrera
                .search_url(" Licuadoras de  inmersión ")
                .unwrap()
                .as_str(),
            SEARCH_URL
        );
        assert_eq!(slug("¿?"), Err(AdapterError::EmptyQuery));
        assert_eq!(slug("piña 1/2 kg").unwrap(), "pina-1-2-kg");
        assert_eq!(
            BodegaAurrera
                .product_url(&RetailerSku::new("00045047183451").unwrap())
                .unwrap()
                .as_str(),
            "https://www.bodegaaurrera.com.mx/ip/00045047183451"
        );
    }

    #[test]
    fn search_fixture_parses_from_next_data() {
        let offers = BodegaAurrera
            .parse_search(&page(SEARCH_URL, SEARCH))
            .unwrap();
        assert!(offers.len() >= 20, "{}", offers.len());
        assert!(offers.iter().all(|o| o.url.contains("/ip/")));
        let o = offers
            .iter()
            .find(|o| o.retailer_sku.as_str() == "00045047183451")
            .unwrap();
        assert_eq!(o.retailer.as_str(), "bodega_aurrera");
        assert!(o.title.starts_with("Licuadora de inmersión Ganiza 6 en 1"));
        assert!(o.price_mxn.centavos() > 0);
        assert!(!o.membership_required);
    }

    #[test]
    fn search_falls_back_to_css_tiles() {
        let html = strip_json(SEARCH);
        let offers = BodegaAurrera
            .parse_search(&page(SEARCH_URL, &html))
            .unwrap();
        assert!(offers.len() >= 5, "{}", offers.len());
        assert!(offers.iter().all(|o| o.price_mxn.centavos() > 0));
    }

    #[test]
    fn product_fixture_parses_from_json_ld() {
        let o = BodegaAurrera
            .parse_product(&page(PRODUCT_URL, PRODUCT))
            .unwrap();
        assert_eq!(o.retailer_sku.as_str(), "00045047183451");
        assert_eq!(
            o.title,
            "Licuadora de inmersión Ganiza 6 en 1 1100W, 15 velocidades con accesorios"
        );
        assert_eq!(o.brand.as_deref(), Some("Ganiza"));
        // The page's `gtin13` ("00045047183451") has a bad check digit, so it
        // is dropped rather than trusted.
        assert_eq!(o.gtin, None);
        assert_eq!(o.availability, Availability::InStock);
        assert!(o.price_mxn.centavos() > 0);
    }
}
