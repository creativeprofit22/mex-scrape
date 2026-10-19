# Liverpool adapter (`liverpool`)

Status: **parser tested** against sanitized fixtures captured on 2026-10-19 through Web Unlocker (country MX).

| Host | Access mode | Membership required | Page kinds |
| --- | --- | --- | --- |
| `www.liverpool.com.mx` | Unlocker | no | search, product |

## URLs

- Search: `/tienda?s=<q>`.
- Product: `/tienda/pdp/<slug>/<id>`. The SKU is the product id.

## Parsing

- The site is a Next.js App Router app. Its page data is an RSC stream, not JSON, so there is no page JSON to read.
- Search: JSON-LD (none today), then CSS product cards.
- Product: JSON-LD, then CSS (title and price blocks), then meta tags.

## Tests and fixtures

- Fixtures: `crates/retailers/tests/fixtures/liverpool/{search,product}.html`.
- Tests: `search_fixture_parses_cards`, `product_fixture_parses_with_css`, `css_is_used_when_no_json_exists` (JSON stripped), `selectors_parse_and_urls_build`.

## Browser API

Not used: the server-rendered HTML already contains product cards and prices.
