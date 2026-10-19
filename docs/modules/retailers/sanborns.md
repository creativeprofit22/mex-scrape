# Sanborns adapter (`sanborns`)

Status: **tested**. Product pages and the home page are fetched with Web Unlocker. Category listings are fetched with the Browser API and parsed from the rendered product cards.

| Host | Access mode | Membership required | Page kinds |
| --- | --- | --- | --- |
| `www.sanborns.com.mx` | Unlocker (product, home page); **Browser** (category listing) | no | category listing, home-page listing, product |

## URLs

- Search: `search_url` returns `SearchUnavailable` (category listings only). The site's robots.txt disallows `/resultados/q=<q>/1` (`/resultados/*`). Bright Data refused it with `blocked_needs_kyc (BrowserRobots)` on 2026-10-19, and it stays refused until KYC approval. Listings are category pages instead.
- Category listing: `/cat/<department>/<category>/<subcategory>?id=<id>`, with the path copied from the site's own menu, e.g. `/cat/tecnología y electrónica/tv y video/pantallas?id=100609` (percent-encoded). Use the full menu link; on Sears a short `/categoria/<id>/<slug>/` form loads an empty listing ([sears_mx](sears_mx.md#urls)).
- Home page: `/`, whose product carousels carry server-rendered data.
- Product: `/producto/<id>/<title_seo>`. The SKU is the numeric id.

## Parsing

- Uses the shared Claro Shop parser (`stores/claro.rs`, see [sears_mx](sears_mx.md#parsing)).
- Product: `__NEXT_DATA__`, then JSON-LD, then meta tags.
- Listing: JSON-LD, then `__NEXT_DATA__` (home page), then the rendered product cards (category listings). The cards have no brand (the grey label is the seller).

## Why the Browser API

The Unlocker HTML of a category listing has the category in `__NEXT_DATA__` but no product JSON-LD, no page JSON with a product or price, and no product cards. The products are fetched by JavaScript after the page loads. This was checked on 2026-10-19 with the `pantallas?id=100609` link (fixture `search_unlocker.html`, test `listing_fixture_has_no_product_data_without_javascript`).

The same link rendered with the Browser API showed 499 results, with 100 product cards on the first page (fixture `search.html`).

So Sanborns' `RetailerInfo::browser_pages` is `[Category]`, and search stays on Unlocker so no browser time is spent on a refused page ([ADR 0007](../../decisions/0007-brightdata-browser-api.md)).

Recapture:

```bash
cargo run -p worker --bin capture_fixtures -- --retailer sanborns --kind search --browser \
  --url "https://www.sanborns.com.mx/cat/tecnolog%C3%ADa%20y%20electr%C3%B3nica/tv%20y%20video/pantallas?id=100609" \
  --out-dir target/browser-capture
```

Review the output before you copy it over `crates/retailers/tests/fixtures/sanborns/search.html`. Without `--out-dir`, the tool overwrites the fixture directly.

## Tests and fixtures

- Fixtures in `crates/retailers/tests/fixtures/sanborns/`:
  - `search.html`: rendered category listing (Browser API).
  - `search_unlocker.html`: the same listing via Unlocker, kept as evidence.
  - `home.html`: the home page.
  - `product.html`.
- Tests:
  - `rendered_listing_parses_from_product_cards`
  - `listing_fixture_has_no_product_data_without_javascript`
  - `home_page_parses_from_next_data`
  - `product_fixture_parses_from_next_data`
