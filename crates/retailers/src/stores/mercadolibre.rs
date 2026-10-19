//! MercadoLibre MX. The public API has no keyword search for third parties
//! (only seller-scoped `/sites/MLM/search`, which answers 403 without the
//! seller's token), so pages come through Web Unlocker like the other stores;
//! see `docs/modules/retailers/mercadolibre.md`. Search pages carry a JSON-LD
//! `@graph` of `Product`s (CSS `poly-card` tiles as the fallback); product
//! pages carry JSON-LD. The SKU is the `MLM…` id in the product URL: a
//! catalog product (`/p/MLM52768030`) or a listing
//! (`articulo…/MLM-3044424296-…`, kept as `MLM3044424296`).

use brightdata::TargetUrl;
use domain::{Offer, PriceMxn, RetailerSku};
use scraper::{ElementRef, Html};

use crate::adapter::{
    AdapterError, Page, ParseError, Retailer, clean_query, encode_path_segment, target,
};
use crate::parse::css::{self, TileSpec};
use crate::parse::{
    Draft, jsonld_draft, jsonld_drafts, meta_draft, price, product_from, search_from,
};
use crate::registry::{MERCADOLIBRE, RetailerInfo};

/// The MercadoLibre adapter.
pub(crate) struct MercadoLibre;

/// `MLM` followed by up to 15 digits at the start of `s`, as `MLM<digits>`.
fn mlm_digits(s: &str) -> Option<String> {
    let rest = s.strip_prefix("MLM")?;
    let rest = rest.strip_prefix('-').unwrap_or(rest);
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    (!digits.is_empty() && digits.len() <= 15).then(|| format!("MLM{digits}"))
}

/// The item or catalog id in a product URL; ignores query and fragment
/// (search tiles link through `click1…?…&wid=MLM…` trackers, which are not
/// product URLs and yield `None`).
fn sku_from_url(url: &str) -> Option<String> {
    let path = url.split(['?', '#']).next()?;
    if let Some(at) = path.find("/p/") {
        return mlm_digits(&path[at + 3..]);
    }
    let host_end = path.find("://").map_or(0, |i| i + 3);
    let slug = path[host_end..].split('/').nth(1)?;
    mlm_digits(slug)
}

/// `url` without query and fragment (tracking parameters).
fn clean_url(url: &str) -> String {
    url.split(['?', '#']).next().unwrap_or(url).to_owned()
}

fn with_url_sku(mut d: Draft) -> Draft {
    d.url = d.url.as_deref().map(clean_url);
    if let Some(sku) = d.url.as_deref().and_then(sku_from_url) {
        d.sku = Some(sku);
    }
    d
}

fn jsonld_search_drafts(doc: &Html) -> Vec<Draft> {
    jsonld_drafts(doc).into_iter().map(with_url_sku).collect()
}

/// Tiles link through ad trackers; the item id is in the `wid` parameter.
fn sku_from_tile(_: ElementRef<'_>, href: Option<&str>) -> Option<String> {
    let href = href?;
    sku_from_url(href).or_else(|| {
        let at = href.find("wid=")?;
        mlm_digits(&href[at + 4..])
    })
}

const TILES: TileSpec = TileSpec {
    tile: &["div.poly-card"],
    link: &["a.poly-component__title"],
    title: &["a.poly-component__title"],
    brand: &[".poly-component__brand"],
    // Andes amounts are split into fraction and cents spans with a hidden
    // separator; see `andes_amount`.
    price: &[],
    list_price: &[],
    price_attr: None,
    sku: sku_from_tile,
};

const CURRENT_PRICE: &[&str] = &[".poly-price__current .andes-money-amount"];
const PREVIOUS_PRICE: &[&str] = &["s.andes-money-amount--previous"];

/// The amount of the first Andes money element under `el` matching `list`:
/// the `__fraction` digits (thousands commas dropped) plus the optional
/// `__cents` span. The visible separator between them is a hidden `,`, so
/// the joined text is not parsed.
fn andes_amount(el: ElementRef<'_>, list: &[&str]) -> Option<PriceMxn> {
    let amount = css::selectors(list)
        .iter()
        .find_map(|s| el.select(s).next())?;
    let digits = |sel: &[&str]| {
        css::child_text(amount, sel).map(|t| t.chars().filter(char::is_ascii_digit).collect())
    };
    let whole: String = digits(&[".andes-money-amount__fraction"])?;
    let cents: String = digits(&[".andes-money-amount__cents"]).unwrap_or_default();
    let text = if cents.is_empty() {
        whole
    } else {
        format!("{whole}.{cents}")
    };
    price::from_str(&text)
}

/// CSS tiles. Tracker links are replaced by the canonical listing URL built
/// from the SKU.
fn css_search_drafts(doc: &Html) -> Vec<Draft> {
    let tiles = css::items(doc, TILES.tile);
    css::tile_drafts(doc, &TILES)
        .into_iter()
        .zip(tiles)
        .map(|(mut d, tile)| {
            d.url = d.sku.as_deref().and_then(listing_url);
            d.price = andes_amount(tile, CURRENT_PRICE);
            d.list_price =
                andes_amount(tile, PREVIOUS_PRICE).filter(|l| d.price.is_some_and(|p| *l > p));
            d
        })
        .collect()
}

/// `https://articulo.mercadolibre.com.mx/MLM-<digits>` for a listing id;
/// catalog ids (`/p/` pages) are not distinguishable from tile ids, so tiles
/// always use the listing form, which the site redirects as needed.
fn listing_url(sku: &str) -> Option<String> {
    let digits = sku.strip_prefix("MLM")?;
    Some(format!("https://articulo.mercadolibre.com.mx/MLM-{digits}"))
}

fn product_page_draft(doc: &Html) -> Draft {
    with_url_sku(jsonld_draft(doc))
}

const PRODUCT_TITLE: &[&str] = &["h1.ui-pdp-title"];
const PRODUCT_PRICE: &[&str] = &[".ui-pdp-price__second-line .andes-money-amount"];
const PRODUCT_LIST_PRICE: &[&str] = &[".ui-pdp-price__original-value"];

/// CSS fallback: the `<h1>`, the Andes price block and the canonical link
/// (`og:title` ends in " - $ 1,745", so it is not used for the title).
fn product_css_draft(doc: &Html) -> Draft {
    let root = doc.root_element();
    let price = andes_amount(root, PRODUCT_PRICE);
    with_url_sku(Draft {
        title: css::first_text(doc, PRODUCT_TITLE),
        price,
        list_price: andes_amount(root, PRODUCT_LIST_PRICE)
            .filter(|l| price.is_some_and(|p| *l > p)),
        url: meta_draft(doc).url,
        ..Draft::default()
    })
}

impl Retailer for MercadoLibre {
    fn info(&self) -> &'static RetailerInfo {
        &MERCADOLIBRE
    }

    /// `listado.mercadolibre.com.mx/<words-joined-by-dashes>`.
    fn search_url(&self, query: &str) -> Result<TargetUrl, AdapterError> {
        let q = clean_query(query)?;
        let slug: String = q
            .split_whitespace()
            .map(|w| {
                w.chars()
                    .filter(|c| c.is_alphanumeric())
                    .collect::<String>()
                    .to_lowercase()
            })
            .filter(|w| !w.is_empty())
            .collect::<Vec<_>>()
            .join("-");
        if slug.is_empty() {
            return Err(AdapterError::EmptyQuery);
        }
        target(&format!(
            "https://listado.mercadolibre.com.mx/{}",
            encode_path_segment(&slug)
        ))
    }

    /// Catalog ids are not distinguishable from listing ids, so this builds
    /// the listing URL; the site redirects catalog-backed listings.
    fn product_url(&self, sku: &RetailerSku) -> Result<TargetUrl, AdapterError> {
        let id = sku.as_str();
        let canonical = mlm_digits(id).filter(|c| c == id);
        let url = canonical
            .as_deref()
            .and_then(listing_url)
            .ok_or(AdapterError::InvalidSku)?;
        target(&url)
    }

    fn parse_search(&self, page: &Page) -> Result<Vec<Offer>, ParseError> {
        search_from(
            self.info(),
            page,
            &[jsonld_search_drafts, css_search_drafts],
        )
    }

    fn parse_product(&self, page: &Page) -> Result<Offer, ParseError> {
        product_from(self.info(), page, &[product_page_draft, product_css_draft])
    }
}

#[cfg(test)]
mod tests {
    use domain::Availability;

    use super::*;
    use crate::parse::test_support::{page, strip_json};

    const SEARCH: &str = include_str!("../../tests/fixtures/mercadolibre/search.html");
    const SEARCH_URL: &str = "https://listado.mercadolibre.com.mx/licuadora";
    const PRODUCT: &str = include_str!("../../tests/fixtures/mercadolibre/product.html");
    const PRODUCT_URL: &str = "https://www.mercadolibre.com.mx/licuadora-ninja-profesional-plus-bn701laa-1400w-tritura-hielo-al-instante-capacidad-de-21l-para-toda-la-familia-3-programas-auto-iq-smoothies-helados-y-frozen-negro/p/MLM52768030";
    const PRODUCT_TITLE_TEXT: &str = "Licuadora Ninja Profesional Plus BN701LAA 1400W Tritura Hielo al Instante Capacidad de 2.1L para Toda la Familia 3 Programas Auto-iQ Smoothies, Helados y Frozen Negro";

    #[test]
    fn product_fixture_parses_from_json_ld() {
        let o = MercadoLibre
            .parse_product(&page(PRODUCT_URL, PRODUCT))
            .unwrap();
        assert_eq!(o.retailer.as_str(), "mercadolibre");
        assert_eq!(o.retailer_sku.as_str(), "MLM52768030");
        assert_eq!(o.title, PRODUCT_TITLE_TEXT);
        assert_eq!(o.brand.as_deref(), Some("Ninja"));
        assert_eq!(o.price_mxn.centavos(), 174_500);
        assert_eq!(o.availability, Availability::InStock);
        assert!(!o.membership_required);
        assert_eq!(o.url, PRODUCT_URL);
    }

    #[test]
    fn product_falls_back_to_css() {
        let stripped = strip_json(PRODUCT);
        let o = MercadoLibre
            .parse_product(&page(PRODUCT_URL, &stripped))
            .unwrap();
        assert_eq!(o.retailer_sku.as_str(), "MLM52768030");
        assert_eq!(o.title, PRODUCT_TITLE_TEXT);
        assert_eq!(o.price_mxn.centavos(), 174_500);
        assert_eq!(o.url, PRODUCT_URL);
    }

    #[test]
    fn selectors_parse_and_urls_build() {
        assert!(TILES.lists().iter().all(|l| css::all_parse(l)));
        assert!(
            [
                CURRENT_PRICE,
                PREVIOUS_PRICE,
                PRODUCT_TITLE,
                PRODUCT_PRICE,
                PRODUCT_LIST_PRICE
            ]
            .iter()
            .all(|l| css::all_parse(l))
        );
        assert_eq!(
            MercadoLibre.search_url("Licuadora").unwrap().as_str(),
            SEARCH_URL
        );
        assert_eq!(
            MercadoLibre
                .search_url("licuadora  ninja!")
                .unwrap()
                .as_str(),
            "https://listado.mercadolibre.com.mx/licuadora-ninja"
        );
        assert_eq!(
            MercadoLibre
                .product_url(&RetailerSku::new("MLM3044424296").unwrap())
                .unwrap()
                .as_str(),
            "https://articulo.mercadolibre.com.mx/MLM-3044424296"
        );
        assert!(
            MercadoLibre
                .product_url(&RetailerSku::new("MLM12x").unwrap())
                .is_err()
        );
    }

    #[test]
    fn skus_come_from_product_urls() {
        assert_eq!(
            sku_from_url("https://www.mercadolibre.com.mx/licuadora-x/p/MLM52768030?a=1"),
            Some("MLM52768030".into())
        );
        assert_eq!(
            sku_from_url("https://articulo.mercadolibre.com.mx/MLM-3044424296-licuadora-_JM#x"),
            Some("MLM3044424296".into())
        );
        assert_eq!(
            sku_from_url("https://www.mercadolibre.com.mx/ofertas"),
            None
        );
        assert_eq!(
            sku_from_tile(
                Html::parse_fragment("<p></p>").root_element(),
                Some("https://click1.mercadolibre.com.mx/count?a=b&wid=MLM3044424296&sid=x")
            ),
            Some("MLM3044424296".into())
        );
    }

    #[test]
    fn search_fixture_parses_from_json_ld() {
        let offers = MercadoLibre
            .parse_search(&page(SEARCH_URL, SEARCH))
            .unwrap();
        assert!(offers.len() >= 20, "{}", offers.len());
        assert!(offers.iter().all(|o| !o.membership_required));
        let o = offers
            .iter()
            .find(|o| o.retailer_sku.as_str() == "MLM52768030")
            .unwrap();
        assert_eq!(o.retailer.as_str(), "mercadolibre");
        assert!(
            o.title
                .starts_with("Licuadora Ninja Profesional Plus Bn701laa")
        );
        assert_eq!(o.brand.as_deref(), Some("Ninja"));
        assert_eq!(o.price_mxn.centavos(), 174_500);
        assert_eq!(o.availability, Availability::InStock);
        assert!(o.url.ends_with("/p/MLM52768030"), "{}", o.url);
    }

    #[test]
    fn search_falls_back_to_css_tiles() {
        let stripped = strip_json(SEARCH);
        let offers = MercadoLibre
            .parse_search(&page(SEARCH_URL, &stripped))
            .unwrap();
        assert!(offers.len() >= 20, "{}", offers.len());
        let o = offers
            .iter()
            .find(|o| o.retailer_sku.as_str() == "MLM3044424296")
            .unwrap();
        assert!(o.title.starts_with("Oster Licuadora Clásica"));
        assert_eq!(o.price_mxn.centavos(), 178_900);
        assert_eq!(o.url, "https://articulo.mercadolibre.com.mx/MLM-3044424296");
        // Superscript cents behind a hidden "," separator.
        assert!(offers.iter().any(|o| o.price_mxn.centavos() == 80_626));
        assert!(offers.iter().any(|o| o.list_price_mxn.is_some()));
    }
}
