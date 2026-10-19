# Chedraui adapter (`chedraui`)

Status: **parser tested** against sanitized fixtures captured on 2026-10-19 through Web Unlocker (country MX).

| Host | Access mode | Membership required | Page kinds |
| --- | --- | --- | --- |
| `www.chedraui.com.mx` | Unlocker | no | search, product |

## URLs

- Search: VTEX full-text search, `/<q>?_q=<q>&map=ft`.
- Product: `/<slug>/p`. The SKU is the VTEX slug.

## Parsing

- The site runs on VTEX IO. Its server HTML holds the Apollo cache in `<template data-varname="__STATE__">` plus an empty render container.
- Search and product: JSON-LD, then `__STATE__` (`parse/vtex.rs`), then CSS on rendered VTEX blocks (`vtex-product-summary`, `vtex-product-price`, `vtex-store-components`), then meta tags.
- The CSS fallback matches only browser-rendered HTML. A test asserts that the Unlocker HTML without `__STATE__` has no products. The CSS test uses a hand-written rendered sample, not a capture.
- An invalid EAN in the data is dropped.

## Tests and fixtures

- Fixtures: `crates/retailers/tests/fixtures/chedraui/{search,product}.html`.
- Tests: `search_fixture_parses_vtex_state`, `product_fixture_parses_vtex_state`, `search_falls_back_to_rendered_css_tiles`, `product_falls_back_to_rendered_css`, `unlocker_html_without_json_has_no_rendered_products`, `css_selectors_parse`, `urls`.

## Browser API

Not used: `__STATE__` in the Unlocker HTML already contains the product data.
