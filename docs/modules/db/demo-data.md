# Demo data

Local demos only ([ADR 0016](../../decisions/0016-demo-data-mode.md)). A new demo user signs in to a workspace that already has searches, matches, product histories and a working assistant. Every result state shows: profit, loss, needs review, no match, out of stock and hidden by membership. The data takes the real worker → db → API path; nothing is written straight into `search_results` or `offer_matches`.

Code: `crates/db/src/demo/` (`catalog`, `history`, `account`, `searches`), orchestrated by the `seed_demo` binary. The worker side is `worker::demo` ([worker.md](../worker.md#demo-mode)).

## Catalog (`config/demo_catalog.json`, `demo::catalog`)

25 Mexican retail products with 61 listings at Walmart MX, Liverpool, Costco MX, Sam's Club MX, Chedraui, Bodega Aurrera and HEB MX. 16 match Amazon by GTIN (`accepted`), 6 only by title (`needs_review`) and 3 have no Amazon product (`no_match`). ASINs are `B0DEMO0001`–`B0DEMO0022`. There is no Coppel adapter, so Coppel is not in the catalog. Store links point at the real stores with made-up SKUs.

`DemoCatalog::embedded()` validates it:

- unique keys and listings; valid GTINs, ASINs and SKUs; positive prices;
- a GTIN exactly for `accepted` products, and an Amazon product unless `no_match`;
- `groups`: unique, trimmed, and each one a keyword of at least one product.

Errors name the product key or the group.

### Keyword groups

`seed_demo` queues one search per group. Profit is after the demo fees (15 % referral plus the FBA fee) with zero prep and shipping.

| Group | Profitable | Losing | Review / no match |
|---|---|---|---|
| `cocina` | Oster, Ninja, Hamilton Beach, Nespresso at Walmart | Nespresso at Liverpool | T-fal, Vasconia (needs review); tortillero, molcajete (no match) |
| `electronica` | Sony, JBL at Costco and Sam's | Samsung TV, Nintendo Switch, JBL at Liverpool | Xiaomi Smart Band (needs review) |
| `hogar` | Black+Decker, Stanley | Roomba | Mytek fan (needs review) |
| `herramientas` | DeWalt | Bosch rotary hammer | Truper screwdrivers (needs review) |
| `despensa` | Nescafé Clásico | Café Legal, Huggies | chile guajillo (no match) |

The worker test `every_group_has_profit_loss_and_review` runs each group through the search job's flow (adapters, matcher, `calculate_roi`) and checks this mix, counting only results the demo account can see.

### Out of stock

The catalog's `availability: "out_of_stock"` marks a listing as sold out on its store page (default `in_stock`). The fake store pages then say `OutOfStock`, and searches store `availability = out_of_stock` ("Agotado" tag):

- Nintendo Switch at Liverpool, Roomba at Liverpool, DeWalt drill at Walmart MX, Huggies at HEB MX.

## Demo account (`demo::account`)

`demo@example.com` / `demo-password-123`: Spanish, `free`, onboarded. Memberships (`DEMO_MEMBERSHIPS`): **Costco yes, Sam's Club no.** So Sam's Club MX results are hidden and counted under "Ocultas por membresía" on `electronica`, `herramientas` and `despensa` (and any keyword search that finds a Sam's listing, e.g. `audifonos`). Costco results show. Each run resets the password, locale, plan and memberships.

## Price history (`demo::history`)

365 daily observations ending yesterday for each listing that has none. Observations are append-only, so a listing that already has history (from an older seed or a search) keeps it. To see a fresh history, seed a new database (see [Running the demo](#running-the-demo)).

`daily_points` is pure and deterministic. Each listing gets its own seed (a hash of retailer and SKU). All rates are relative to today's price:

- **Noise:** a drift of ±3 % between fortnightly anchors, plus ±2 % daily jitter.
- **Hot Sale:** −17 to −22 % for 9 days from the last Monday of May (May 25 – Jun 2 in 2026).
- **Buen Fin:** −20 to −26 % for the 5 days ending on the third Monday of November (Nov 13–17 in 2025).
- **December:** +6 to +10 % from Dec 1 to 24, then −5 % markdowns from Dec 26 to Jan 6.
- **Today:** the last 14 days blend into today's price, so the first live observation doesn't jump.
- **Bounds:** prices are clamped to 70–115 % of today's price, rounded to whole pesos, at least $1.
- **Stock:** 1–3 out-of-stock streaks of 2–5 days, placed by the seed at least 20 days back. Listings sold out today also end with 3 out-of-stock days.

Unit tests fix today at 2026-10-23 and check:

- the bounds, whole pesos and determinism;
- no fixed period;
- the Hot Sale mean is at most 88 % of the late-April/early-May mean;
- the Buen Fin mean is at most 85 % of October's, and December's is at least 102 % of October's;
- the streak counts and lengths;
- the civil-date maths.

## Queued searches (`demo::searches`)

`queue_group_searches` starts one search per catalog group through `app::StartSearch` over `PgSearches`. This is the use case `POST /searches` uses: `searches::create_search` writes the `searches` row and its `search_jobs` message in one transaction. The demo worker then:

- fills `search_results`, `offer_matches`, `amazon_products` (the catalog's Amazon titles and brands, ADR 0017), `price_observations` and `tracked_products`;
- makes the product pages (headed by the Amazon name), the detail panel's history and the assistant's tools return data;

all with no manual search. Until the demo worker has run them, a demo ASIN has no match and no stored name, so its product page shows the not-found state (404 `product_not_found`) rather than an empty history.

Rerun rule: a group is queued again only when the demo account has no search for it in any status other than `failed` or `refused`. A rerun while the earlier searches exist queues nothing. If the kill switch refused them, turn it off and rerun.

The recent-search list in the web app lives in browser storage, so the seed can't fill it. Open a group with `/es/search?q=electronica`.

## Running the demo

Use a separate local database, so the old zigzag history (append-only) and other data stay out of the way. From the repo root, in bash:

```bash
docker compose up -d --wait db
docker compose exec db createdb -U mex mex_scrape_demo   # once
export DATABASE_URL=postgres://mex:mex@127.0.0.1:5432/mex_scrape_demo
cargo run -p db --bin migrate
cargo run -p db --bin seed_demo           # history, account, 5 queued searches
DEMO_DATA=true cargo run -p worker --bin worker   # runs them in seconds; keep it running
```

Then start the api with the assistant settings, the sidecar on the scripted fake model, and the web app. Each needs its own terminal:

```bash
# api (the two keys are any local values of at least 32 bytes)
export DATABASE_URL=postgres://mex:mex@127.0.0.1:5432/mex_scrape_demo
export AGENT_URL=http://localhost:8090
export AGENT_SERVICE_TOKEN=local-demo-service-token-0123456789abcdef
export AGENT_GRANT_KEY=local-demo-grant-key-0123456789abcdef0123
cargo run -p api

# assistant sidecar: the scripted fake model reads real data through the api
npm --prefix services/agent ci
AGENT_SERVICE_TOKEN=local-demo-service-token-0123456789abcdef \
AGENT_API_URL=http://127.0.0.1:8080 \
AGENT_PROVIDER=palsu AGENT_ALLOW_FAKE_PROVIDER=true \
npm --prefix services/agent start

# web
npm --prefix web ci && npm --prefix web run build && npm --prefix web run start
```

Sign in at `http://localhost:3000/es` as `demo@example.com` / `demo-password-123`:

- **Product page:** `/es/products/B0DEMO0004` (Sony headphones). Walmart and Liverpool are visible and Sam's is hidden; the chart shows the Hot Sale and Buen Fin drops.
- **Search:** `/es/search?q=electronica` shows every state.
- **Assistant:** `/es/assistant`. Ask about an ASIN, e.g. "¿Cuál es el precio más bajo de B0DEMO0004?". The fake model calls `get_price_history` and answers with the 365-day low from the seeded history. A real provider works too (`AGENT_PROVIDER=anthropic` with `AGENT_MODEL` and `AGENT_PROVIDER_API_KEY`).

`AGENT_URL` needs a host name (`localhost`, not `127.0.0.1`); with an IP the api logs "assistant disabled" and the assistant answers that it isn't available.

### Recorded run (2026-10-23, fresh `mex_scrape_demo`)

- Seed: 61 listings with history, 5 searches queued; a rerun queued 0. The demo worker finished all 5 `done`.
- `GET /searches/2` (`electronica`): `hidden_results: 4` (the Sam's Club rows) and results with `in_stock` and `out_of_stock`.
- `POST /assistant/chat` with "¿Cuál es el precio más bajo de B0DEMO0004?" replied: "Para preparar el Buen Fin: el precio más bajo de B0DEMO0004 en los últimos 365 días fue $681.00 (walmart_mx). Si lo ves por debajo de ese precio, es buen momento para comprar." That matches `price_observations`: Walmart's low was $681 on 2025-11-15 (Buen Fin). Sam's lower $666 is left out because the demo account has no Sam's membership.

The demo worker runs only the demo account's searches, so it can stay up during `npm --prefix web run e2e`; e2e searches stay queued for `seed_e2e complete`.
