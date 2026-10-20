# retailers

## Purpose

Store adapters for Mexican retailers. Each adapter builds a store's search and product URLs and parses fetched pages into the normalized `domain::Offer`. Parsing prefers embedded JSON-LD, then page JSON (`__NEXT_DATA__`, VTEX `__STATE__`), then CSS selectors via the `scraper` crate. Pages are fetched by the caller through `brightdata` (Web Unlocker, or the Browser API where JavaScript is proven necessary). This crate does no I/O.

## Adapters

| Store | Id | Status | Access mode | Membership | Page kinds | Doc |
| --- | --- | --- | --- | --- | --- | --- |
| Walmart MX | `walmart_mx` | parser tested | Unlocker | no | search (`z=1`), product | [walmart_mx](retailers/walmart_mx.md) |
| Liverpool | `liverpool` | parser tested | Unlocker | no | search, product | [liverpool](retailers/liverpool.md) |
| Chedraui | `chedraui` | parser tested | Unlocker | no | search, product | [chedraui](retailers/chedraui.md) |
| Bodega Aurrera | `bodega_aurrera` | parser tested | Unlocker | no | keyword listing, product | [bodega_aurrera](retailers/bodega_aurrera.md) |
| Costco MX | `costco_mx` | parser tested | Unlocker | **yes** | search, product | [costco_mx](retailers/costco_mx.md) |
| Sam's Club MX | `sams_mx` | parser tested | Unlocker | **yes** | search (`z=1`), product | [sams_mx](retailers/sams_mx.md) |
| Sears MX | `sears_mx` | tested | Unlocker (product), **Browser** (category listing) | no | category listing, product | [sears_mx](retailers/sears_mx.md) |
| El Palacio de Hierro | `palacio_de_hierro` | parser tested | Unlocker | no | category listing, product | [palacio_de_hierro](retailers/palacio_de_hierro.md) |
| HEB MX | `heb_mx` | parser tested | Unlocker | no | search / listing, product | [heb_mx](retailers/heb_mx.md) |
| Sanborns | `sanborns` | tested | Unlocker (product, home page), **Browser** (category listing) | no | category listing, home-page listing, product | [sanborns](retailers/sanborns.md) |
| Waldo's | `waldos` | parser tested | Unlocker | no | category listing, product | [waldos](retailers/waldos.md) |
| MercadoLibre MX | `mercadolibre` | parser tested | Unlocker (public API rejected: no keyword search for third parties) | no | search, product | [mercadolibre](retailers/mercadolibre.md) |

"Parser tested" means the store's tests parse sanitized fixtures captured through Web Unlocker on 2026-10-19. The Browser API is used only for page kinds whose Unlocker HTML is proven to lack product data ([ADR 0007](../decisions/0007-brightdata-browser-api.md), [ADR 0008](../decisions/0008-browser-api-settle-blocking-category-listings.md)). Those page kinds are listed in `RetailerInfo::browser_pages`, and the store doc records the evidence. A robots.txt-disallowed page is refused by Bright Data (`blocked_needs_kyc`), so adapters use the allowed listing pages instead. TikTok Shop and Temu are out of scope.

## Public API

- `Retailer` trait (`Send + Sync`): `info()`, `search_url(query)` (search page, or a listing page where search is blocked; `SearchUnavailable` for category-only stores: Sears MX, Sanborns, Waldo's), `product_url(&RetailerSku)`, `parse_search(&Page) -> Vec<Offer>`, `parse_product(&Page) -> Offer`. `Page { url: TargetUrl, body, fetched_at: UnixMillis }`. Errors: `AdapterError` (`EmptyQuery`, `QueryTooLong`, `InvalidSku`, `InvalidUrl`, `SearchUnavailable`) and `ParseError` (`WrongHost`, `NoOffers`, `MissingField`, `InvalidField`).
- `adapters()`: every adapter in registry order. `adapter_for(id)` looks one up.
- Registry: `RetailerInfo { id, name, hosts, samples, access: AccessMode, browser_pages, membership_required }`. `AccessMode { Unlocker, Browser }`. `RetailerInfo::access_for(PageKind)` returns `Browser` for the proven JavaScript-only page kinds. Also `PageKind { Search, Category, Product }` and `all()`, `hosts()`, `retailer_for_host(host)`. Edit `src/registry.rs` to change these.
- `parse`: shared helpers. `jsonld` reads every `application/ld+json` block, including `@graph`, arrays, `Product`, `ItemList`, `Offer` and `AggregateOffer`. `embedded` reads `__NEXT_DATA__`, VTEX `__STATE__` and `<script id=…>` JSON. `vtex` handles the VTEX IO state and rendered blocks. `css` provides selector lists and `TileSpec` tiles. `price` parses MX price formats into integer centavos. `Draft` merges fields and validates them into an `Offer`: the URL must be on the retailer's hosts, an invalid GTIN is dropped, and `membership_required` comes from the registry.
- `source`: `RetailerOfferSource` implements `app::OfferSource` over the adapters.
  - `new(Arc<dyn PageFetcher>, Arc<dyn app::Clock>, Arc<std::sync::Mutex<ParseHealth>>)`: `search`/`product` look up `adapter_for`, build the URL, fetch it with `info().access_for(PageKind::Search|Product)`, stamp the `Page` with `clock.now()` and parse it through `health::parse_page` (`ParseKind::Search|Product`; `product` takes the first offer). Every parse is recorded in the caller-owned `ParseHealth`, so a drop below the threshold logs the alert. The lock is taken only after the fetch completes and never held across an `.await`; a poisoned lock is recovered with `into_inner` so tracking continues.
  - `lookup_only()`: registry lookups only; `search`/`product` validate their input, then fail closed with `OfferSourceError::Unavailable`.
  - Errors map to `OfferSourceError`: `AdapterError` to `InvalidQuery`/`InvalidSku`/`SearchUnavailable`, `FetchError { BlockedNeedsKyc, Failed }` to `BlockedNeedsKyc`/`FetchFailed`, any `ParseError` to `ParseFailed`.
  - `PageFetcher::fetch(&TargetUrl, AccessMode) -> String` is injected by the caller, so this crate still does no I/O. Robots.txt and caps are the fetcher's job: the worker's jobs fetch through `brightdata::Runner` runs instead and parse with `health::parse_page`, so no Bright Data call bypasses `Runner`.
- `fixture::sanitize(html)`: keeps the element tree with its text, JSON-LD, known page-data scripts (token-like JSON values redacted) and VTEX `__STATE__`. It drops comments, `<style>`, `<svg>`, `<iframe>`, `<noscript>`, other scripts, `<link>` except canonical, `on*`/`style`/`nonce`/`integrity`/`srcset` attributes, hidden input values and CSRF/token/nonce meta tags. Used by the worker's `capture_fixtures`.
- `health`: `HealthConfig { threshold (0.8), min_samples (20), window (100) }`, read from `RETAILER_PARSE_ALERT_THRESHOLD` and `RETAILER_PARSE_ALERT_MIN_SAMPLES` via `HealthConfig::from_lookup`. `ParseHealth` keeps a rolling success window per retailer and is owned by the caller (no global state). `record(id, ok)` returns `HealthStatus::{Ok, Alert { rate }, Low { rate }}` (`Alert` once per drop, `Low` while it stays below the threshold). `parse_page(adapter, kind, page, &mut health)` parses, records the result (a search page counts as a success only with at least one offer) and logs `tracing::warn!` with retailer, kind, rate and threshold once per drop below the threshold. It re-arms after recovery.

## Category listings

Stores without keyword search (Sears MX, Sanborns; ADR 0008) are covered by scheduled Browser API browses of configured category listings (worker, ADR 0011).

- `config/category_listings.json` (checked in; the worker reads it from `CATEGORY_LISTINGS_PATH`): `{"listings": [{"retailer": "sears_mx", "url": "https://www.sears.com.mx/cat/…?id=4714"}, …]}`. Use **full menu links** copied from the store's menu (ADR 0008): bare `/cat/<slug>` paths without the `id=` parameter list no products.
- `category::parse_listings(json) -> Vec<CategoryListing { retailer, url: TargetUrl }>` validates every entry: a registered retailer whose `browser_pages` contains `Category` (`category_retailers()`), and a URL that parses as `TargetUrl`, is on one of that retailer's `hosts`, has a path under `/cat/` and a non-empty numeric `id=` parameter. Entries must be exactly `{retailer, url}`; duplicates and more than `MAX_LISTINGS` (500) are rejected. `CategoryConfigError` names the entry index only, never the URL. A unit test validates the checked-in file.
- The worker browses at most `CATEGORY_MAX_PAGES_PER_RUN` listings per retailer per run, in file order, so put the most valuable categories first.

## Fixtures

`crates/retailers/tests/fixtures/<id>/{search,product}.html`, captured with `cargo run -p worker --bin capture_fixtures` (robots.txt first, one request per page, hard cap) and sanitized. Each store module's tests load them with `include_str!`. Re-sanitize existing fixtures with `--resanitize`.

## Dependencies

- Workspace: `domain`, `app` (`OfferSource`, `Clock`, `PortFuture`), `brightdata` (`TargetUrl` only; no fetching).
- External: `scraper`, `serde_json`, `thiserror`, `tracing`. Dev: `tokio` (async `source` tests).

## Owned tables

None.
