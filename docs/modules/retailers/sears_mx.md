# Sears MX adapter (`sears_mx`)

Status: **tested**. Product pages are fetched with Web Unlocker; category listings are fetched with the Browser API and parsed from the rendered product cards.

| Host | Access mode | Membership required | Page kinds |
| --- | --- | --- | --- |
| `www.sears.com.mx` | Unlocker (product); **Browser** (category listing) | no | category listing, product |

## URLs

- Search: `search_url` returns `SearchUnavailable` (category listings only). robots.txt disallows `/resultados/*` (it would be `blocked_needs_kyc`). Listings are category pages instead.
- Category listing: `/cat/<department>/<category>/<subcategory>?id=<id>`, with the path copied from the site's own menu, e.g. `/cat/línea blanca y electrodomésticos/electrodomésticos de cocina/licuadoras y picadoras?id=4714` (percent-encoded). Do **not** use the short form `/categoria/<id>/<slug>/`. It loads a mismatched path (`.../electrodomésticos/...` instead of `.../electrodomésticos de cocina/...`) and the page lists **0 products** even in a real browser.
- Product: `/producto/<id>/<title_seo>`. The SKU is the numeric id.

## Parsing

- Uses the shared Claro Shop parser (`stores/claro.rs`), which Sanborns also uses. It reads product objects in `__NEXT_DATA__` (`id`, `title`, `sale_price`, `price`, `brand`, `ean`, `stock`).
- Product: `__NEXT_DATA__`, then JSON-LD `Product`, then meta tags.
- Listing: JSON-LD, then `__NEXT_DATA__`, then the rendered product cards (`claro::TILES`): `article.cardProduct`, its `/producto/<id>/` link (SKU), `h3` title, `CardProduct_precio1*` price and `CardProduct_textUnderline*` list price. The CSS module classes carry a build hash, so they are matched by prefix. The cards have no brand (the grey label is the seller).

## Why the Browser API

The Unlocker HTML of a category listing has the right category in `__NEXT_DATA__` but no product JSON-LD, no page JSON with a product or price, and no product cards ("0 resultados"). The products are fetched by JavaScript after the page loads. This was checked on 2026-10-19 with the correct `/cat/...` link (fixture `search_unlocker.html`, test `listing_fixture_has_no_product_data_without_javascript`).

The same link rendered with the Browser API showed 479 results, with 100 product cards on the first page (fixture `search.html`). The capture took about 15 s with the default download blocking on ([brightdata](../brightdata.md#browser-api-adr-0007-adr-0008)).

So Sears MX's `RetailerInfo::browser_pages` is `[Category]`, and `access_for(Category)` returns `Browser` ([ADR 0007](../../decisions/0007-brightdata-browser-api.md)). Product pages stay on Unlocker.

Recapture:

```bash
cargo run -p worker --bin capture_fixtures -- --retailer sears_mx --kind search --browser \
  --url "https://www.sears.com.mx/cat/l%C3%ADnea%20blanca%20y%20electrodom%C3%A9sticos/electrodom%C3%A9sticos%20de%20cocina/licuadoras%20y%20picadoras?id=4714" \
  --out-dir target/browser-capture
```

Review the output before you copy it over `crates/retailers/tests/fixtures/sears_mx/search.html`. Without `--out-dir`, the tool overwrites the fixture directly.

## Tests and fixtures

- Fixtures in `crates/retailers/tests/fixtures/sears_mx/`:
  - `search.html`: rendered listing (Browser API).
  - `search_unlocker.html`: the same listing via Unlocker, kept as evidence.
  - `product.html`.
- Tests:
  - `rendered_listing_parses_from_product_cards`
  - `listing_fixture_has_no_product_data_without_javascript`
  - `product_fixture_parses_from_next_data`
  - `product_falls_back_to_json_ld` (page JSON stripped)
  - the `claro.rs` unit tests, including `tile_selectors_parse_and_ids_come_from_links`
