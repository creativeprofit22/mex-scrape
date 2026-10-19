//! Fixture sanitizer: shrinks a fetched page to what parsers need and strips
//! anything that could carry a session, token or tracking state.
//!
//! Kept: the element tree with its text, JSON-LD, known page-data scripts
//! (with token-like JSON values redacted), and VTEX `__STATE__` templates.
//! Dropped: comments, `<style>`, `<svg>`, `<iframe>`, `<noscript>`, other
//! scripts, `<link>` (except canonical), `on*`/`style`/`nonce`/`integrity`/
//! `srcset` attributes, hidden input values and csrf/token/nonce `<meta>`.

use std::fmt::Write as _;

use scraper::{ElementRef, Html, Node};

/// Script `id`s whose JSON page data is kept.
pub const KEPT_SCRIPT_IDS: &[&str] = &["__NEXT_DATA__", "__NUXT_DATA__", "__APOLLO_STATE__"];

/// Global names whose inline-script assignment is kept.
pub const KEPT_ASSIGNMENTS: &[&str] = &[
    "__PRELOADED_STATE__",
    "__INITIAL_STATE__",
    "__APOLLO_STATE__",
    "__NUXT__",
    "__STATE__",
];

/// VTEX template `data-varname`s that are kept.
pub const KEPT_TEMPLATES: &[&str] = &["__STATE__"];

const DROPPED_ELEMENTS: &[&str] = &[
    "style", "svg", "iframe", "noscript", "object", "embed", "canvas", "video", "audio",
];

const VOID: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track",
    "wbr",
];

const SENSITIVE_WORDS: &[&str] = &[
    "token",
    "csrf",
    "xsrf",
    "nonce",
    "session",
    "secret",
    "password",
    "passwd",
    "auth",
    "cookie",
    "email",
    "phone",
    "telefono",
    "apikey",
    "api_key",
    "signature",
    // Claro Shop's Algolia app id and search key (`algol_principal`,
    // `algol_secundario`).
    "algol",
];

/// JSON keys (case-insensitive) whose whole value is removed: customer
/// reviews and their authors are personal data.
pub const PERSONAL_KEYS: &[&str] = &[
    "review",
    "reviews",
    "author",
    "reviewer",
    "usernickname",
    "nickname",
    "customerreviews",
    "topreviews",
    "reviewsdata",
];

/// Placeholder for redacted values.
pub const REDACTED: &str = "REDACTED";

/// Sanitizes `html` into a deterministic, smaller document.
#[must_use]
pub fn sanitize(html: &str) -> String {
    let doc = Html::parse_document(html);
    let mut out = String::from("<!DOCTYPE html>\n");
    for child in doc.tree.root().children() {
        if let Some(el) = ElementRef::wrap(child) {
            write_element(el, &mut out);
        }
    }
    out.push('\n');
    out
}

fn is_sensitive(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    SENSITIVE_WORDS.iter().any(|w| n.contains(w))
}

fn keep_script(el: ElementRef<'_>, text: &str) -> bool {
    let v = el.value();
    if v.attr("src").is_some() {
        return false;
    }
    if v.attr("type")
        .is_some_and(|t| t.eq_ignore_ascii_case("application/ld+json"))
    {
        return true;
    }
    if v.attr("id").is_some_and(|id| KEPT_SCRIPT_IDS.contains(&id)) {
        return true;
    }
    if in_kept_template(el) {
        return true;
    }
    KEPT_ASSIGNMENTS
        .iter()
        .any(|name| text.contains(&format!("{name} =")) || text.contains(&format!("{name}=")))
}

fn in_kept_template(el: ElementRef<'_>) -> bool {
    el.ancestors().filter_map(ElementRef::wrap).any(|a| {
        a.value().name() == "template"
            && a.value()
                .attr("data-varname")
                .is_some_and(|n| KEPT_TEMPLATES.contains(&n))
    })
}

fn keep_element(el: ElementRef<'_>) -> bool {
    let v = el.value();
    let name = v.name();
    if DROPPED_ELEMENTS.contains(&name) {
        return false;
    }
    match name {
        "link" => v
            .attr("rel")
            .is_some_and(|r| r.eq_ignore_ascii_case("canonical")),
        "meta" => !["name", "property", "http-equiv", "itemprop"]
            .iter()
            .filter_map(|a| v.attr(a))
            .any(is_sensitive),
        "template" => v
            .attr("data-varname")
            .is_some_and(|n| KEPT_TEMPLATES.contains(&n)),
        _ => true,
    }
}

fn keep_attr(el_name: &str, hidden_input: bool, name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    !(n.starts_with("on")
        || matches!(
            n.as_str(),
            "style" | "nonce" | "integrity" | "srcset" | "sizes"
        )
        || (hidden_input && n == "value")
        || (el_name == "input" && n == "value" && is_sensitive(name))
        || n.starts_with("data-") && is_sensitive(&n))
}

fn write_element(el: ElementRef<'_>, out: &mut String) {
    if !keep_element(el) {
        return;
    }
    let v = el.value();
    let name = v.name();
    if name == "script" {
        let text: String = el.text().collect();
        if !keep_script(el, &text) {
            return;
        }
        open_tag(el, out);
        out.push_str(&scrub_script(text.trim()).replace("</", "<\\/"));
        out.push_str("</script>");
        return;
    }
    open_tag(el, out);
    if VOID.contains(&name) {
        return;
    }
    write_children(*el, out);
    let _ = write!(out, "</{name}>");
}

fn write_children(node: ego_tree_like::NodeRef<'_>, out: &mut String) {
    for child in node.children() {
        match child.value() {
            Node::Element(_) => {
                if let Some(e) = ElementRef::wrap(child) {
                    write_element(e, out);
                }
            }
            Node::Text(t) => push_text(t, out),
            Node::Fragment => write_children(child, out),
            _ => {}
        }
    }
}

/// Re-exported node handle type, so `write_children` can take fragments.
mod ego_tree_like {
    /// A node handle in a parsed document.
    pub type NodeRef<'a> = <scraper::ElementRef<'a> as std::ops::Deref>::Target;
}

fn push_text(t: &str, out: &mut String) {
    let collapsed = t.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        if !t.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        return;
    }
    if t.starts_with(char::is_whitespace) {
        out.push(' ');
    }
    out.push_str(&escape(&collapsed, false));
    if t.ends_with(char::is_whitespace) {
        out.push(' ');
    }
}

fn open_tag(el: ElementRef<'_>, out: &mut String) {
    let v = el.value();
    let name = v.name();
    let hidden = name == "input"
        && v.attr("type")
            .is_some_and(|t| t.eq_ignore_ascii_case("hidden"));
    let _ = write!(out, "<{name}");
    for (k, val) in v.attrs() {
        if keep_attr(name, hidden, k) {
            let _ = write!(out, " {k}=\"{}\"", escape(val, true));
        }
    }
    out.push('>');
}

fn escape(s: &str, attr: bool) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' if attr => o.push_str("&quot;"),
            _ => o.push(c),
        }
    }
    o
}

/// A kept script's text: valid JSON is re-serialized without
/// [`PERSONAL_KEYS`] and with sensitive values redacted; anything else (JS
/// assignments) gets [`redact_json_like`] only.
#[must_use]
pub fn scrub_script(text: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(mut v) => {
            scrub_value(&mut v, 0);
            serde_json::to_string(&v).unwrap_or_default()
        }
        Err(_) => redact_json_like(text),
    }
}

fn scrub_value(v: &mut serde_json::Value, depth: usize) {
    use serde_json::Value;
    if depth > 128 {
        *v = Value::Null;
        return;
    }
    match v {
        Value::Object(map) => {
            map.retain(|k, _| !PERSONAL_KEYS.contains(&k.to_ascii_lowercase().as_str()));
            for (k, child) in map.iter_mut() {
                if child.is_string() && is_sensitive(k) {
                    *child = Value::String(REDACTED.to_owned());
                } else {
                    scrub_value(child, depth + 1);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|c| scrub_value(c, depth + 1)),
        Value::String(text) => {
            if let Some(scrubbed) = scrub_nested_json(text, depth) {
                *text = scrubbed;
            }
        }
        _ => {}
    }
}

/// A string holding a JSON object or array (e.g. Claro Shop's
/// `runtimeConfig.POWREVIEWS`, which carries an API key) is scrubbed too.
/// It is re-serialized only when scrubbing changed it, so other embedded
/// JSON keeps its exact text.
fn scrub_nested_json(text: &str, depth: usize) -> Option<String> {
    let trimmed = text.trim_start();
    if !(trimmed.starts_with('{') || trimmed.starts_with('[')) {
        return None;
    }
    let original: serde_json::Value = serde_json::from_str(text).ok()?;
    let mut scrubbed = original.clone();
    scrub_value(&mut scrubbed, depth + 1);
    (scrubbed != original).then(|| serde_json::to_string(&scrubbed).unwrap_or_default())
}

/// Replaces the string value of every JSON key that looks sensitive
/// (`"csrfToken": "abc"` → `"csrfToken": "REDACTED"`). Works on JSON and
/// JS object literals with double-quoted keys; leaves everything else as is.
#[must_use]
pub fn redact_json_like(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let mut pending_key: Option<bool> = None; // Some(sensitive) after a `"key":`
    while let Some(start) = rest.find('"') {
        let (before, from_quote) = rest.split_at(start);
        if pending_key.is_some() && !before.trim().is_empty() {
            pending_key = None;
        }
        out.push_str(before);
        let Some(len) = string_len(from_quote) else {
            out.push_str(from_quote);
            return out;
        };
        let literal = &from_quote[..len];
        let after = &from_quote[len..];
        let is_key = after.trim_start().starts_with(':');
        if !is_key && pending_key == Some(true) {
            out.push('"');
            out.push_str(REDACTED);
            out.push('"');
        } else {
            out.push_str(literal);
        }
        pending_key = if is_key {
            Some(is_sensitive(&literal[1..len - 1]))
        } else {
            None
        };
        if is_key {
            let colon = after.find(':').map_or(0, |i| i + 1);
            out.push_str(&after[..colon]);
            rest = &after[colon..];
        } else {
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

/// Byte length of the JSON string literal at the start of `s` (with quotes).
fn string_len(s: &str) -> Option<usize> {
    let mut escaped = false;
    for (i, c) in s.char_indices().skip(1) {
        match c {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            '"' => return Some(i + 1),
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_noise_and_secrets_keeps_data() {
        let html = r#"<!doctype html><html><head>
            <meta name="csrf-token" content="abc123"><meta name="description" content="Tienda">
            <link rel="stylesheet" href="/a.css"><link rel="canonical" href="https://x.mx/p">
            <style>.a{}</style><!-- secret comment -->
            <script>var tracking = "zzz";</script>
            <script src="/app.js"></script>
            <script type="application/ld+json">{"@type":"Product","name":"A </b>"}</script>
            <script id="__NEXT_DATA__" type="application/json">{"props":{"sessionId":"s1","price":10}}</script>
            <script>window.__PRELOADED_STATE__ = {"authToken":"t0k","sku":"9"};</script>
            </head><body onload="x()">
            <svg><path d="M0"/></svg><iframe src="https://ads"></iframe><noscript>n</noscript>
            <form><input type="hidden" name="_csrf" value="hidv4l"><input name="q" value="lic"></form>
            <div   class="p" style="color:red" data-price="1299" data-session-id="ss">  Licuadora
               <b>Oster</b>  </div><img src="/i.jpg" srcset="/i2.jpg 2x" alt="i">
            <template data-varname="__STATE__"><script>{"Product:1":{"price":5,"email":"a@b.mx"}}</script></template>
            <template data-varname="__OTHER__"><script>{"x":1}</script></template>
            </body></html>"#;
        let s = sanitize(html);
        for gone in [
            "abc123",
            "secret comment",
            "tracking",
            "app.js",
            "a.css",
            ".a{}",
            "<svg",
            "<iframe",
            "<noscript",
            "hidv4l",
            "onload",
            "color:red",
            "srcset",
            "data-session-id",
            "s1",
            "t0k",
            "a@b.mx",
            "__OTHER__",
        ] {
            assert!(!s.contains(gone), "{gone} left in:\n{s}");
        }
        for kept in [
            r#"<meta content="Tienda" name="description">"#,
            r#"<link href="https://x.mx/p" rel="canonical">"#,
            r#""name":"A <\/b>""#,
            r#""price":10"#,
            r#""sku":"9""#,
            r#"data-price="1299""#,
            "Licuadora <b>Oster</b>",
            r#"<input name="q" value="lic">"#,
            r#"<img alt="i" src="/i.jpg">"#,
            r#""Product:1":{"email":"REDACTED","price":5}"#,
        ] {
            assert!(s.contains(kept), "{kept} missing in:\n{s}");
        }
        assert!(s.starts_with("<!DOCTYPE html>"));
        assert_eq!(sanitize(&s), s, "sanitize is idempotent");
    }

    #[test]
    fn json_scripts_lose_reviews_and_secrets() {
        let out = scrub_script(
            r#"{"@type":"Product","name":"A","review":[{"author":{"name":"Juan"}}],
               "props":{"Reviews":{"x":1},"items":[{"userNickname":"jp","sessionToken":"s","n":2}]}}"#,
        );
        assert_eq!(
            out,
            r#"{"@type":"Product","name":"A","props":{"items":[{"n":2,"sessionToken":"REDACTED"}]}}"#
        );
        assert_eq!(
            scrub_script(r#"window.x = {"authToken":"t"}"#),
            r#"window.x = {"authToken":"REDACTED"}"#
        );
    }

    #[test]
    fn json_inside_json_strings_is_scrubbed_too() {
        let out = scrub_script(
            r#"{"runtimeConfig":{"POWREVIEWS":"{\"apikey\": \"8d8d-key\", \"locale\":\"es_MX\"}",
               "VARIANTS":"{\"telmex\": false,  \"spaced\": true}","note":"{not json"}}"#,
        );
        assert!(!out.contains("8d8d-key"), "{out}");
        assert!(
            out.contains(r#""POWREVIEWS":"{\"apikey\":\"REDACTED\",\"locale\":\"es_MX\"}""#),
            "{out}"
        );
        // Unchanged embedded JSON keeps its exact text.
        assert!(
            out.contains(r#""VARIANTS":"{\"telmex\": false,  \"spaced\": true}""#),
            "{out}"
        );
        assert!(out.contains(r#""note":"{not json""#), "{out}");
        let algolia = scrub_script(r#"{"algol":{"algol_secundario":"7b13de","n":1}}"#);
        assert_eq!(
            algolia,
            r#"{"algol":{"algol_secundario":"REDACTED","n":1}}"#
        );
    }

    #[test]
    fn redaction_only_touches_sensitive_string_values() {
        assert_eq!(
            redact_json_like(
                r#"{"csrfToken" : "a\"b", "title":"token", "n":{"apiKey":"k"},"auth":1}"#
            ),
            r#"{"csrfToken" : "REDACTED", "title":"token", "n":{"apiKey":"REDACTED"},"auth":1}"#
        );
        assert_eq!(
            redact_json_like(r#"{"a":"unterminated"#),
            r#"{"a":"unterminated"#
        );
    }
}
