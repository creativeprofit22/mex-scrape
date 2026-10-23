# 0016. Demo data: a fixture-backed Bright Data connector in the worker

- Status: Accepted
- Date: 2026-10-23 (amended 2026-10-23: keyword groups, queued searches, memberships, seasonal history, demo offers cap, assistant)
- Extends: [ADR 0011](0011-pgmq-jobs-schedules-caps.md) (jobs and caps), [ADR 0010](0010-amazon-spapi-lwa-brightdata-dataset-fallback.md) (Amazon fallback)

## Context

Without Bright Data and SP-API credentials, every local search is refused, so the search results, detail panel, price chart, product page and calculator have nothing to show. The e2e seed (`seed_e2e complete`) stands in for the worker with fixed rows, which skips fetching, parsing, matching and pricing, and its fixtures are tied to the e2e tests. A demo must exercise the real path and must not change the e2e fixtures.

## Decision

- **A fake connector, not a fake worker.** `DEMO_DATA=true` (off by default; exactly `true` or `false`) makes the worker build `worker::demo::DemoTransport`, an implementation of `brightdata::Transport`, instead of the Bright Data client. Search, refresh and category jobs then run unchanged: `Runner` runs and gates, the store adapters' parsers, `match_and_store`, ROI, `search_results`, `price_observations` and tracking.
- **One fixture.** `config/demo_catalog.json` (embedded; parsed and validated by `db::demo::catalog`) lists 25 products, their store listings (some sold out) and their Amazon.com.mx products, each with the match outcome it must produce (`accepted`, `needs_review`, `no_match`), plus the keyword groups (`cocina`, `electronica`, `hogar`, `herramientas`, `despensa`). Every group shows a profitable and a losing accepted match and a needs-review or no-match result. The fake serves listings as JSON-LD store pages (`InStock` or `OutOfStock`) and Amazon products as Datasets v3 records; tests run every listing and every group search through the real adapters, matcher and ROI maths.
- **Fees.** The Bright Data fallback cannot estimate Amazon fees (`Unsupported`), and without fees no ROI is computed. In demo mode `worker::demo::DemoFees` takes SP-API's slot for fee estimates only (15 % referral plus the catalog's FBA fee); its other lookups are `NotConfigured`, so they fall through to the faked Bright Data fallback.
- **Pre-filled through the real path.** The seed queues the group searches through `app::StartSearch` and `db::searches::create_search`, the same path as `POST /searches`, rather than writing results. The demo worker runs them, so search results, matches, tracked products and product histories exist before the first sign-in. A group with a live search (any status but `failed` or `refused`) is not queued again.
- **The assistant uses the scripted fake model.** No provider key is needed for a demo: the sidecar runs with `AGENT_PROVIDER=palsu AGENT_ALLOW_FAKE_PROVIDER=true` and reads the demo data through the api's tools.
- **No network, local only.** Demo mode ignores Bright Data and SP-API credentials, skips the robots.txt pre-check, and refuses a `DATABASE_URL` that is not on a loopback host (`db::local_guard::require_local`), because the fake prices land in the append-only `price_observations`.
- **Gates stay, except the connector flag.** The kill switch, caps, one-active-run rule and Amazon request budget still apply. The connector flag guards paid calls, and the demo transport makes none, so the demo worker's run store treats the connector as enabled (`PgRunStore::with_demo_connector`); `safety_settings.brightdata_enabled` is never turned on for a demo. The default Bright Data caps rise (`DEMO_CAPS`), since a demo search takes one page run plus one or two dataset runs per offer and nothing is paid; set caps still win. For the same reason the default offers per retailer rise to 10 (`DEMO_SEARCH_MAX_OFFERS`), so a group search covers every listing; a set value still wins.
- **A separate seed and account.** `seed_demo` (idempotent, local only, no opt-in) only orchestrates `db::demo`: a year of seasonal history per listing (deterministic noise, Hot Sale and Buen Fin drops, December prices, out-of-stock streaks), the demo account `demo@example.com` with a Costco membership but no Sam's Club (so membership-hidden results show), and one queued search per keyword group. It shares the local-host check with `seed_e2e` (`db::local_guard`) but none of its fixtures. The demo worker runs only that account's searches and drops other tenants' search messages without touching their rows, so the e2e account's searches stay for `seed_e2e complete`.

## Consequences

- Demo searches look real end to end, and a change that breaks parsing, matching or pricing also breaks the demo tests.
- A demo never enables the real connector, so switching back to real credentials needs a deliberate `brightdata_enabled` change.
- A demo worker can stay up during the web e2e suite. Searches by any account other than the demo one are dropped from the queue while it runs (their rows stay `queued`); run a normal worker for those.
- Store links in demo results point at the real stores with made-up SKUs.
- `price_observations` is append-only, so a database seeded with an older history model keeps it; a fresh history needs a fresh local database (`docs/modules/db/demo-data.md`).
- A demo group search makes about 30 Amazon (fake) requests, which counts against the per-tenant request budget like a real one.
