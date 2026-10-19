# amazon

## Purpose

Amazon.com.mx catalog, offer and fee data ([ADR 0003](../decisions/0003-amazon-sp-api-primary-brightdata-fallback.md), [ADR 0010](../decisions/0010-amazon-spapi-lwa-brightdata-dataset-fallback.md)). Two implementations of `app::ports::AmazonSource`:

- `SpApiAmazonSource`: Amazon's Selling Partner API (primary).
- `BrightDataAmazonSource`: Bright Data's "Amazon products global dataset" `gd_lwhideng15g8jg63s7` through Datasets v3 (fallback).

`app::FallbackAmazonSource` combines them; `worker` builds it from server secrets ([worker.md](worker.md)).

## Public API

- `SpApiConfig::from_secrets(client_id, client_secret, refresh_token)`: `ConfigError::{MissingClientId, MissingClientSecret, MissingRefreshToken}` (the first one missing, in that order; messages name the env var). Defaults: production endpoint, 30 s timeout, `RetryPolicy::default()`, the rate table below. `with_endpoint(SpApiEndpoint::Sandbox)` targets the sandbox.
- `SpApiClient::new(config)`: one `GuardedClient` for LWA (`api.amazon.com` only) and one for the SP-API host only. The client redacts the three credentials and every access token itself.
  - `search_catalog_by_identifiers(&[Gtin])` (1 to `MAX_IDENTIFIERS` = 20, all of one `IdentifierType`) and `search_catalog_by_keyword(&str)` (1 to `MAX_KEYWORD_CHARS` = 200 printable characters).
  - Errors are `SpApiError { NotAuthorized, RateLimited, Transient, Invalid, Parse, Build }`; `to_source_error()` maps them to the port error.
- `SpApiAmazonSource::new(client, clock)`: the client as an `AmazonSource`. Offers and fees are public only through it: `AmazonSource::offers` and `AmazonSource::fees` (`Fulfillment::Fba` sets `IsAmazonFulfilled`).
- `BrightDataAmazonSource::new(transport, run_store, clock)`; `with_timing(Timing)` changes poll timing.
- `RateLimiter`, `per_request_from_header`, `Operation`, `RateLimit`, `RateTable`.
- Constants: `MX_MARKETPLACE_ID`, `SPAPI_NA_HOST`, `SPAPI_NA_SANDBOX_HOST`, `LWA_HOST`, `LWA_TOKEN_URL`, `MXN`, `AMAZON_GLOBAL_DATASET_ID`, `AMAZON_MX_ORIGIN`, `LIMIT_PER_INPUT`, `FALLBACK_TENANT`, `FALLBACK_JOB`.

## SP-API

### Authentication

Since October 2023 SP-API calls need no AWS SigV4 signature or IAM role. A call is authorized by a Login with Amazon (LWA) access token in the `x-amz-access-token` header.

- Token refresh: `POST https://api.amazon.com/auth/o2/token`, form body `grant_type=refresh_token`, `refresh_token`, `client_id`, `client_secret` (sent with `GuardedRequest::form`; the secret values are redacted).
- The token (valid 3600 s) is cached and reused until 60 s before it expires. Concurrent callers share one refresh.
- A 401 from SP-API drops the cached token and retries once with a fresh one; a second 401 is `NotAuthorized`.
- LWA `invalid_grant`, `invalid_client`, `unauthorized_client` or `access_denied` ⇒ `NotAuthorized`; LWA 5xx or timeouts ⇒ `Transient`.
- Every request sends `user-agent: mex-scrape/<version> (Language=Rust)`.

### Endpoints

Host: `sellingpartnerapi-na.amazon.com` (production) or `sandbox.sellingpartnerapi-na.amazon.com` (sandbox). Marketplace: Mexico, `A1AM78C64UM0Y8`, sent on every call.

| Call | Request | Notes |
|---|---|---|
| Catalog by barcode | `GET /catalog/2022-04-01/items?marketplaceIds=A1AM78C64UM0Y8&identifiers=…&identifiersType=UPC\|EAN\|GTIN&includedData=summaries,images,salesRanks,identifiers` | Type from GTIN length: 12 ⇒ `UPC`, 13 ⇒ `EAN`, otherwise `GTIN`. |
| Catalog by keyword | `GET /catalog/2022-04-01/items?marketplaceIds=…&keywords=…&includedData=…` | |
| Offers | `GET /products/pricing/v0/items/{asin}/offers?MarketplaceId=A1AM78C64UM0Y8&ItemCondition=New` | Buy box price, lowest new price, offer count. |
| Fees | `POST /products/fees/v0/items/{asin}/feesEstimate` | Body always has `CurrencyCode: "MXN"`; `IsAmazonFulfilled` from `Fulfillment::Fba`. |

Catalog results give ASIN, title, brand, images and sales ranks (classification and display-group ranks).

**MXN only.** The fees API defaults to USD, so the request always names MXN. Every money value in any response must be `MXN`; any other currency fails the whole result (`Parse`). Nothing is ever converted.

### Rate limits

Each operation has its own limiter (a token bucket, burst = bucket size). Values from Amazon's reference pages, checked 2026-10-19:

| Operation | Rate | Burst |
|---|---|---|
| `searchCatalogItems` | 2 req/s | 2 |
| `getItemOffers` | 0.5 req/s | 1 |
| `getMyFeesEstimateForASIN` | 1 req/s | 2 |

If a response's `x-amzn-RateLimit-Limit` header reports a slower rate, the limiter adopts it (never a faster one).

### Retries and errors

- 429: wait for the operation's bucket, then back off 1 s, 2 s, 4 s (`RetryPolicy`: at most 3 retries, each delay capped at 30 s, a numeric `Retry-After` honoured up to the cap). Still 429 ⇒ `RateLimited`.
- 5xx, timeouts, transport errors: same backoff (all calls are reads; the fees POST is idempotent). Still failing ⇒ `Transient`.
- 401: one forced token refresh, then `NotAuthorized`. 403 ⇒ `NotAuthorized` (often a missing role).
- 404 ⇒ empty catalog result or an empty offer summary. Other 4xx ⇒ `Invalid`.
- Response bodies are capped at 8 MiB. Error text never contains a URL, a token or a credential.

Port mapping: `NotAuthorized` ⇒ `NotAuthorized`; `RateLimited` ⇒ `RateLimited`; `Transient`/`Build` ⇒ `Unavailable`; `Invalid` ⇒ `InvalidInput`; `Parse` ⇒ `ParseFailed`.

Known Amazon issue: `getItemOffers` can return stale prices for some ASINs.

### Required roles

From Amazon's [role mappings](https://developer-docs.amazon/sp-api/docs/role-mappings.md) (checked 2026-10-19):

| Operation | Roles (any one) |
|---|---|
| `searchCatalogItems` (Catalog Items 2022-04-01) | Product Listing |
| `getItemOffers` (Product Pricing v0) | Pricing, Product Listing |
| `getMyFeesEstimateForASIN` (Product Fees v0) | Product Listing, Pricing, or several others |

So **Product Listing** alone covers all three. Request **Pricing** as well; it is harmless and keeps offers and fees working if roles change.

## Owner setup: SP-API registration

Until these steps are done the worker uses the Bright Data fallback only. They must be done by the **Primary User** of the Amazon.com.mx Seller Central account. Source: Amazon's [Register as a Private SP-API Developer](https://developer-docs.amazon/sp-api/docs/register-as-a-private-developer.md), [Register your Application](https://developer-docs.amazon/sp-api/docs/registering-your-application.md) and [Authorize Private Applications](https://developer-docs.amazon/sp-api/docs/self-authorization.md) (checked 2026-10-19).

1. Sign in to Seller Central (sellercentral.amazon.com.mx). If the account already moved to the Solution Provider Portal, sign in at solutionproviderportal.amazon.com instead and open Settings ⇒ Developer Profile.
2. Open the menu ⇒ **Apps and Services** ⇒ **Develop Apps**, and fill in the developer profile:
   - contact information;
   - Data Access: **Private Developer: I build application(s) that integrate my own company with Amazon Services APIs**;
   - Roles: **Product Listing** and **Pricing** (no restricted roles);
   - use cases and security controls; accept the agreements; choose **Register**.
3. Wait for Amazon's review. Updates go to the admin email; answer any question **within five days** or the case is closed.
4. After approval, on Develop Apps choose **Add new app client**: API type SP-API, **Sellers**, the same two roles. Private apps can stay in draft.
5. On the app, choose **Authorize app** (self-authorization). Copy the **refresh token** it shows.
6. On the app, view the LWA credentials and copy the **client id** and **client secret**.
7. Put the three values in the server's secrets (never in git, chat or logs):
   - `SPAPI_LWA_CLIENT_ID`
   - `SPAPI_LWA_CLIENT_SECRET`
   - `SPAPI_REFRESH_TOKEN`
8. Restart the worker. Its log shows `sp_api=true`.
9. Optional: run the sandbox check below to prove the credentials work.

Amazon expires LWA client secrets periodically; rotate it in the portal when asked and update `SPAPI_LWA_CLIENT_SECRET`.

### Sandbox

```bash
SPAPI_LWA_CLIENT_ID=... SPAPI_LWA_CLIENT_SECRET=... SPAPI_REFRESH_TOKEN=... \
  cargo test -p amazon --test live_sandbox -- --ignored --nocapture
```

One token refresh and one catalog search against `sandbox.sellingpartnerapi-na.amazon.com` (no retries). The static sandbox only answers the request shapes Amazon documents, so a 400 still proves the token and authorization work; 401/403 means wrong credentials or a missing role. It prints only the result kind.

## Bright Data fallback

Dataset `gd_lwhideng15g8jg63s7` ("Amazon products global dataset") via `brightdata::runs::Runner`, so the kill switch, daily caps and durable runs apply. Run scope: tenant `system`, job `amazon_mx_fallback`; one call at a time (a scope allows one active run). The worker builds the fallback only while it holds the Bright Data driver lock; other workers use SP-API only ([worker.md](worker.md)). A run still active when the poll budget ends is cancelled and returns `Unavailable`. A run interrupted by a worker stop is failed (not resumed) at the next start-up, so it never holds the scope's active slot ([worker.md](worker.md)).

| Port call | Dataset request |
|---|---|
| `catalog_by_gtin` | `discover_by=keywords`, input `{ keywords: <GTIN>, domain: "https://www.amazon.com.mx", pages_to_search: 1 }`, `limit_per_input = 5`; keeps only records whose `upc` equals the GTIN (leading zeros ignored). The dataset has no UPC discovery for amazon.com.mx (`discover_by_upc` exists only on the US-only dataset). |
| `catalog_by_keyword` | `discover_by=keywords`, same domain, `limit_per_input = 5`. |
| `offers` | collect by URL `https://www.amazon.com.mx/dp/{asin}`. |
| `fees` | `Unsupported` (the dataset has no fee data). |

Every record must come from `amazon.com.mx` and, when it has a currency, be Mexican pesos; others are dropped (fail closed). Bright Data labels pesos `MXD` on amazon.com.mx records (live check, 2026-10-19); that label is not an ISO code and is accepted as MXN only on amazon.com.mx records. Results carry `source = BrightData`.

Live result (2026-10-19): the dataset does return amazon.com.mx records with a price and an ASIN, but the record had **no `upc`**. So keyword and offer lookups work, while barcode lookups through the fallback will often find nothing (an empty result, never a guess). SP-API is needed for reliable barcode matching.

### When the fallback is used

`app::FallbackAmazonSource` calls SP-API first and uses Bright Data only when SP-API is not configured or answers `NotConfigured`, `NotAuthorized` or `Unavailable`. `RateLimited`, `ParseFailed`, `InvalidInput` and `Unsupported` are returned as-is, so a slow SP-API never causes paid Bright Data runs.

### Live check

```bash
BRIGHTDATA_API_KEY=... BRIGHTDATA_UNLOCKER_ZONE=... \
  cargo test -p amazon --test live_brightdata_mx -- --ignored --nocapture
```

One keyword discovery on amazon.com.mx with `limit_per_input = 1` (at most one billed record), free progress polls and one download. Prints only statuses, the record count, domains and currencies. Results: [amazon-brightdata-mx.md](../verification/amazon-brightdata-mx.md).

## Tests

- `src/client/tests.rs`: a loopback fake server with the recorded fixtures in `tests/fixtures/` (synthetic ids, shaped after Amazon's model examples). Covers token caching and refresh, the `x-amz-access-token` header, the marketplace id, MXN in the fees body, USD rejection, 429 backoff under paused time, per-operation spacing, 401 ⇒ one refresh ⇒ `NotAuthorized`, and no secret in errors.
- `src/brightdata_source/tests.rs`: a fake `Transport` and an in-memory `RunStore`. Covers the domain/currency filter, UPC matching, `Unsupported` fees and gate refusal ⇒ `Unavailable`.
- `tests/live_sandbox.rs`, `tests/live_brightdata_mx.rs`: ignored live checks above.

No test uses real credentials; credentials come only from the server's environment.

## Dependencies

- Workspace: `domain`, `app` (the port), `http-guard` (all HTTP), `brightdata` (the fallback).
- External: `serde`, `serde_json`, `thiserror`, `tokio`.
- Dev: `http-guard` with `test-support`.

## Owned tables

None. Fallback runs are stored by `worker`'s `PgRunStore` (`brightdata_runs`).
