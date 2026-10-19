//! Page JSON embedded in HTML: `<script id="__NEXT_DATA__">`, VTEX
//! `<template data-varname="__STATE__">`, and `window.X = {...}` assignments.

use scraper::{Html, Selector};
use serde_json::Value;

/// Deepest JSON nesting walked by [`walk`].
const MAX_DEPTH: usize = 64;

/// JSON in `<script id="{id}">` (e.g. `__NEXT_DATA__`).
#[must_use]
pub fn script_by_id(doc: &Html, id: &str) -> Option<Value> {
    if !id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return None;
    }
    let sel = Selector::parse(&format!("script#{id}")).ok()?;
    let text: String = doc.select(&sel).next()?.text().collect();
    serde_json::from_str(text.trim()).ok()
}

/// JSON in a VTEX `<template data-varname="{name}"><script>…</script></template>`.
#[must_use]
pub fn vtex_template(doc: &Html, name: &str) -> Option<Value> {
    if !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return None;
    }
    let sel = Selector::parse(&format!(r#"template[data-varname="{name}"]"#)).ok()?;
    let tpl = doc.select(&sel).next()?;
    // html5ever puts template children in a separate fragment; the text
    // collected from the element covers it.
    let text: String = tpl.text().collect();
    serde_json::from_str(text.trim()).ok()
}

/// JSON assigned in an inline script: `window.{name} = {...}` or
/// `{name} = {...}` (first match).
#[must_use]
pub fn assignment(doc: &Html, name: &str) -> Option<Value> {
    let sel = Selector::parse("script:not([src])").ok()?;
    doc.select(&sel).find_map(|s| {
        let text: String = s.text().collect();
        assignment_in(&text, name)
    })
}

/// [`assignment`] on raw script text.
#[must_use]
pub fn assignment_in(text: &str, name: &str) -> Option<Value> {
    let mut from = 0;
    while let Some(pos) = text[from..].find(name) {
        let after = from + pos + name.len();
        let rest = text[after..].trim_start();
        if let Some(rhs) = rest.strip_prefix('=').map(str::trim_start)
            && !rhs.starts_with('=')
            && let Some(json) = balanced(rhs)
            && let Ok(v) = serde_json::from_str(json)
        {
            return Some(v);
        }
        from = after;
    }
    None
}

/// The balanced `{...}` or `[...]` prefix of `s`, honouring JSON strings.
fn balanced(s: &str) -> Option<&str> {
    let open = s.chars().next()?;
    if open != '{' && open != '[' {
        return None;
    }
    let mut depth = 0usize;
    let mut in_str = false;
    let mut escaped = false;
    for (i, c) in s.char_indices() {
        if in_str {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_str = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' | '[' => depth += 1,
            '}' | ']' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(&s[..=i]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Visits every object/array under `root` depth-first, in document order
/// (object keys in source order are not guaranteed; serde_json sorts them).
pub fn walk<'a>(root: &'a Value, visit: &mut impl FnMut(&'a Value)) {
    fn go<'a>(v: &'a Value, visit: &mut impl FnMut(&'a Value), depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        match v {
            Value::Object(m) => {
                visit(v);
                m.values().for_each(|c| go(c, visit, depth + 1));
            }
            Value::Array(a) => a.iter().for_each(|c| go(c, visit, depth + 1)),
            _ => {}
        }
    }
    go(root, visit, 0);
}

/// Every object under `root` for which `pred` holds, in [`walk`] order.
#[must_use]
pub fn find_all(root: &Value, pred: impl Fn(&Value) -> bool) -> Vec<&Value> {
    let mut out = Vec::new();
    walk(root, &mut |v| {
        if pred(v) {
            out.push(v);
        }
    });
    out
}

/// Follows a `/`-separated path of object keys and array indices.
#[must_use]
pub fn at<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('/')
        .filter(|p| !p.is_empty())
        .try_fold(v, |cur, key| match cur {
            Value::Object(m) => m.get(key),
            Value::Array(a) => key.parse::<usize>().ok().and_then(|i| a.get(i)),
            _ => None,
        })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn next_data_and_template() {
        let doc = Html::parse_document(
            r#"<html><body><script id="__NEXT_DATA__" type="application/json">{"props":{"a":[1,{"b":2}]}}</script>
            <template data-varname="__STATE__"><script>{"Product:1":{"productName":"X"}}</script></template>
            </body></html>"#,
        );
        let nd = script_by_id(&doc, "__NEXT_DATA__").unwrap();
        assert_eq!(at(&nd, "props/a/1/b"), Some(&json!(2)));
        let st = vtex_template(&doc, "__STATE__").unwrap();
        assert_eq!(at(&st, "Product:1/productName"), Some(&json!("X")));
        assert!(script_by_id(&doc, "bad id\"]").is_none());
    }

    #[test]
    fn assignment_handles_strings_with_braces() {
        let doc = Html::parse_document(
            r#"<script>var x = 1; if (a == b) {}; window.__PRELOADED_STATE__ = {"t":"a } b","n":[1,2]};</script>"#,
        );
        let v = assignment(&doc, "__PRELOADED_STATE__").unwrap();
        assert_eq!(v, json!({"t":"a } b","n":[1,2]}));
        assert!(assignment_in("x == {}", "x").is_none());
        assert!(assignment_in("x = {", "x").is_none());
    }

    #[test]
    fn find_all_walks_nested() {
        let v = json!({"a":[{"price":1},{"b":{"price":2}}]});
        let found = find_all(&v, |o| o.get("price").is_some());
        assert_eq!(found.len(), 2);
    }
}
