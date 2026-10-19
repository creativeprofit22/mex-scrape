# 0009. Onion architecture: an `app` layer with inward-owned ports

- Status: Accepted
- Date: 2026-10-19
- Amends: [0001](0001-rust-backend-nextjs-web.md)

## Context

ADR 0001 split the backend into small crates by responsibility, but did not say which way dependencies point between use cases and infrastructure. As the first flows appeared, the entry points started holding business logic and calling adapters directly. `api` defined its own `AccessSource` trait and called `retailers::retailer_for_host` itself. `worker` repeated the same per-retailer grouping against `db`. The next phases (Amazon lookups, matching and ROI, price history, the job queue) would copy this into more places. So we adopted the layering now, while only one flow had to move, instead of after several more.

## Decision

The backend follows the Onion architecture. Dependencies point inward only:

- **domain** (core): entities and pure rules. No I/O, no workspace dependencies.
- **app** (application layer): use cases and the port traits they need (`OfferSource`, `AmazonSource`, `PriceHistoryRepo`, `AccessCountsRepo`, `Clock`). It depends only on `domain` and non-I/O libraries (`thiserror`). Port signatures use only domain and app types, and errors are app-owned enums with no detail strings. Use cases take ports as constructor arguments (`Arc<dyn Port>`) and never construct adapters.
- **infrastructure** (adapters): `db`, `retailers`, `amazon`, `http-guard`, `brightdata`, `secrets`. They implement app ports (`db::access::PgAccessCounts`, `retailers::RetailerOfferSource`). They may depend on `app` and `domain`, never on an entry point.
- **entry points** (composition roots): `api` and `worker`. They build adapters, inject them into use cases, and map results to HTTP responses or logs.

`brightdata` stays a product-agnostic library outside the onion ([ADR 0006](0006-brightdata-agnostic-client-durable-runs.md)). It depends on neither `domain` nor `app`; `worker` bridges it to the app ports (later through a `Runner`-backed `retailers::PageFetcher`, so robots.txt checks and spend caps still apply to every Bright Data call).

Ports are async and object-safe without `async-trait`: methods return a boxed `app::PortFuture`.

`scripts/check_docs.py` (in CI) fails if `app` depends on an infrastructure or entry-point crate, or directly on `sqlx`, `axum`, `hyper`, `tower`, `utoipa` or an HTTP client crate.

The first flow moved is the Bright Data access report (`blocked_needs_kyc` and the other outcomes): `app::AccessReport` serves both the admin endpoint and the worker's start-up metrics.

Defined in `app` by later phases:

- the Amazon phase implements `AmazonSource`, and may refine its signature;
- the matching phase adds the match/ROI and price-history use cases and implements `PriceHistoryRepo`;
- the job-queue phase adds search/refresh orchestration and the `Runner`-backed page fetcher.

## Consequences

- New dependency directions: `db → app` and `retailers → app`. Both are listed in the allowed-dependencies table.
- Use cases are unit-tested with hand-written fakes and no database.
- `AmazonSource`, `PriceHistoryRepo` and `Clock` are interface-only for now; their signatures may change when they are first implemented.
- Boxed futures cost one allocation per port call, which is negligible next to network and database I/O.
