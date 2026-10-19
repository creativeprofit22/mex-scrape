//! Shared parsing for the Grupo Sanborns / Claro Shop Next.js storefront,
//! used by Sears MX and Sanborns: product objects in `__NEXT_DATA__`
//! (`id`, `title`, `sale_price`, `price`, `brand`, `ean`, `stock`,
//! `title_seo`). Product URLs are `/producto/{id}/{title_seo}`. There is no
//! keyword search: robots.txt disallows `/resultados/*`.

use brightdata::TargetUrl;
use domain::{Availability, PriceMxn, RetailerSku};
use scraper::Html;
use serde_json::Value;

use crate::adapter::{AdapterError, target};
use crate::parse::css::{self, TileSpec};
use crate::parse::{Draft, clean_text, embedded, price};

/// `https://{host}/producto/{id}` for an all-digit product id (the slug is
/// cosmetic).
pub(crate) fn product_url(host: &str, sku: &RetailerSku) -> Result<TargetUrl, AdapterError> {
    let id = sku.as_str();
    if id.is_empty() || id.len() > 12 || !id.bytes().all(|b| b.is_ascii_digit()) {
        return Err(AdapterError::InvalidSku);
    }
    target(&format!("https://{host}/producto/{id}"))
}

fn is_product(v: &Value) -> bool {
    v.get("id").is_some_and(Value::is_u64)
        && v.get("title").is_some_and(Value::is_string)
        && v.get("sale_price").is_some_and(Value::is_number)
}

fn product_draft(p: &Value) -> Draft {
    let s = |k: &str| p.get(k).and_then(Value::as_str).and_then(clean_text);
    let id = p.get("id").and_then(Value::as_u64);
    let sale: Option<PriceMxn> = p
        .get("sale_price")
        .and_then(price::from_json)
        .filter(|x| x.centavos() > 0);
    let list = p
        .get("price")
        .and_then(price::from_json)
        .filter(|l| sale.is_some_and(|s| *l > s));
    let availability = p.get("stock").and_then(Value::as_i64).map(|n| {
        if n > 0 {
            Availability::InStock
        } else {
            Availability::OutOfStock
        }
    });
    let url = id.map(|id| match s("title_seo") {
        Some(seo) => format!("/producto/{id}/{seo}"),
        None => format!("/producto/{id}"),
    });
    Draft {
        title: s("title"),
        brand: s("brand"),
        price: sale,
        list_price: list,
        availability,
        gtin: s("ean"),
        sku: id.map(|i| i.to_string()),
        url,
    }
}

/// Search/listing source: every product object in `__NEXT_DATA__`.
pub(crate) fn next_search_drafts(doc: &Html) -> Vec<Draft> {
    embedded::script_by_id(doc, "__NEXT_DATA__")
        .map(|data| {
            embedded::find_all(&data, is_product)
                .into_iter()
                .map(product_draft)
                .collect()
        })
        .unwrap_or_default()
}

/// The all-digit product id after `/producto/` in a tile link.
fn id_from_href(_: scraper::ElementRef<'_>, href: Option<&str>) -> Option<String> {
    let path = href?;
    let rest = &path[path.find("/producto/")? + "/producto/".len()..];
    let id = rest.split(['/', '?', '#']).next()?;
    (!id.is_empty() && id.len() <= 12 && id.bytes().all(|b| b.is_ascii_digit()))
        .then(|| id.to_owned())
}

/// Product cards rendered by the browser on category listings (CSS module
/// classes carry a build hash, so they are matched by prefix).
pub(crate) const TILES: TileSpec = TileSpec {
    tile: &["article.cardProduct"],
    link: &["a[href*=\"/producto/\"]"],
    title: &["h3"],
    brand: &[],
    price: &["p[class*=\"CardProduct_precio1\"]"],
    list_price: &["span[class*=\"CardProduct_textUnderline\"]"],
    price_attr: None,
    sku: id_from_href,
};

/// Listing source for browser-rendered pages: the product cards.
pub(crate) fn css_search_drafts(doc: &Html) -> Vec<Draft> {
    css::tile_drafts(doc, &TILES)
}

/// Product source: `props.pageProps.response.data` in `__NEXT_DATA__`.
pub(crate) fn next_product_draft(doc: &Html) -> Draft {
    embedded::script_by_id(doc, "__NEXT_DATA__")
        .and_then(|data| {
            embedded::at(&data, "props/pageProps/response/data")
                .filter(|p| is_product(p))
                .map(product_draft)
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn product_object_maps_to_draft() {
        let d = product_draft(&json!({
            "id": 306869, "title": " Cafetera  X ", "sale_price": 889, "price": 1489,
            "brand": "KOBLENZ", "ean": "0000000000000", "stock": 0, "title_seo": "cafetera-x"
        }));
        assert_eq!(d.title.as_deref(), Some("Cafetera X"));
        assert_eq!(d.price.map(|p| p.centavos()), Some(88_900));
        assert_eq!(d.list_price.map(|p| p.centavos()), Some(148_900));
        assert_eq!(d.availability, Some(Availability::OutOfStock));
        assert_eq!(d.sku.as_deref(), Some("306869"));
        assert_eq!(d.url.as_deref(), Some("/producto/306869/cafetera-x"));
        let same = product_draft(&json!({"id": 1, "title": "Y", "sale_price": 10, "price": 10}));
        assert_eq!(same.list_price, None);
        assert!(!is_product(
            &json!({"id": "1", "title": "Y", "sale_price": 10})
        ));
    }

    #[test]
    fn tile_selectors_parse_and_ids_come_from_links() {
        assert!(TILES.lists().iter().all(|l| css::all_parse(l)));
        let doc = Html::parse_document("<p></p>");
        let el = doc.root_element();
        assert_eq!(
            id_from_href(el, Some("/producto/1003912/licuadora-x/")).as_deref(),
            Some("1003912")
        );
        assert_eq!(id_from_href(el, Some("/producto/abc/x")), None);
        assert_eq!(id_from_href(el, Some("/cat/x?id=4714")), None);
        assert_eq!(id_from_href(el, None), None);
    }

    #[test]
    fn urls() {
        let sku = RetailerSku::new("306869").unwrap();
        assert_eq!(
            product_url("www.sears.com.mx", &sku).unwrap().as_str(),
            "https://www.sears.com.mx/producto/306869"
        );
        assert!(product_url("www.sears.com.mx", &RetailerSku::new("CKM-212").unwrap()).is_err());
    }
}
