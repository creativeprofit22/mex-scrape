# app

## Purpose

The application layer ([ADR 0009](../decisions/0009-onion-architecture.md)): use cases and the ports (traits) they need. It does no I/O. Infrastructure crates implement the ports, and the entry points (`api`, `worker`) build the adapters and inject them.

## Public API

- `PortFuture<'a, T>`: the boxed `Send` future async port methods return, so every port is object-safe (`Arc<dyn Port>`).
- Ports (`app::ports`, all `Send + Sync`):
  - `Clock { now() -> UnixMillis }`. Injected so use cases stay deterministic.
  - `OfferSource`: `retailer_for_host(host) -> Option<RetailerId>` (sync registry lookup), `search(&RetailerId, query) -> Vec<Offer>`, `product(&RetailerId, &RetailerSku) -> Offer`. Errors: `OfferSourceError { UnknownRetailer, InvalidQuery, InvalidSku, SearchUnavailable, BlockedNeedsKyc, FetchFailed, ParseFailed, Unavailable }`. Implemented by `retailers::RetailerOfferSource`.
  - `AmazonSource`: Amazon.com.mx lookups, all money in MXN. `catalog_by_gtin(&Gtin)` and `catalog_by_keyword(&str)` return `CatalogResult { items: Vec<CatalogItem>, source, fetched_at }`, where `CatalogItem { asin: Asin, title, brand, images, sales_ranks: Vec<SalesRank { category, rank }> }`. `offers(&Asin) -> OfferSummary { buy_box_mxn, lowest_new_mxn, offer_count, source, fetched_at }`. `fees(&Asin, PriceMxn, Fulfillment { Fba, Merchant }) -> FeeEstimate { total_mxn, details: Vec<FeeDetail { fee_type, amount_mxn }>, source, fetched_at }`. `source` is `AmazonDataSource { SpApi, BrightData }`. Errors: `AmazonSourceError { NotConfigured, NotAuthorized, InvalidInput, Unsupported, Unavailable, RateLimited, ParseFailed, Refused }` (`Refused`: the spending guard said no; it never falls back). Implemented by `amazon::SpApiAmazonSource` and `amazon::BrightDataAmazonSource`.
  - `PriceHistoryRepo`: `record(&Offer)`, `history(&RetailerId, &RetailerSku, since) -> Vec<PricePoint>`. `PricePoint { price_mxn, list_price_mxn, availability, observed_at }`, sorted by `observed_at`. Implemented by `db::price_history::PgPriceHistory`.
  - `MatchRepo`: `upsert(&ProductMatch) -> UpsertOutcome` (`Written`, or `KeptRejected` when a stored `rejected` match was kept and nothing was written — a reviewer's rejection is never overwritten), `accepted_listings(&Asin) -> Vec<(RetailerId, RetailerSku)>` (sorted by retailer, then SKU). Implemented by `db::price_history::PgMatches`.
  - `AccessCountsRepo`: `counts_by_host() -> Vec<HostAccess>`. `HostAccess { host, totals: AccessTotals }`, `AccessTotals { ok, blocked_needs_kyc, skipped_robots, failed }` (with saturating `add`). Implemented by `db::access::PgAccessCounts`.
  - `RequestBudget { consume(&TenantId, n) }`: the per-tenant daily budget of paid Amazon requests, checked together with the global kill switch. Errors: `BudgetError { KillSwitch, CapReached, Unavailable }`; callers fail closed on all three. Implemented by `db::usage::PgRequestBudget`.
  - `SearchRepo`: `create_and_enqueue(&TenantId, &SearchQuery) -> SearchId` (row and job message in one transaction) and `load(&TenantId, SearchId) -> Option<SearchView>` (tenant-scoped). `SearchView { id, query, status, reason, created_at, finished_at, retailers: Vec<RetailerOutcomeView { retailer, outcome, offers }>, results: Vec<SearchResultView { retailer, retailer_sku, title, url, price_mxn, amazon: Option<ResultMatchView { asin, status, confidence, amazon_price_mxn, fees_mxn, profit_centavos, roi_bp, margin_bp }> }> }`. Implemented by `db::searches::PgSearches`.
    `SearchView` also has `hidden_results` (set by `SearchResults::get`), and each `SearchResultView` has `availability` and `membership_required`.
  - `UserRepo` (ADR 0012): `create(&Email, password_hash, Locale) -> UserRecord` (plan `free`, not onboarded) (`CreateUserError { EmailTaken, Unavailable }`), `find_credentials(&Email) -> Option<UserCredentials { user, password_hash }>` (its `Debug` hides the hash), `find_by_id(UserId)`, `set_memberships(UserId, Memberships)` (also marks the user onboarded) and `set_locale(UserId, Locale)`, both `Option<UserRecord>` (`None` if the user is gone). `UserRecord { id, email, tenant, plan, memberships, onboarded, locale }`; the tenant is always `TenantId::for_user(id)`. Implemented by `db::users::PgUsers`.
  - `PasswordHasher`: `hash(password)` (`HashError`), `verify(password, hash) -> bool`, `dummy_hash()` (a valid hash of a random password, for unknown e-mails). Implemented by `api::auth::hasher::Argon2Hasher`.
  - `RepoError { Unavailable }`: the repository error.
  - None of the error enums carry detail strings, so adapter errors (which may echo URLs or credentials) never cross a port.
- Use cases:
  - `amazon::FallbackAmazonSource::new(primary, fallback)` (both `Option<Arc<dyn AmazonSource>>`), itself an `AmazonSource`. It asks the primary (SP-API) first and the fallback (Bright Data) only when the primary is missing or fails with `NotConfigured`, `NotAuthorized` or `Unavailable`. `RateLimited`, `ParseFailed`, `InvalidInput` and `Unsupported` are returned as-is, so a busy SP-API never triggers paid fallback calls. With neither source it returns `NotConfigured`.
  - `amazon::GatedAmazonSource::new(inner, Arc<dyn RequestBudget>, TenantId)`, an `AmazonSource` that consumes one request from the tenant's budget before every call and returns `Refused` without calling `inner` on any budget error. The worker wraps its `FallbackAmazonSource` in it per job, so SP-API and the Bright Data fallback are both capped.
  - `search::StartSearch::new(Arc<dyn SearchRepo>)`: `start(&TenantId, raw)` cleans the query (`domain::SearchQuery`) and enqueues it. `SearchResults::new(Arc<dyn SearchRepo>)`: `get(&TenantId, SearchId, Memberships) -> Option<SearchView>` drops results the memberships do not allow (`domain::Memberships::allows`) and counts them in `hidden_results`. Errors: `SearchError { InvalidQuery(SearchQueryError), Storage }`; nothing is stored for an invalid query.
  - `access::AccessReport::new(Arc<dyn AccessCountsRepo>, Arc<dyn OfferSource>)`, the Bright Data access report.
    - `per_host()` returns `Vec<HostAccessReport { retailer, host, totals }>` in the repository's order. The label is the retailer id from `OfferSource::retailer_for_host`, or the host when it is not registered.
    - `per_retailer()` returns a `BTreeMap<String, AccessTotals>` summed per label and sorted.
    - Both fail with `AccessReportError::Storage` when the counts cannot be read.
    - Unit tests use hand-written fakes, with no database.
  - `price_stats::PriceStats::new(Arc<dyn PriceHistoryRepo>, Arc<dyn MatchRepo>, Arc<dyn Clock>)`, the Keepa-style price stats. `for_product(&Asin, Memberships)` returns `ProductPriceStats { asin, as_of, listings: Vec<ListingPriceStats { retailer, retailer_sku, windows: Vec<PriceWindow> }>, hidden_listings }`: for each listing with an accepted match to the ASIN, it reads the last 365 days of history and computes `domain::window_stats` for 30, 90 and 365 days at `Clock::now()`. Listings whose retailer is a membership store (`domain::is_membership_retailer`) and not allowed by `Memberships::allows` are dropped and counted in `hidden_listings` (ADR 0012, same rule as `SearchResults::get`). Fails with `PriceStatsError::Storage`. Tests use fixed fixture data and a fixed clock.
  - `price_series::PriceSeries::new(Arc<dyn PriceHistoryRepo>, Arc<dyn MatchRepo>, Arc<dyn Clock>)`: `for_product(&Asin, days, Memberships)` returns `ProductPriceSeries { asin, since, as_of, listings: Vec<ListingPriceSeries { retailer, retailer_sku, points: Vec<DailyPrice { day, low_mxn, high_mxn, in_stock }> }>, hidden_listings }`, one point per UTC day with observations, oldest first, for the product chart, with the same membership hiding as `PriceStats`. `days` must be 1–`MAX_SERIES_DAYS` (365), else `PriceSeriesError::InvalidDays`; `Storage` on a read error.
  - `roi::RoiBatch::calculate(&[RoiInput]) -> Result<Vec<Result<RoiResult, RoiError>>, RoiBatchError>`: `domain::calculate_roi` per item, in order; one bad item gets its own `RoiError` without failing the batch. The batch needs 1–`MAX_ROI_BATCH` (100) items (`RoiBatchError { Empty, TooMany }`). Backs `POST /roi`, so the calculator and search pages agree with the worker.
  - `account` (ADR 0012), all over `Arc<dyn UserRepo>`:
    - `SignUp::new(users, hasher)`: `sign_up(email, password, Locale)` checks `domain::Email` and `check_new_password`, hashes, creates. `SignUpError { InvalidEmail, WeakPassword(PasswordPolicyError), EmailTaken, Unavailable }`.
    - `SignIn::new(users, hasher)`: `sign_in(email, password) -> UserRecord` or `SignInError::InvalidCredentials` for an invalid or unknown e-mail, an over-long password or a wrong password. Unknown e-mails still verify against `dummy_hash()`, so timing does not reveal which accounts exist. `Unavailable` on storage errors.
    - `Profile::new(users)`: `get(UserId)`, `set_locale(UserId, Locale)` (`ProfileError { NotFound, Unavailable }`). `UpdateOnboarding::new(users)`: `set(UserId, Memberships)`.

## Dependencies

- Workspace: `domain` only. `scripts/check_docs.py` fails if `app` depends on an infrastructure or entry-point crate, or directly on `sqlx`, `axum`, `hyper`, `tower`, `utoipa` or an HTTP client crate.
- External: `thiserror`. Dev: `tokio` (async tests).

## Owned tables

None.
