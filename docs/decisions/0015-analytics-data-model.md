# 0015. Analytics data model: listing and Amazon dimensions, append-only Amazon prices, analytics views

- Status: Proposed (for review by the repo owner; becomes Accepted when merged unchanged)
- Date: 2026-10-21

## Summary

1. Today we keep store prices over time, but **Amazon prices are not kept**: a search stores only the Amazon price and fee it saw, in its own result row, and a retry overwrites them.
2. Product details we already fetch are **thrown away**: the store's brand and barcode (GTIN), and Amazon's title and categories.
3. We add three tables: one row per store listing, one row per Amazon product, and an **append-only log** of every Amazon price we fetch. A daily, budget-capped job re-prices the Amazon products users actually look at.
4. On top of these we add a read-only `analytics` schema of views. BI tools query only the views, never the app's tables.
5. Why: to see how Amazon prices move, where the arbitrage is (by category and by product), and which stores price in ways that help a buyer.

## Questions this answers

These four questions are first examples to shape the model; the real analysis questions still have to be agreed with the client. The tables keep general history (listings, Amazon products, an append-only price log), so new questions should mostly need new views, not new tables.

| Question | Answered by | How |
|---|---|---|
| How much does a product's Amazon price move over a month? | `analytics.amazon_prices` (`amazon_price_observations` + `amazon_products`) | Min, max and spread of `sell_price_mxn` per ASIN over 30 days, from search fetches plus the daily re-pricing. |
| Best arbitrage opportunities by product category | `analytics.current_opportunities` | Median `roi_pct` and the number of profitable ASINs per `display_group` (Amazon's broad category), each ASIN counted once (rule 7). |
| Best arbitrage opportunities by product | `analytics.current_opportunities` | Accepted matches ordered by `profit_mxn` or `roi_pct`, filtered on `actionable` and `membership_required IS NOT TRUE`, with `display_group_rank` as a demand filter. |
| Which stores have price movements that help me buy (volatile, with lows below other stores)? | `analytics.listing_prices` (`price_observations` + `retailer_listings` + `offer_matches`) | Per store, from `in_stock`/`unknown` rows only: price volatility (spread ÷ mean per listing over 30 days) and, for products several stores sell, how often the store's 30-day low is the lowest. Products group by accepted ASIN; a listing without one joins the ASIN of an accepted listing with the same GTIN, else groups by GTIN. |

## Context

Checked against the code on 2026-10-21:

- **Amazon prices and fees are snapshots only.** They live only in `search_results.amazon_price_mxn` and `fees_mxn` ([0006_jobs.sql](../../crates/db/migrations/0006_jobs.sql)), which `upsert_result` overwrites on a retry ([searches.rs](../../crates/db/src/searches.rs)). `price()` in the search job keeps the buy box (else the lowest new offer) and the fee total; it drops the offer count, the source and the fetch time ([search.rs](../../crates/worker/src/jobs/search.rs), `OfferSummary` in [ports.rs](../../crates/app/src/ports.rs)). The product-refresh and category jobs never fetch Amazon prices.
- **Brand, GTIN and Amazon details are never stored.** `Offer` has `brand` and `gtin` ([offer.rs](../../crates/domain/src/offer.rs)), but `PgPriceHistory::record` writes only retailer, SKU, price, list price, availability and time ([price_history.rs](../../crates/db/src/price_history.rs)), and no migration has a brand or GTIN column. Amazon's `CatalogItem` (title, brand, sales ranks) is used for matching and then dropped.
- **Amazon's category kinds are mixed together.** SP-API merges `displayGroupRanks` (broad, e.g. a whole department) and `classificationRanks` (narrow) into one list sorted by rank ([parse.rs](../../crates/amazon/src/parse.rs) L122–140). The Bright Data path does the same with `root_bs_*` and `bs_*` ([brightdata_source.rs](../../crates/amazon/src/brightdata_source.rs) L346–358). Narrow ranks are usually the smaller numbers, so the "best rank" is usually a narrow classification, which would split products into tiny groups.
- **Store prices are already kept.** `price_observations` is append-only, enforced by triggers that refuse UPDATE, DELETE and TRUNCATE ([0004_matching_price_history.sql](../../crates/db/migrations/0004_matching_price_history.sql)). The product refresh records tracked listings on `REFRESH_CRON` (every 6 hours by default).
- Money in `search_results` and `price_observations` is in centavos despite the `_mxn` suffix. The store list (12 stores, 3 of them category-only) exists only in code ([registry.rs](../../crates/retailers/src/registry.rs)).

## Decision

- `db` owns the tables: raw SQL with sqlx, no ORM. New ports go in `app`, `db` implements them and `worker` wires them. `domain` stays pure ([ADR 0009](0009-onion-architecture.md)). No Rust code reads the views.
- Append-only tables share one `append_only()` trigger function, a generalised version of the `price_observations` triggers.
- New money columns end in `_centavos` and times are `TIMESTAMPTZ`. `retailer`, SKU and ASIN keep today's CHECKs.
- The app keeps reading today's tables, so the API does not change.

### `retailer_listings`: one row per store listing, latest value wins

| Column | Type and rule |
|---|---|
| `retailer`, `retailer_sku` | Primary key |
| `title` | TEXT NULL, 1–500 |
| `brand` | TEXT NULL, 1–200; a known value is never overwritten with NULL |
| `gtin` | TEXT NULL, `^([0-9]{8}\|[0-9]{12,14})$`; never overwritten with NULL (`domain::Gtin` checks the check digit) |
| `url` | TEXT NULL, 1–2048 |
| `membership_required` | BOOL NULL; NULL = unknown (a backfilled listing never seen in a search) |
| `first_seen_at`, `last_seen_at` | TIMESTAMPTZ; `last_seen_at >= first_seen_at` and only moves forward |

- **Writer:** `PriceHistoryRepo::record` (search, refresh and category jobs) upserts the listing in one transaction with the `price_observations` insert.
- **Backfill:** listings in `price_observations`, `offer_matches`, `tracked_products`, `search_results`, `match_decisions` and `match_suggestions`. Title, URL and membership come from the newest `search_results` row (rule 3), else NULL; brand and GTIN stay NULL because they were never stored.

### `amazon_products`: one row per ASIN, latest known value wins (rule 5)

| Column | Type and rule |
|---|---|
| `asin` | Primary key |
| `title` | TEXT NULL, 1–500 |
| `brand` | TEXT NULL, 1–200 |
| `display_group`, `display_group_rank` | TEXT NULL / INT NULL ≥ 1: broad category and its rank (SP-API display group; Bright Data `root_bs_*`). Question 2 groups by this. |
| `classification`, `classification_rank` | TEXT NULL / INT NULL ≥ 1: narrow category and its rank, the best (lowest) classification rank (SP-API classification; Bright Data `bs_*`) |
| `source` | `sp_api` / `bright_data` / `backfill` |
| `first_seen_at`, `last_seen_at` | TIMESTAMPTZ |

- **Code change (rollout step 1):** `app::SalesRank` gets a kind (display group or classification), and both parsers in `amazon` set it instead of merging the lists. Nothing outside `amazon` reads sales ranks today, so the change is small. The mapping of Bright Data's `root_bs_*` / `bs_*` fields is checked against a live record in the same PR.
- **Writer:** `match_and_store` upserts every candidate from `catalog_by_gtin` / `catalog_by_keyword`. Those lookups are already paid for.
- **Backfill:** the ASINs in `offer_matches`, `search_results`, `match_decisions` and `match_suggestions`, with the other columns NULL and `source = 'backfill'`.

### `amazon_price_observations`: append-only, one row per Amazon offers fetch

| Column | Type and rule |
|---|---|
| `id` | BIGINT identity, primary key |
| `asin` | FK → `amazon_products` |
| `sell_price_centavos` | NULL, ≥ 0: the buy box, else the lowest new offer (the price ROI uses) |
| `buy_box_centavos`, `lowest_new_centavos` | NULL, ≥ 0 |
| `offer_count` | INT NULL, ≥ 0 |
| `source` | `sp_api` / `bright_data` / `backfill` |
| `observed_at` | TIMESTAMPTZ: the fetch time |
| `search_id` | NULL, FK → `searches` (NULL for re-pricing rows) |
| `UNIQUE (search_id, asin)` | Dedupes search rows (rule 4's `ON CONFLICT` relies on it). Re-pricing rows have a NULL `search_id` and are not constrained, because NULLs are distinct. |

- **CHECK:** unless `source = 'backfill'`, `offer_count` is set and `sell_price_centavos` equals `COALESCE(buy_box_centavos, lowest_new_centavos)`.
- **Index:** `(asin, observed_at)`.
- **Writers:**
  - The search job, once per ASIN per search (several offers can match one ASIN; this also saves repeated offers calls).
  - The daily re-pricing below.
  - A row is written even when there is no price, because "no offers" is a data point too.
- **Backfill:** one row per `(search_id, asin)` among `search_results` rows with an Amazon price, `observed_at = COALESCE(searches.finished_at, searches.updated_at)` (an approximation), `source = 'backfill'`.

### Daily Amazon re-pricing of tracked ASINs (rollout step 2b)

Searches alone give one Amazon price per search, too sparse for question 1. The scheduled refresh therefore also fetches Amazon offers, within these limits:

- **Which ASINs:** the existing `tracked_products` set, filtered by recent searches. No new selection rule is added.
  - `tracked_products` is filled only by `db::tracking::track`, which the search and category jobs call for `Accepted` matches ([search.rs](../../crates/worker/src/jobs/search.rs) L303–306, [category.rs](../../crates/worker/src/jobs/category.rs) L81–84, [tracking.rs](../../crates/db/src/tracking.rs)).
  - Rows are never deleted and carry no ASIN, so re-pricing joins `offer_matches` for the ASIN and keeps only matches still `accepted`.
  - It also requires that the listing appeared in a search in the last `AMAZON_REPRICE_RECENT_DAYS` days (default 60). Listings tracked only by category browses are therefore skipped until a user searches them.
- **How often:** an ASIN is re-priced only when its newest `amazon_price_observations` row is older than `AMAZON_REPRICE_INTERVAL_HOURS` (default 24). The refresh still ticks every 6 hours, but each ASIN is priced at most once per interval, oldest first.
- **Budget:** calls go through the existing `app::GatedAmazonSource`. It takes one request from a `provider_usage` budget before every call and fails closed, and it wraps the SP-API → Bright Data fallback, so the cap covers both sources.
  - Re-pricing uses its own budget tenant (`system:reprice`) with its own cap, `AMAZON_REPRICE_REQUESTS_PER_DAY`, so it never eats the budget of category browses (tenant `system`) or of users. When the budget refuses, the run stops and the rest wait for the next tick.
- **Cost:** only the offers call is made, never fees. SP-API calls are currently free but rate-limited (`getItemOffers` defaults to one call every 2 s in [config.rs](../../crates/amazon/src/config.rs)). The Bright Data fallback is paid per call.

### Analytics views and read-only role

- The `analytics` schema holds only views:
  - `amazon_prices`: Amazon price observations with the product's title and categories.
  - `opportunities`: `search_results` with the search's finish time, the listing's current match status (rule 9) and the product's categories and ranks. This stays the **search-time snapshot**: what a user was shown.
  - `current_opportunities`: one row per `accepted` match. It prices the newest `in_stock`/`unknown` store observation against the newest Amazon observation with a sell price, each with its time.
    - Next to those prices it shows the listing's newest `availability`, Amazon's newest `offer_count` and check time, `membership_required` (from `retailer_listings`; memberships: [ADR 0012](0012-session-auth-csrf-rate-limits-i18n.md)) and `actionable` (rule 6), so out-of-stock, offer-less and Costco/Sam's-only deals can be filtered out.
    - It uses the global `accepted` status in `offer_matches` and deliberately ignores per-tenant `match_decisions` ([ADR 0013](0013-assistant-sidecar-interface.md)), because BI is aggregate across users. Decisions never write `offer_matches`; they only overlay the deciding tenant's own reads ([db/src/assistant.rs](../../crates/db/src/assistant.rs) L6–7, [app/src/assistant.rs](../../crates/app/src/assistant.rs)).
    - **Fees are an approximation.** Re-pricing never fetches fees, so fees are the sell price × the fee ratio of a search result for the same ASIN (rules 1–2).
    - That ratio fits percentage fees (referral) but not fixed per-unit fees (FBA fulfilment), so the error grows as the price moves away from the searched price.
    - Prep and shipping come from that same row (`amazon_price − fees − price − profit`, per [roi.rs](../../crates/domain/src/roi.rs) L89–90) until `roi_assumptions` exists.
  - `listing_prices`: `price_observations` with the listing's title, GTIN and membership flag, plus its accepted ASIN.

#### View rules

Checked in a SQL spike on the local demo data plus synthetic 30-day history.

1. **Fee ratio:** `fees_mxn::numeric / NULLIF(amazon_price_mxn, 0)`, never integer division (both are BIGINT, so a plain `/` gives 0).
2. **Fee row:** the listing's newest `search_results` row whose `asin` equals the current match's ASIN and that has `amazon_price_mxn`, `fees_mxn` and `profit_centavos`. Without one, fees, profit, ROI and margin are NULL; nothing is guessed.
3. **Latest:** newest `observed_at`, ties broken by highest `id`. For `search_results`: newest `searches.created_at`, then highest `search_id`.
4. **One Amazon row per search and ASIN:** the search writer fetches each ASIN once and inserts with `ON CONFLICT (search_id, asin) DO NOTHING`, so a retried search keeps its first fetch. The backfill uses `DISTINCT ON (search_id, asin)`.
5. **No NULL overwrites:** the `amazon_products` upsert keeps a known title, brand, category or rank when a lookup lacks it (`COALESCE`, as in `retailer_listings`); a category and its rank change together.
6. **Actionable:** true only when the listing's newest observation is `in_stock`/`unknown` and Amazon's newest observation has a sell price (it has offers). Missing data counts as false.
7. **Aggregates:** question 4 takes lows and volatility from `in_stock`/`unknown` observations only. Question 2 counts each ASIN once, through its best listing: `actionable`, `membership_required IS NOT TRUE` and the highest known profit (ties by retailer, then SKU).
8. **Memberships:** `membership_required` is NULL when unknown; filters use `membership_required IS NOT TRUE`, never `NOT membership_required` (which drops unknown rows).
9. **Snapshot status:** `opportunities` joins `offer_matches` on `(retailer, retailer_sku, asin)`, so the status is NULL when the listing is now matched to another ASIN.
10. **Money:** only `*_centavos` (BIGINT) and `*_mxn` (`NUMERIC(14,2)`, pesos); ratios as `*_pct`. A base column named `*_mxn` that holds centavos is renamed `*_centavos`, never passed through.

#### Read-only role

- No personal data: no e-mail, password hash, sessions, chat ids or assistant reasons. `tenant_id` stays out until the open question below is settled.
- Role `mex_analytics` (`NOLOGIN`):
  - `USAGE` and `SELECT` on `analytics` only, plus default privileges so future views are covered.
  - `default_transaction_read_only = on` and a `statement_timeout`.
  - Postgres has no `CREATE ROLE IF NOT EXISTS`, so the migration wraps the creation in a `DO` block that checks `pg_roles`.
  - If the migrating user lacks `CREATEROLE` (common on managed Postgres), creating the role becomes an operator runbook step instead.
- Login users are created outside migrations and granted `mex_analytics`.

### Rollout

One PR per step. Each PR includes its migration, `db` functions with Postgres tests, and the Owned tables section of [db.md](../modules/db.md). The PRs that change the `amazon` or `worker` crates also update [amazon.md](../modules/amazon.md) and [worker.md](../modules/worker.md).

1. `retailer_listings` and `amazon_products`: writers, backfill, and the `SalesRank` kind change in `app` and `amazon`.
2. `amazon_price_observations`: the search-job writer and backfill.
   - 2b. Daily re-pricing in the scheduled refresh, with its three config values and its own budget.
3. The `analytics` views and the `mex_analytics` role (or its runbook).

## Later

- `amazon_fee_estimates` and `amazon_fee_estimate_lines`: every SP-API fee estimate with its input price and per-fee lines (today only the total survives, in `search_results`).
- `roi_assumptions`: versioned prep, shipping and fixed costs (today env values that are never stored).
- Lineage on `search_results`: the ids of the price, Amazon price, fee and assumption rows a result was computed from.
- `offer_match_events` (match history) **needs a change to the match writer.** It cannot just wrap `MatchRepo::upsert`, because that is never called for a `no_candidate` outcome: `match_and_store` returns before any write when matching finds nothing ([offer_matching.rs](../../crates/worker/src/offer_matching.rs) L118–120). It is also never called for `rejected`: `match_offer` "never returns `MatchStatus::Rejected`" and yields only `accepted` or `needs_review` ([score.rs](../../crates/matching/src/score.rs) L103, L128–150).
- Foreign keys from `price_observations`, `offer_matches`, `tracked_products` and `search_results` to `retailer_listings` (`NOT VALID`, then `VALIDATE`).
- Sales-rank history (this ADR keeps only the latest ranks).
- **Buy-box ownership** (does Amazon itself hold the buy box?). Until this lands, opportunity rankings overstate products where Amazon sells: a third-party seller rarely wins that buy box, so the sell price may be out of reach.
  - `OfferSummary` has no seller data ([ports.rs](../../crates/app/src/ports.rs)).
  - SP-API `getItemOffers` returns per-offer `SellerId`, `IsBuyBoxWinner` and `IsFulfilledByAmazon` (see the [item_offers.json](../../crates/amazon/tests/fixtures/item_offers.json) fixture), but [parse.rs](../../crates/amazon/src/parse.rs) reads only `Summary`. "Amazon holds it" means the winner's `SellerId` is Amazon's own Mexican seller id, to be confirmed.
  - Bright Data documents `buybox_seller` and `seller_id` fields, not yet seen on a live MX record; the mapper reads only `final_price` and `number_of_sellers`.

## Consequences

- **Volume** stays small: one listing upsert per recorded store offer, at most 27 Amazon price rows per search (9 searchable stores × `SEARCH_MAX_OFFERS_PER_RETAILER`, default 3) and at most `AMAZON_REPRICE_REQUESTS_PER_DAY` from re-pricing. The append-only tables will need a retention ADR, as `price_observations` already does.
- The scheduled refresh now spends Amazon budget. If SP-API is unavailable, the fallback makes it a paid Bright Data cost, bounded by the re-pricing cap.
- Recording a store price becomes one transaction (listing + observation), so a failed listing upsert also fails that write.
- Backfilled rows are marked `source = 'backfill'`. They have no brand, GTIN or category, and their times are approximate.
- Opportunity statistics reflect what users searched for, not the market: they are biased toward popular products, and categories nobody searches stay empty. Re-pricing only covers ASINs that were searched, so the same bias applies to question 1.
- `amazon_price_observations` could later feed an Amazon price-history chart in the app, next to the existing store price history. That would be a separate API change.
- No API change in this ADR, so no `gen:api` run.

## Open questions

- Re-pricing defaults: is a cap of 500 requests a day (`AMAZON_REPRICE_REQUESTS_PER_DAY`) and an interval of 24 hours (`AMAZON_REPRICE_INTERVAL_HOURS`) right?
- Question 4 gets better with more data. Should we record prices for every parsed offer, not just the first 3 per store?
- `search_id` on an append-only table blocks deleting a user's searches, which a request under LFPDPPP (Mexico's data-protection law) may require. Keep it or drop it?
- May BI see `tenant_id`?
