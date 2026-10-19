//! Shared parsing for Walmart's "Glass" Next.js storefront, used by Walmart
//! MX, Bodega Aurrera and Sam's Club MX: `__NEXT_DATA__` items, then
//! `div[data-item-id]` product tiles.

use domain::{Availability, PriceMxn};
use scraper::Html;
use serde_json::Value;

use crate::parse::{Draft, clean_text, compact, css, embedded, price};

/// CSS selectors for the tile fallback (asserted valid in tests).
pub(crate) const TILE: &[&str] = &["div[data-item-id]"];
pub(crate) const TILE_LINK: &[&str] = &["a[link-identifier]"];
pub(crate) const TILE_TITLE: &[&str] = &[r#"[data-automation-id="product-title"]"#];
pub(crate) const TILE_PRICE: &[&str] = &[r#"[data-automation-id="product-price"]"#];

fn is_item(v: &Value) -> bool {
    v.get("usItemId").is_some_and(Value::is_string)
        && v.get("name").is_some_and(Value::is_string)
        && v.get("priceInfo").is_some_and(Value::is_object)
}

/// Selling and list price from a Glass `priceInfo` (string or object forms)
/// or a bare `price` number.
fn prices(item: &Value) -> (Option<PriceMxn>, Option<PriceMxn>) {
    let info = item.get("priceInfo");
    let pick = |keys: &[&str]| {
        keys.iter().find_map(|k| {
            let v = info?.get(*k)?;
            match v {
                Value::Object(_) => v.get("price").and_then(price::from_json),
                _ => price::from_json(v),
            }
        })
    };
    let current = pick(&["currentPrice", "linePrice", "linePriceDisplay", "itemPrice"])
        .or_else(|| item.get("price").and_then(price::from_json))
        .filter(|p| p.centavos() > 0);
    let was = pick(&["wasPrice", "listPrice"]);
    (current, was)
}

fn availability(item: &Value) -> Option<Availability> {
    if item.get("isOutOfStock") == Some(&Value::Bool(true)) {
        return Some(Availability::OutOfStock);
    }
    match item.get("availabilityStatus").and_then(Value::as_str)? {
        "IN_STOCK" => Some(Availability::InStock),
        "OUT_OF_STOCK" => Some(Availability::OutOfStock),
        _ => Some(Availability::Unknown),
    }
}

fn item_draft(item: &Value) -> Draft {
    let s = |k: &str| item.get(k).and_then(Value::as_str).and_then(clean_text);
    let (price, list_price) = prices(item);
    Draft {
        title: s("name"),
        brand: s("brand"),
        price,
        list_price,
        availability: availability(item),
        gtin: s("upc").or_else(|| s("gtin13")),
        sku: s("usItemId"),
        url: s("canonicalUrl"),
    }
}

/// Search source: every item in `__NEXT_DATA__`.
pub(crate) fn next_search_drafts(doc: &Html) -> Vec<Draft> {
    let Some(data) = embedded::script_by_id(doc, "__NEXT_DATA__") else {
        return Vec::new();
    };
    let Some(items) = embedded::at(&data, "props/pageProps/initialData/searchResult/itemStacks")
    else {
        // Category ("browse") pages keep items elsewhere; walk everything.
        return embedded::find_all(&data, is_item)
            .into_iter()
            .map(item_draft)
            .collect();
    };
    embedded::find_all(items, is_item)
        .into_iter()
        .map(item_draft)
        .collect()
}

/// Product source: the page's product in `__NEXT_DATA__`.
pub(crate) fn next_product_draft(doc: &Html) -> Draft {
    let Some(data) = embedded::script_by_id(doc, "__NEXT_DATA__") else {
        return Draft::default();
    };
    let product = embedded::at(&data, "props/pageProps/initialData/data/product")
        .filter(|p| p.is_object())
        .or_else(|| embedded::find_all(&data, is_item).into_iter().next());
    product.map(item_draft).unwrap_or_default()
}

/// `/ip/...` part of a tile link (sponsored tiles wrap it in a tracker URL).
fn ip_path(href: &str) -> Option<String> {
    let at = href.rfind("/ip/")?;
    let path = &href[at..];
    let end = path.find(['?', '&', '#']).unwrap_or(path.len());
    Some(path[..end].to_owned())
}

/// CSS fallback: product tiles.
pub(crate) fn css_search_drafts(doc: &Html) -> Vec<Draft> {
    css::items(doc, TILE)
        .into_iter()
        .map(|tile| {
            let link = css::selectors(TILE_LINK)
                .iter()
                .find_map(|s| tile.select(s).next());
            let amounts = css::selectors(TILE_PRICE)
                .iter()
                .find_map(|s| tile.select(s).next())
                .map(|p| price::all_in_text(&compact(&p.text().collect::<String>())))
                .unwrap_or_default();
            let price = amounts.first().copied();
            let list_price = amounts.iter().copied().max().filter(|m| Some(*m) > price);
            Draft {
                title: css::child_text(tile, TILE_TITLE),
                price,
                list_price,
                sku: link
                    .and_then(|a| a.value().attr("link-identifier"))
                    .and_then(clean_text),
                url: link.and_then(|a| a.value().attr("href")).and_then(ip_path),
                ..Draft::default()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selectors_parse_and_ip_paths_are_unwrapped() {
        for list in [TILE, TILE_LINK, TILE_TITLE, TILE_PRICE] {
            assert!(css::all_parse(list));
        }
        assert_eq!(
            ip_path("/sp/track?rd=x&amp;/ip/licuadora/0075").as_deref(),
            Some("/ip/licuadora/0075")
        );
        assert_eq!(ip_path("/ip/a/1?x=2").as_deref(), Some("/ip/a/1"));
        assert_eq!(ip_path("/browse/a"), None);
    }
}
