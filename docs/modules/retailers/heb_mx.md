# HEB MX adapter (`heb_mx`)

Status: **parser tested** against sanitized fixtures captured on 2026-10-19 through Web Unlocker (country MX).

| Host | Access mode | Membership required | Page kinds |
| --- | --- | --- | --- |
| `www.heb.com.mx` | Unlocker | no | search / category listing, product |

## URLs

- Search: `/search?q=<q>`. Category pages such as `/cafe` use the same parser.
- Product: `/<slug>/p`. The SKU is the slug, so search and product offers share it.

## Parsing

- The site is a Next.js App Router app (RSC stream, no page JSON).
- Listings: server-rendered product links, read with CSS.
- Product: JSON-LD `Product`, then CSS, then meta tags.

## Tests and fixtures

- Fixtures: `crates/retailers/tests/fixtures/heb_mx/{search,product}.html` (search is the `/cafe` listing).
- Tests: `listing_fixture_parses_tiles`, `product_fixture_parses_from_json_ld`, `selectors_parse_and_urls_build`.

## Browser API

Not used: the Unlocker HTML already contains the product data.
