# Waldo's adapter (`waldos`)

Status: **parser tested** against sanitized fixtures captured on 2026-10-19 through Web Unlocker (country MX).

| Host | Access mode | Membership required | Page kinds |
| --- | --- | --- | --- |
| `waldos.com.mx` | Unlocker | no | category listing, product |

## URLs

- Search: `search_url` returns `SearchUnavailable` (category listings only). robots.txt disallows `/catalogsearch/`, so listings come from category pages such as `/electrodomesticos/electrodomesticos-de-cocina`.
- Product: `/<url-key>`. The SKU is the URL key, so listing and product offers share it.

## Parsing

- The site runs Magento 2.
- Listings: product tiles (`li.product-item`), with prices from `data-price-amount`.
- Product: JSON-LD `Product`, with the SKU and URL taken from the canonical link, then meta tags.

## Tests and fixtures

- Fixtures: `crates/retailers/tests/fixtures/waldos/{search,product}.html` (search is a category listing).
- Tests: `listing_fixture_parses_tiles`, `product_fixture_parses_from_json_ld`, `selectors_parse_and_urls_build`.

## Browser API

Not used: the Unlocker HTML already contains the product data.
