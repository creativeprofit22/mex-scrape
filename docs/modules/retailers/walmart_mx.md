# Walmart MX adapter (`walmart_mx`)

Status: **parser tested** against sanitized fixtures captured on 2026-10-19 through Web Unlocker (country MX).

| Host | Access mode | Membership required | Page kinds |
| --- | --- | --- | --- |
| `www.walmart.com.mx` | Unlocker | no | search, product |

## URLs

- Search: `/search?q=<q>&z=1`. robots.txt disallows `/search*` but allows `/search?*z=1`, so every search URL carries `z=1`. This replaced the planned browse-page workaround.
- Product: `/ip/<slug>/<id>`. The SKU is the numeric item id.

## Parsing

- Uses the shared Walmart "Glass" Next.js parser (`stores/walmart_glass.rs`), which Bodega Aurrera and Sam's Club MX also use.
- Search: `__NEXT_DATA__` items, then CSS `div[data-item-id]` tiles.
- Product: JSON-LD `Product` (filled from `__NEXT_DATA__`), then Open Graph / product meta tags.

## Tests and fixtures

- Fixtures: `crates/retailers/tests/fixtures/walmart_mx/{search,product}.html`, sanitized by `retailers::fixture::sanitize`.
- Tests: `search_fixture_parses_from_next_data`, `search_fixture_falls_back_to_css_tiles` (JSON stripped), `product_fixture_prefers_json_ld_and_fills_from_next_data`, `product_fixture_falls_back_to_meta_tags`, `wrong_host_is_rejected`, `urls`.

## Browser API

Not used: the Unlocker HTML already contains the product data.
