# MercadoLibre MX adapter (`mercadolibre`)

Status: **parser tested** against sanitized fixtures captured on 2026-10-19 through Web Unlocker (country MX).

| Hosts | Access mode | Membership required | Page kinds |
| --- | --- | --- | --- |
| `listado.mercadolibre.com.mx`, `www.mercadolibre.com.mx`, `articulo.mercadolibre.com.mx` | Unlocker | no | search, product |

## Public API or Web Unlocker

Decision: **Web Unlocker**, not the public API.

- The official "Ítems y Búsquedas" docs (<https://developers.mercadolibre.com.mx/es_ar/items-y-busquedas>, last updated 2026-09-10, checked 2026-10-19) list `/sites/$SITE_ID/search` only with seller filters (`sellerid`, `nickname`, seller + category) and point seller queries to `/users/{id}/items/search`. They document no keyword search for third-party apps.
- Public reports since 2025 show `/sites/MLM/search?q=` answering 403 even with an app token.
- A public-API adapter would also need a new HTTP client crate, because `retailers` does no I/O. The plan said to propose an ADR in that case. Keyword search is not available, so no ADR is proposed.

Revisit this only if MercadoLibre documents a keyword search for third-party apps and its terms allow price comparison.

## URLs

- Search: `https://listado.mercadolibre.com.mx/<q>`.
- Product: a catalog page `https://www.mercadolibre.com.mx/<slug>/p/MLM<id>` or a listing `https://articulo.mercadolibre.com.mx/MLM-<id>-<slug>`. The SKU is the `MLM…` id (`MLM-3044424296` is kept as `MLM3044424296`).

## Parsing

- Search: JSON-LD `@graph` of `Product`s, then CSS `poly-card` tiles (Andes money amounts).
- Product: JSON-LD `Product`, then CSS (`h1.ui-pdp-title`, the Andes price block, the canonical link). `og:title` ends in " - $ 1,745", so it is not used for the title.

## Tests and fixtures

- Fixtures: `crates/retailers/tests/fixtures/mercadolibre/{search,product}.html`.
- Tests: `search_fixture_parses_from_json_ld`, `search_falls_back_to_css_tiles`, `product_fixture_parses_from_json_ld`, `product_falls_back_to_css`, `skus_come_from_product_urls`, `selectors_parse_and_urls_build`.

## Browser API

Not used: the Unlocker HTML already contains the product data.
