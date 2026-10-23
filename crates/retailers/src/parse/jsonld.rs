//! schema.org JSON-LD: every `script[type="application/ld+json"]`, with
//! `@graph`, arrays, `Product`, `ItemList`, `Offer` and `AggregateOffer`.

use domain::Availability;
use scraper::{Html, Selector};
use serde_json::Value;

use super::{Draft, clean_text, price};

/// Every parsed top-level JSON-LD node, flattened through arrays and `@graph`,
/// in document order. Blocks that are not valid JSON are skipped.
#[must_use]
pub fn nodes(doc: &Html) -> Vec<Value> {
    let Ok(sel) = Selector::parse(r#"script[type="application/ld+json"]"#) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for script in doc.select(&sel) {
        let text: String = script.text().collect();
        if let Ok(v) = serde_json::from_str::<Value>(text.trim()) {
            flatten(v, &mut out, 0);
        }
    }
    out
}

fn flatten(v: Value, out: &mut Vec<Value>, depth: usize) {
    if depth > 8 {
        return;
    }
    match v {
        Value::Array(items) => items.into_iter().for_each(|i| flatten(i, out, depth + 1)),
        Value::Object(mut map) => {
            if let Some(graph) = map.remove("@graph") {
                flatten(graph, out, depth + 1);
            }
            if !map.is_empty() {
                out.push(Value::Object(map));
            }
        }
        _ => {}
    }
}

/// True when a node's `@type` is (or includes) `ty`.
#[must_use]
pub fn is_type(v: &Value, ty: &str) -> bool {
    match v.get("@type") {
        Some(Value::String(s)) => s.eq_ignore_ascii_case(ty),
        Some(Value::Array(a)) => a
            .iter()
            .any(|t| t.as_str().is_some_and(|s| s.eq_ignore_ascii_case(ty))),
        _ => false,
    }
}

/// Every `Product` node: top-level ones plus those inside `ItemList`s.
#[must_use]
pub fn products(doc: &Html) -> Vec<Value> {
    let mut out = Vec::new();
    for node in nodes(doc) {
        if is_type(&node, "Product") {
            out.push(node);
        } else if is_type(&node, "ItemList") {
            out.extend(item_list_products(&node));
        }
    }
    out
}

/// Whether the page's JSON-LD states an empty result: an `ItemList` whose
/// `numberOfItems` is `0` (number or string) and no `Product` anywhere. An
/// `ItemList` without `numberOfItems`, or one that merely lists nothing, is
/// not enough: a page whose results load later can ship an empty list.
#[must_use]
pub fn declares_no_results(doc: &Html) -> bool {
    let nodes = nodes(doc);
    let zero = |n: &Value| match n.get("numberOfItems") {
        Some(Value::Number(v)) => v.as_u64() == Some(0),
        Some(Value::String(s)) => s.trim() == "0",
        _ => false,
    };
    nodes.iter().any(|n| is_type(n, "ItemList") && zero(n))
        && !nodes.iter().any(|n| {
            is_type(n, "Product") || (is_type(n, "ItemList") && !item_list_products(n).is_empty())
        })
}

/// `Product`s listed in an `ItemList` (`itemListElement[].item` or direct).
#[must_use]
pub fn item_list_products(list: &Value) -> Vec<Value> {
    let Some(items) = list.get("itemListElement").and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|el| {
            if is_type(el, "Product") {
                Some(el.clone())
            } else {
                el.get("item").filter(|i| is_type(i, "Product")).cloned()
            }
        })
        .collect()
}

/// A string field (`"x"`, a number, or `{ "name": "x" }`).
#[must_use]
pub fn text_field(v: &Value, key: &str) -> Option<String> {
    match v.get(key)? {
        Value::String(s) => clean_text(s),
        Value::Number(n) => Some(n.to_string()),
        Value::Object(o) => o.get("name").and_then(Value::as_str).and_then(clean_text),
        Value::Array(a) => a.first().and_then(|first| match first {
            Value::String(s) => clean_text(s),
            Value::Object(o) => o.get("name").and_then(Value::as_str).and_then(clean_text),
            _ => None,
        }),
        _ => None,
    }
}

/// Draft from a schema.org `Product` node.
#[must_use]
pub fn product_draft(p: &Value) -> Draft {
    let mut d = Draft {
        title: text_field(p, "name"),
        brand: text_field(p, "brand"),
        gtin: ["gtin13", "gtin", "gtin12", "gtin14", "gtin8", "ean"]
            .iter()
            .find_map(|k| text_field(p, k)),
        sku: ["sku", "productID"].iter().find_map(|k| text_field(p, k)),
        url: text_field(p, "url"),
        ..Draft::default()
    };
    if let Some(offer) = p.get("offers").and_then(best_offer) {
        d = d.or(offer_draft(offer));
    }
    d
}

/// The first offer with an MXN (or unspecified) price, descending into
/// arrays and `AggregateOffer.offers`.
fn best_offer(v: &Value) -> Option<&Value> {
    match v {
        Value::Array(a) => a.iter().find_map(best_offer),
        Value::Object(_) => {
            let currency_ok = v
                .get("priceCurrency")
                .and_then(Value::as_str)
                .is_none_or(|c| c.eq_ignore_ascii_case("MXN"));
            let has_price = ["price", "lowPrice"]
                .iter()
                .any(|k| v.get(k).and_then(price::from_json).is_some())
                || v.get("priceSpecification").is_some();
            if currency_ok && has_price {
                Some(v)
            } else {
                v.get("offers").and_then(best_offer)
            }
        }
        _ => None,
    }
}

/// Draft fields from one `Offer`/`AggregateOffer` node.
#[must_use]
pub fn offer_draft(o: &Value) -> Draft {
    let mut price = ["price", "lowPrice"]
        .iter()
        .find_map(|k| o.get(k).and_then(price::from_json));
    let mut list_price = None;
    let specs: Vec<&Value> = match o.get("priceSpecification") {
        Some(Value::Array(a)) => a.iter().collect(),
        Some(v @ Value::Object(_)) => vec![v],
        _ => Vec::new(),
    };
    for spec in specs {
        let p = spec.get("price").and_then(price::from_json);
        let kind = spec.get("priceType").and_then(Value::as_str).unwrap_or("");
        if kind.contains("Strikethrough") || kind.contains("ListPrice") || kind.contains("MSRP") {
            list_price = list_price.or(p);
        } else {
            price = price.or(p);
        }
    }
    Draft {
        price,
        list_price,
        availability: o
            .get("availability")
            .and_then(Value::as_str)
            .map(Availability::from_schema_org),
        sku: text_field(o, "sku"),
        url: text_field(o, "url"),
        ..Draft::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(scripts: &[&str]) -> Html {
        let body: String = scripts
            .iter()
            .map(|s| format!(r#"<script type="application/ld+json">{s}</script>"#))
            .collect();
        Html::parse_document(&format!("<html><head>{body}</head><body></body></html>"))
    }

    #[test]
    fn product_with_offer_and_brand_object() {
        let d = doc(&[
            r#"{"@context":"https://schema.org","@type":"Product","name":" Licuadora  Oster ",
            "brand":{"@type":"Brand","name":"Oster"},"sku":"123","gtin13":"7501055300075",
            "offers":{"@type":"Offer","price":"1299.00","priceCurrency":"MXN",
            "availability":"https://schema.org/InStock","url":"https://x/p"}}"#,
        ]);
        let ps = products(&d);
        assert_eq!(ps.len(), 1);
        let dr = product_draft(&ps[0]);
        assert_eq!(dr.title.as_deref(), Some("Licuadora Oster"));
        assert_eq!(dr.brand.as_deref(), Some("Oster"));
        assert_eq!(dr.price.unwrap().centavos(), 129_900);
        assert_eq!(dr.availability, Some(Availability::InStock));
        assert_eq!(dr.gtin.as_deref(), Some("7501055300075"));
        assert_eq!(dr.sku.as_deref(), Some("123"));
    }

    #[test]
    fn only_an_explicit_zero_count_declares_no_results() {
        for zero in [
            r#"{"@type":"ItemList","numberOfItems":0,"itemListElement":[]}"#,
            r#"{"@graph":[{"@type":"WebPage"},{"@type":"ItemList","numberOfItems":" 0 "}]}"#,
        ] {
            assert!(declares_no_results(&doc(&[zero])), "{zero}");
        }
        for not_zero in [
            r#"{"@type":"ItemList","itemListElement":[]}"#,
            r#"{"@type":"ItemList","numberOfItems":"100","itemListElement":[]}"#,
            r#"{"@type":"ItemList","numberOfItems":null}"#,
            r#"{"@type":"WebPage"}"#,
        ] {
            assert!(!declares_no_results(&doc(&[not_zero])), "{not_zero}");
        }
        // A zero count next to real products is not trusted.
        assert!(!declares_no_results(&doc(&[
            r#"{"@type":"ItemList","numberOfItems":0}"#,
            r#"{"@type":"Product","name":"A"}"#,
        ])));
        assert!(!declares_no_results(&doc(&[
            r#"{"@type":"ItemList","numberOfItems":0,"itemListElement":[{"@type":"Product","name":"A"}]}"#,
        ])));
        assert!(!declares_no_results(&Html::parse_document("<html></html>")));
    }

    #[test]
    fn graph_item_list_aggregate_and_strikethrough() {
        let d = doc(&[
            "not json",
            r#"{"@graph":[{"@type":"WebPage"},{"@type":"ItemList","itemListElement":[
              {"@type":"ListItem","item":{"@type":"Product","name":"A",
                "offers":{"@type":"AggregateOffer","lowPrice":99.5,"highPrice":120}}},
              {"@type":"ListItem","url":"https://x/b"},
              {"@type":["Product"],"name":"C","offers":[
                {"price":"10","priceCurrency":"USD"},
                {"priceCurrency":"MXN","priceSpecification":[
                  {"price":"200"},{"price":"250","priceType":"https://schema.org/StrikethroughPrice"}]}]}
            ]}]}"#,
        ]);
        let ps = products(&d);
        assert_eq!(ps.len(), 2);
        assert_eq!(product_draft(&ps[0]).price.unwrap().centavos(), 9_950);
        let c = product_draft(&ps[1]);
        assert_eq!(c.price.unwrap().centavos(), 20_000);
        assert_eq!(c.list_price.unwrap().centavos(), 25_000);
    }
}
