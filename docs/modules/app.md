# app

## Purpose

The application layer ([ADR 0009](../decisions/0009-onion-architecture.md)): use cases and the ports (traits) they need. It does no I/O. Infrastructure crates implement the ports, and the entry points (`api`, `worker`) build the adapters and inject them.

## Public API

- `PortFuture<'a, T>`: the boxed `Send` future async port methods return, so every port is object-safe (`Arc<dyn Port>`).
- Ports (`app::ports`, all `Send + Sync`):
  - `Clock { now() -> UnixMillis }`. Injected so use cases stay deterministic.
  - `OfferSource`: `retailer_for_host(host) -> Option<RetailerId>` (sync registry lookup), `search(&RetailerId, query) -> Vec<Offer>`, `product(&RetailerId, &RetailerSku) -> Offer`. Errors: `OfferSourceError { UnknownRetailer, InvalidQuery, InvalidSku, SearchUnavailable, BlockedNeedsKyc, FetchFailed, ParseFailed, Unavailable }`. Implemented by `retailers::RetailerOfferSource`.
  - `AmazonSource`: Amazon.com.mx lookups, all money in MXN. `catalog_by_gtin(&Gtin)` and `catalog_by_keyword(&str)` return `CatalogResult { items: Vec<CatalogItem>, source, fetched_at }`, where `CatalogItem { asin: Asin, title, brand, images, sales_ranks: Vec<SalesRank { category, rank }> }`. `offers(&Asin) -> OfferSummary { buy_box_mxn, lowest_new_mxn, offer_count, source, fetched_at }`. `fees(&Asin, PriceMxn, Fulfillment { Fba, Merchant }) -> FeeEstimate { total_mxn, details: Vec<FeeDetail { fee_type, amount_mxn }>, source, fetched_at }`. `source` is `AmazonDataSource { SpApi, BrightData }`. Errors: `AmazonSourceError { NotConfigured, NotAuthorized, InvalidInput, Unsupported, Unavailable, RateLimited, ParseFailed }`. Implemented by `amazon::SpApiAmazonSource` and `amazon::BrightDataAmazonSource`.
  - `PriceHistoryRepo`: `record(&Offer)`, `history(&RetailerId, &RetailerSku, since) -> Vec<PricePoint>`. `PricePoint { price_mxn, list_price_mxn, availability, observed_at }`. Interface only, until the matching phase.
  - `AccessCountsRepo`: `counts_by_host() -> Vec<HostAccess>`. `HostAccess { host, totals: AccessTotals }`, `AccessTotals { ok, blocked_needs_kyc, skipped_robots, failed }` (with saturating `add`). Implemented by `db::access::PgAccessCounts`.
  - `RepoError { Unavailable }`: the repository error.
  - None of the error enums carry detail strings, so adapter errors (which may echo URLs or credentials) never cross a port.
- Use cases:
  - `amazon::FallbackAmazonSource::new(primary, fallback)` (both `Option<Arc<dyn AmazonSource>>`), itself an `AmazonSource`. It asks the primary (SP-API) first and the fallback (Bright Data) only when the primary is missing or fails with `NotConfigured`, `NotAuthorized` or `Unavailable`. `RateLimited`, `ParseFailed`, `InvalidInput` and `Unsupported` are returned as-is, so a busy SP-API never triggers paid fallback calls. With neither source it returns `NotConfigured`.
  - `access::AccessReport::new(Arc<dyn AccessCountsRepo>, Arc<dyn OfferSource>)`, the Bright Data access report.
    - `per_host()` returns `Vec<HostAccessReport { retailer, host, totals }>` in the repository's order. The label is the retailer id from `OfferSource::retailer_for_host`, or the host when it is not registered.
    - `per_retailer()` returns a `BTreeMap<String, AccessTotals>` summed per label and sorted.
    - Both fail with `AccessReportError::Storage` when the counts cannot be read.
    - Unit tests use hand-written fakes, with no database.

## Dependencies

- Workspace: `domain` only. `scripts/check_docs.py` fails if `app` depends on an infrastructure or entry-point crate, or directly on `sqlx`, `axum`, `hyper`, `tower`, `utoipa` or an HTTP client crate.
- External: `thiserror`. Dev: `tokio` (async tests).

## Owned tables

None.
