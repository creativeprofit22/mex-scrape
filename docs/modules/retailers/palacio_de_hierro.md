# El Palacio de Hierro adapter (`palacio_de_hierro`)

Status: **parser tested** against sanitized fixtures captured on 2026-10-19 through Web Unlocker (country MX).

| Host | Access mode | Membership required | Page kinds |
| --- | --- | --- | --- |
| `www.elpalaciodehierro.com` | Unlocker | no | category listing, product |

## URLs

- Search: robots.txt disallows every URL with a query string, so `/buscar?q=` is not used. Listings come from category pages such as `/hogar/electrodomesticos/`.
- Product: `/<slug>-<id>.html`. The SKU is the trailing numeric id.

## Parsing

- The site runs Salesforce B2C Commerce (SFCC).
- Listings: JSON-LD `ItemList`.
- Product: there is no JSON-LD `Product`, so CSS, then Open Graph meta tags.

## Tests and fixtures

- Fixtures: `crates/retailers/tests/fixtures/palacio_de_hierro/{search,product}.html` (search is a category listing).
- Tests: `listing_fixture_parses_from_json_ld_item_list`, `product_fixture_parses_with_css`, `selectors_parse_and_urls_build`.

## Browser API

Not used: the Unlocker HTML already contains the product data.
