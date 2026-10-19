//! VTEX IO stores (Chedraui): the normalized Apollo cache in
//! `<template data-varname="__STATE__">`. Objects reference each other with
//! `{"type": "id", "id": "<key>"}`.

use domain::Availability;
use scraper::Html;
use serde_json::{Map, Value};

use super::css::{self, TileSpec};
use super::{Draft, clean_text, compact, embedded, price};

/// The `__STATE__` object, if present.
#[must_use]
pub fn state(doc: &Html) -> Option<Map<String, Value>> {
    match embedded::vtex_template(doc, "__STATE__")? {
        Value::Object(m) => Some(m),
        _ => None,
    }
}

/// Follows an `{"type":"id","id":…}` reference (or returns `v` itself).
fn deref<'a>(state: &'a Map<String, Value>, v: &'a Value) -> Option<&'a Value> {
    match (v.get("type").and_then(Value::as_str), v.get("id")) {
        (Some("id"), Some(Value::String(id))) => state.get(id),
        _ => Some(v),
    }
}

/// The value of the first key of `obj` starting with `prefix` (VTEX keys
/// carry arguments, e.g. `items({"filter":"FIRST_AVAILABLE"})`).
fn field<'a>(obj: &'a Value, prefix: &str) -> Option<&'a Value> {
    obj.as_object()?
        .iter()
        .find(|(k, _)| *k == prefix || k.starts_with(&format!("{prefix}(")))
        .map(|(_, v)| v)
}

/// Draft from one `Product` object. SKU is the URL slug (`linkText`), which
/// is what `/<slug>/p` product URLs need.
#[must_use]
pub fn product_draft(state: &Map<String, Value>, p: &Value) -> Draft {
    let s = |k: &str| p.get(k).and_then(Value::as_str).and_then(clean_text);
    let link_text = s("linkText");
    // `link` may point at the VTEX platform host; the slug path is canonical.
    let url = link_text
        .as_ref()
        .map(|l| format!("/{l}/p"))
        .or_else(|| s("link"));
    let item = field(p, "items")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(|r| deref(state, r));
    let offer = item
        .and_then(|i| i.get("sellers"))
        .and_then(Value::as_array)
        .and_then(|sellers| {
            let resolved: Vec<&Value> = sellers.iter().filter_map(|r| deref(state, r)).collect();
            resolved
                .iter()
                .find(|s| s.get("sellerDefault") == Some(&Value::Bool(true)))
                .or(resolved.first())
                .copied()
        })
        .and_then(|seller| seller.get("commertialOffer"))
        .and_then(|r| deref(state, r));
    let range_low = |which: &str| {
        p.get("priceRange")
            .and_then(|r| deref(state, r))
            .and_then(|r| r.get(which))
            .and_then(|r| deref(state, r))
            .and_then(|r| r.get("lowPrice"))
            .and_then(price::from_json)
    };
    let price = offer
        .and_then(|o| o.get("Price"))
        .and_then(price::from_json)
        .filter(|p| p.centavos() > 0)
        .or_else(|| range_low("sellingPrice"));
    let list_price = offer
        .and_then(|o| o.get("ListPrice"))
        .and_then(price::from_json)
        .or_else(|| range_low("listPrice"));
    let availability = offer
        .and_then(|o| o.get("AvailableQuantity"))
        .and_then(Value::as_i64)
        .map(|q| {
            if q > 0 {
                Availability::InStock
            } else {
                Availability::OutOfStock
            }
        });
    Draft {
        title: s("productName"),
        brand: s("brand"),
        price,
        list_price,
        availability,
        gtin: item
            .and_then(|i| i.get("ean"))
            .and_then(Value::as_str)
            .and_then(clean_text),
        sku: link_text,
        url,
    }
}

/// Every `Product` in the state (sorted by cache key).
#[must_use]
pub fn product_drafts(state: &Map<String, Value>) -> Vec<Draft> {
    state
        .iter()
        .filter(|(k, v)| {
            k.starts_with("Product:")
                && v.get("__typename").and_then(Value::as_str) == Some("Product")
        })
        .map(|(_, v)| product_draft(state, v))
        .collect()
}

/// Search source: every product in `__STATE__`.
#[must_use]
pub fn search_drafts(doc: &Html) -> Vec<Draft> {
    state(doc).map(|s| product_drafts(&s)).unwrap_or_default()
}

/// Product source: the product in `__STATE__` whose slug matches the
/// canonical URL, else the first one.
#[must_use]
pub fn product_draft_from(doc: &Html) -> Draft {
    let Some(st) = state(doc) else {
        return Draft::default();
    };
    let canonical = super::css::first_attr(doc, &[r#"link[rel="canonical"]"#], "href");
    let drafts = product_drafts(&st);
    let pick = canonical
        .as_deref()
        .and_then(|c| {
            drafts.iter().find(|d| {
                d.sku
                    .as_deref()
                    .is_some_and(|slug| c.contains(&format!("/{slug}/p")))
            })
        })
        .or(drafts.first());
    pick.cloned().unwrap_or_default()
}

/// The VTEX product slug of a `/<slug>/p` URL or path.
#[must_use]
pub fn slug_from_href(href: &str) -> Option<String> {
    let end = href.find(['?', '#']).unwrap_or(href.len());
    let path = href[..end].trim_end_matches('/');
    let slug = path.strip_suffix("/p")?.rsplit('/').next()?;
    let ok = !slug.is_empty()
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    ok.then(|| slug.to_owned())
}

fn tile_sku(_: scraper::ElementRef<'_>, href: Option<&str>) -> Option<String> {
    href.and_then(slug_from_href)
}

/// Rendered VTEX IO product-summary tiles (`vtex.product-summary` and
/// `vtex.product-price` blocks). The server HTML of a VTEX IO store has no
/// rendered DOM, so this only matches browser-rendered pages.
pub const TILES: TileSpec = TileSpec {
    tile: &["section.vtex-product-summary-2-x-container"],
    link: &["a.vtex-product-summary-2-x-clearLink"],
    title: &["span.vtex-product-summary-2-x-productBrand"],
    brand: &["span.vtex-product-summary-2-x-productBrandName"],
    price: &["span.vtex-product-price-1-x-sellingPriceValue"],
    list_price: &["span.vtex-product-price-1-x-listPriceValue"],
    price_attr: None,
    sku: tile_sku,
};

/// Rendered VTEX IO product page (`vtex.store-components` blocks).
pub const PRODUCT_TITLE: &[&str] = &["span.vtex-store-components-3-x-productBrand"];
/// Brand on a rendered product page.
pub const PRODUCT_BRAND: &[&str] = &["span.vtex-store-components-3-x-productBrandName"];
/// Selling price on a rendered product page.
pub const PRODUCT_PRICE: &[&str] = &["span.vtex-product-price-1-x-sellingPriceValue"];
/// List price on a rendered product page.
pub const PRODUCT_LIST_PRICE: &[&str] = &["span.vtex-product-price-1-x-listPriceValue"];
const CANONICAL: &[&str] = &[r#"link[rel="canonical"]"#];

/// CSS fallback for listings: rendered product-summary tiles.
#[must_use]
pub fn css_search_drafts(doc: &Html) -> Vec<Draft> {
    css::tile_drafts(doc, &TILES)
}

/// CSS fallback for product pages: rendered store-components blocks; the
/// SKU and URL come from the canonical link.
#[must_use]
pub fn css_product_draft(doc: &Html) -> Draft {
    let amount =
        |list: &[&str]| css::first_text(doc, list).and_then(|t| price::first_in_text(&compact(&t)));
    let price = amount(PRODUCT_PRICE);
    let canonical = css::first_attr(doc, CANONICAL, "href");
    Draft {
        title: css::first_text(doc, PRODUCT_TITLE),
        brand: css::first_text(doc, PRODUCT_BRAND),
        price,
        list_price: amount(PRODUCT_LIST_PRICE).filter(|l| price.is_some_and(|p| *l > p)),
        sku: canonical.as_deref().and_then(slug_from_href),
        url: canonical,
        ..Draft::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_references_to_offer_and_ean() {
        let html = r#"<html><body><template data-varname="__STATE__"><script>{
          "Product:sp-1":{"__typename":"Product","productName":"Licuadora","brand":"Oster",
            "linkText":"licuadora-1","link":"/licuadora-1/p",
            "items({\"filter\":\"FIRST_AVAILABLE\"})":[{"type":"id","id":"Product:sp-1.items.0"}]},
          "Product:sp-1.items.0":{"ean":"7501055300075","sellers":[{"type":"id","id":"Product:sp-1.items.0.sellers.0"}]},
          "Product:sp-1.items.0.sellers.0":{"sellerDefault":true,"commertialOffer":{"type":"id","id":"$o"}},
          "$o":{"Price":345.5,"ListPrice":400,"AvailableQuantity":0}
        }</script></template></body></html>"#;
        let doc = Html::parse_document(html);
        let d = search_drafts(&doc);
        assert_eq!(d.len(), 1);
        let d = &d[0];
        assert_eq!(d.title.as_deref(), Some("Licuadora"));
        assert_eq!(d.price.unwrap().centavos(), 34_550);
        assert_eq!(d.list_price.unwrap().centavos(), 40_000);
        assert_eq!(d.availability, Some(Availability::OutOfStock));
        assert_eq!(d.gtin.as_deref(), Some("7501055300075"));
        assert_eq!(d.sku.as_deref(), Some("licuadora-1"));
        assert_eq!(d.url.as_deref(), Some("/licuadora-1/p"));
    }
}
