# 0017. Store Amazon catalog data seen during matching

- Status: Accepted
- Date: 2026-10-23
- Extends: [ADR 0010](0010-amazon-spapi-lwa-brightdata-dataset-fallback.md) (Amazon sources), [ADR 0009](0009-onion-architecture.md) (ports and adapters)

## Context

The product page and the detail panel showed only an ASIN: `offer_matches` stores the ASIN, and the Amazon title and brand that `app::ports::AmazonSource` returns during matching were dropped after scoring. A made-up ASIN also answered `200` with no listings, so the web app showed "no history yet" for a product that does not exist.

## Decision

- **What.** One global row per ASIN in `amazon_products(asin, title, brand, source, seen_at)` (migration `0010_amazon_products.sql`): the Amazon.com.mx product name (1–500 characters), its brand when known (1–200), which source gave it (`sp_api` or `bright_data`) and when it was seen. Like `offer_matches`, it is the same for every tenant.
- **From where.** Only from the catalog lookups the worker already makes while matching (`worker::offer_matching::match_and_store`): after a match is found, the `CatalogItem` of the matched ASIN (GTIN hits first) is normalised by `app::AmazonProduct::from_catalog` (whitespace collapsed, cut on character boundaries, blank title refused, blank brand dropped) and stored **before** the match, so every match written from now on has a name. No extra Amazon request is made. A storage failure fails the match (`OfferMatchError::Storage`).
- **How refreshed.** An upsert keyed by ASIN where the newest `seen_at` wins; an older lookup never overwrites a newer one, and a newer lookup without a brand keeps the stored brand. There is no backfill: older matches get a name the next time a search matches the listing.
- **API.** `GET /products/{asin}/price-history` and `/price-stats` (and the assistant's internal route) gain nullable `title` and `brand`; no new endpoint. An ASIN with no stored name, no match in any status and no listing visible to the reader answers `404 product_not_found`. A known ASIN with no history still answers `200` with empty listings, so the web app can tell "not found" from "no history yet".
- **Layers.** The port `app::AmazonProductRepo` and the pure `app::product` module hold the rules; `db::amazon_products::PgAmazonProducts` is the adapter; `api` only maps the result to HTTP.

## Consequences

- Product pages and the detail panel lead with the Amazon name; until a listing is re-matched they fall back to the ASIN heading.
- A demo database shows names only after the demo worker has run the queued searches (`docs/modules/db/demo-data.md`).
- Clients that relied on `200` for an unknown ASIN must handle `404` (the web app and the assistant sidecar do).
- Stored names can go stale if Amazon renames a product and no new match sees it; that is accepted, since a name is display-only.
