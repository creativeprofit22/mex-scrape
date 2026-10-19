# Costco MX adapter (`costco_mx`)

Status: **parser tested** against sanitized fixtures captured on 2026-10-19 through Web Unlocker (country MX).

| Host | Access mode | Membership required | Page kinds |
| --- | --- | --- | --- |
| `www.costco.com.mx` | Unlocker | **yes** | search, product |

## URLs

- Search: `/search?text=<q>`.
- Product: `/<category path>/p/<code>`. The SKU is the product code.

## Parsing

- The site runs SAP Commerce (Spartacus, Angular SSR). The `storefront-state` script holds only site context, so search reads the server-rendered product tiles with CSS.
- Product: JSON-LD `Product`, then CSS, then meta tags.
- Every offer has `membership_required = true`, set from the registry.

## Tests and fixtures

- Fixtures: `crates/retailers/tests/fixtures/costco_mx/{search,product}.html`.
- Tests: `search_fixture_parses_tiles_and_every_offer_needs_membership`, `product_fixture_parses_from_json_ld` (also asserts the membership flag), `selectors_parse_and_urls_build`.

## Browser API

Not used: the server-rendered HTML already contains tiles and prices.
