# Bodega Aurrera adapter (`bodega_aurrera`)

Status: **parser tested** against sanitized fixtures captured on 2026-10-19 through Web Unlocker (country MX).

| Host | Access mode | Membership required | Page kinds |
| --- | --- | --- | --- |
| `www.bodegaaurrera.com.mx` | Unlocker | no | keyword listing, product |

## URLs

- Search: robots.txt disallows every `/search*` URL. "Search" therefore uses the keyword listing pages `/c/kp/<slug>`, which the store lists in its `topicSitemap.xml`.
- Product: `/ip/<slug>/<id>`.

## Parsing

- Uses the Walmart Glass parser (see [walmart_mx](walmart_mx.md)).
- Listing: `__NEXT_DATA__`, then CSS tiles.
- Product: JSON-LD, then page JSON, then meta tags.

## Tests and fixtures

- Fixtures: `crates/retailers/tests/fixtures/bodega_aurrera/{search,product}.html`.
- Tests: `search_fixture_parses_from_next_data`, `search_falls_back_to_css_tiles`, `product_fixture_parses_from_json_ld`, `urls`.

## Browser API

Not used: the Unlocker HTML already contains the product data.
