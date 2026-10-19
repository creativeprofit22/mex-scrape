# app

## Purpose

The application layer ([ADR 0009](../decisions/0009-onion-architecture.md)): use cases and the ports (traits) they need. It does no I/O. Infrastructure crates implement the ports, and the entry points (`api`, `worker`) build the adapters and inject them.

## Public API

- `PortFuture<'a, T>`: the boxed `Send` future async port methods return, so every port is object-safe (`Arc<dyn Port>`).
- Ports (`app::ports`, all `Send + Sync`):
  - `Clock { now() -> UnixMillis }`. Injected so use cases stay deterministic.
  - `OfferSource`: `retailer_for_host(host) -> Option<RetailerId>` (sync registry lookup), `search(&RetailerId, query) -> Vec<Offer>`, `product(&RetailerId, &RetailerSku) -> Offer`. Errors: `OfferSourceError { UnknownRetailer, InvalidQuery, InvalidSku, SearchUnavailable, BlockedNeedsKyc, FetchFailed, ParseFailed, Unavailable }`. Implemented by `retailers::RetailerOfferSource`.
  - `AmazonSource`: `listing_by_gtin(&Gtin) -> Option<AmazonListing>`, with `AmazonSourceError { Unavailable, RateLimited, ParseFailed }`. `AmazonListing { asin, title, price_mxn, fetched_at }`. Interface only; the Amazon phase implements it and may refine it.
  - `PriceHistoryRepo`: `record(&Offer)`, `history(&RetailerId, &RetailerSku, since) -> Vec<PricePoint>`. `PricePoint { price_mxn, list_price_mxn, availability, observed_at }`. Interface only, until the matching phase.
  - `AccessCountsRepo`: `counts_by_host() -> Vec<HostAccess>`. `HostAccess { host, totals: AccessTotals }`, `AccessTotals { ok, blocked_needs_kyc, skipped_robots, failed }` (with saturating `add`). Implemented by `db::access::PgAccessCounts`.
  - `RepoError { Unavailable }`: the repository error.
  - None of the error enums carry detail strings, so adapter errors (which may echo URLs or credentials) never cross a port.
- Use cases:
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
