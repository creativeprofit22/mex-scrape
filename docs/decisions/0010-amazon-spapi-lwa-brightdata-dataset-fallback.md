# 0010. Amazon: SP-API with LWA tokens, Bright Data dataset fallback in `amazon`

- Status: Accepted
- Date: 2026-10-19
- Supersedes in part: [ADR 0003](0003-amazon-sp-api-primary-brightdata-fallback.md) (how the fallback works and where it lives)

## Context

ADR 0003 made the Selling Partner API (SP-API) primary and planned a fallback that fetches public Amazon.com.mx pages through the Web Unlocker, orchestrated by `worker`, with `amazon` depending only on `domain` and `http-guard`.

Since then:

- Amazon dropped AWS Signature V4 / IAM for SP-API (October 2023). A call is authorized by a Login with Amazon (LWA) access token in the `x-amz-access-token` header.
- Bright Data offers a structured "Amazon products global dataset" (`gd_lwhideng15g8jg63s7`) through Datasets v3, which already runs under the `brightdata` crate's kill switch, spend caps and durable runs. Parsing raw Amazon pages ourselves would duplicate that and break often.
- Bright Data's published input schema (`scrapers-full.json`, checked 2026-10-19) lists `collect_by_url` and `discover_by_keywords` (with a `domain` field, e.g. `https://www.amazon.com.mx`) for that dataset, but **no `discover_by_upc`**. Barcode discovery exists only on the amazon.com-only dataset `gd_l7q7dkf244hwjntr0`, which returns US data.

## Decision

- **No SigV4.** `amazon` refreshes an LWA access token (`POST https://api.amazon.com/auth/o2/token`, `grant_type=refresh_token`) and sends it as `x-amz-access-token` to `sellingpartnerapi-na.amazon.com` with marketplace `A1AM78C64UM0Y8` (Mexico).
- **Fallback inside `amazon`.** `amazon::BrightDataAmazonSource` implements `app::ports::AmazonSource` over dataset `gd_lwhideng15g8jg63s7`, driven through `brightdata::runs::Runner` (gates, caps, durable runs). `amazon` may therefore depend on `brightdata` (and on `app` for the port). `worker` only builds both sources and wraps them in `app::FallbackAmazonSource`.
- **When the fallback is used.** Only when SP-API is not configured, not authorized or unavailable (`NotConfigured`, `NotAuthorized`, `Unavailable`). Rate limits, parse failures, bad input and unsupported lookups are returned as-is, so a busy SP-API never triggers paid Bright Data calls.
- **Barcode lookups in the fallback.** The fallback runs `discover_by_keywords` on `https://www.amazon.com.mx` with the barcode as the keyword and keeps only records whose `upc` field equals that barcode. Anything else is dropped (fail closed); no match means an empty result, never a guess.
- **Fees** are not available from Bright Data; the fallback returns `Unsupported`.
- **MXN only.** Every money value must be in MXN. SP-API fee estimates are always requested with `CurrencyCode: "MXN"` (the API's default is USD), and any non-MXN amount in a response is a parse failure. Bright Data records not on `amazon.com.mx` or not priced in Mexican pesos are dropped; on `amazon.com.mx` records Bright Data labels pesos `MXD` (seen live 2026-10-19, not an ISO code), which counts as MXN. We never convert currencies.
- Results carry `AmazonDataSource { SpApi, BrightData }` so callers know which provider answered (ADR 0003's per-data-point source tracking).

## Consequences

- `amazon`'s allowed dependencies become `domain, app, http-guard, brightdata`. `brightdata` stays product-agnostic (ADR 0006): the Amazon-specific dataset id and record mapping live in `amazon`.
- The owner must register an SP-API developer app and self-authorize it before SP-API data flows; until then the fallback serves catalog and offer lookups (not fees). Steps are in [`docs/modules/amazon.md`](../modules/amazon.md).
- Barcode matching through the fallback finds fewer products than SP-API does (only listings whose page shows the barcode).
- Whether the dataset really returns amazon.com.mx data in MXN is checked live and recorded in [`docs/verification/amazon-brightdata-mx.md`](../verification/amazon-brightdata-mx.md).
