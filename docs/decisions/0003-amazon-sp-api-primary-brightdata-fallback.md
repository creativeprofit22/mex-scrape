# 0003. Amazon SP-API primary, Bright Data fallback

- Status: Accepted; superseded in part by [ADR 0010](0010-amazon-spapi-lwa-brightdata-dataset-fallback.md) (the fallback is the Bright Data Amazon dataset inside `amazon`, not a Web Unlocker page fetch orchestrated by `worker`)
- Date: 2026-10-18

## Context

ROI needs reliable Amazon.com.mx prices, fees and catalog data. The Selling Partner API (SP-API) provides official data but has rate limits, requires seller authorization, and does not cover every case.

## Decision

The `amazon` crate uses SP-API as the primary source. When SP-API cannot answer (missing data, throttling, no authorization), the worker may fall back to fetching the public Amazon.com.mx page through `brightdata`. Results record which source produced them.

## Consequences

- Two code paths to keep consistent; source is tracked per data point.
- SP-API credentials and refresh tokens are secrets handled outside the repo.
- `amazon` depends only on `domain` and `http-guard`; the fallback is orchestrated by `worker`, keeping adapters independent.
