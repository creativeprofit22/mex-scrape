# Architecture

mex-scrape is a Spanish-first web app for Mexican Amazon retail-arbitrage sellers: search a product, compare prices across Mexican retailers against Amazon.com.mx, and see ROI and price history.

It is a monorepo:

- **Rust backend** — a Cargo workspace in `crates/` (decision: [ADR 0001](decisions/0001-rust-backend-nextjs-web.md)).
- **Web app** — Next.js in `web/`, talking to the backend only through the OpenAPI contract.
- **Postgres** — schema owned by the `db` crate; local instance from `compose.yaml`.
- **Assistant sidecar** — a Node 22 service in `services/agent` running gg-agent headless; its tools only call the `api` ([ADR 0004](decisions/0004-gg-agent-sidecar-for-ai.md), [ADR 0013](decisions/0013-assistant-sidecar-interface.md)).
- **`fathom_transcript.py`** — standalone Python helper, unrelated to the app.

```
web (Next.js) ──HTTP/OpenAPI──▶ api ─┐                 (sessions, CSRF, rate limits; enqueues searches, reads results)
                                     ├─▶ db ──▶ Postgres (tables + pgmq queues, ADR 0011)
                    worker ──────────┘                 (worker consumes queues, runs schedules)
api, worker ─▶ matching ─▶ domain
worker ─▶ retailers ─▶ brightdata ─▶ http-guard ─▶ internet
worker ──────────────▶ brightdata   (run store + recovery)
worker ─▶ amazon ─────────────────▶ http-guard ─▶ internet   (SP-API, LWA tokens; ADR 0010)
          amazon ─▶ brightdata                   (Amazon dataset fallback, ADR 0010)
worker ─▶ http-guard                (Secret, Redactor: env secrets loaded at start-up)
api ─▶ http-guard ─▶ agent (services/agent, Node) ─▶ model provider   (chat proxy, private_service policy; ADR 0013)
       agent ──tools──▶ api /internal/agent/*    (service bearer + per-chat grant; no db, Bright Data or SP-API)
http-guard, db ─▶ secrets           (Secret, Redactor; db holds DATABASE_URL as a Secret)
api, worker, db, retailers, amazon ─▶ app ─▶ domain   (use cases and ports, ADR 0009)
```

## Layers

The backend follows the Onion architecture ([ADR 0009](decisions/0009-onion-architecture.md)). Dependencies point inward only:

```
┌ entry points (composition roots): api, worker ────────────────────────┐
│ ┌ infrastructure (adapters): db, retailers, amazon,                 ┐ │
│ │                            http-guard, brightdata, secrets        │ │
│ │ ┌ app: use cases + ports (OfferSource, AmazonSource,           ┐  │ │
│ │ │      PriceHistoryRepo, MatchRepo, AccessCountsRepo, Clock,   │  │ │
│ │ │      RequestBudget, SearchRepo, UserRepo, PasswordHasher,    │  │ │
│ │ │      MatchDecisionRepo, AgentUsageRepo, AssistantEngine)     │  │ │
│ │ │ ┌ domain: entities and pure rules ┐                          │  │ │
│ │ │ └─────────────────────────────────┘                          │  │ │
│ │ └──────────────────────────────────────────────────────────────┘  │ │
│ └───────────────────────────────────────────────────────────────────┘ │
└───────────────────────────────────────────────────────────────────────┘
```

- **domain**: entities and pure rules.
- **app**: use cases and the port traits they call. Adapters implement them (`db::access::PgAccessCounts`, `db::price_history::PgPriceHistory`, `db::price_history::PgMatches`, `db::searches::PgSearches`, `db::usage::PgRequestBudget`, `db::users::PgUsers`, `db::assistant::{PgMatchDecisions, PgAgentUsage}`, `api::auth::hasher::Argon2Hasher`, `api::SidecarEngine`, `retailers::RetailerOfferSource`, `amazon::SpApiAmazonSource`, `amazon::BrightDataAmazonSource`).
- **infrastructure**: adapters for Postgres, store pages, Amazon, outbound HTTP, Bright Data and secrets.
- **entry points**: `api` and `worker` build adapters, inject them into use cases, and map results to HTTP or logs.
- `brightdata` stays product-agnostic, outside the onion ([ADR 0006](decisions/0006-brightdata-agnostic-client-durable-runs.md)); `worker` bridges it.

## Jobs, queues and schedules

Slow and paid work runs as durable background jobs in the `worker` ([ADR 0011](decisions/0011-pgmq-jobs-schedules-caps.md); details and runbook in [worker.md](modules/worker.md)):

- Queues are pgmq on the same Postgres (SQL vendored as a migration, called through `db::queue`; no Redis): `search_jobs` (searches enqueued by `POST /searches`, atomically with the `searches` row) and `refresh_jobs` (`product_refresh` and `category_browse`, enqueued by the schedules), each with a `_dlq` dead-letter queue. Reads use a visibility timeout; transient failures retry with backoff a bounded number of times, then dead-letter; refusals are never retried.
- Schedules (UTC cron, `croner` with an injected clock): `REFRESH_CRON` refreshes tracked listings into `price_observations`; `CATEGORY_BROWSE_CRON` browses the configured category listings of stores without keyword search (Sears MX, Sanborns; [ADR 0008](decisions/0008-browser-api-settle-blocking-category-listings.md)) through the Browser API with a page cap.
- Only the worker holding the Bright Data driver lock runs consumers and schedules, so ticks never double-fire.
- Spending guards run before every paid call: Bright Data runs pass `brightdata::runs::check_gates` (kill switch, enabled, key, per-tenant daily caps, one active run); Amazon calls (SP-API and fallback) pass `app::GatedAmazonSource` (kill switch, per-tenant daily request cap). Tenants: `user-<id>` for each signed-in user's searches ([ADR 0012](decisions/0012-session-auth-csrf-rate-limits-i18n.md)), `system` for scheduled jobs.
- Each job logs per-retailer metrics (`mex_scrape::metrics`: requests, parse success, `blocked_needs_kyc`, estimated cost).

## Accounts, sessions and the web app

[ADR 0012](decisions/0012-session-auth-csrf-rate-limits-i18n.md); details in [api.md](modules/api.md) and [web.md](modules/web.md):

- Users sign up with e-mail and password (Argon2id). `api` keeps server-side sessions in Postgres (`sessions`); the browser holds only an `HttpOnly`, `SameSite=Lax`, `Secure` session-id cookie. Every non-GET user route needs the session's `X-CSRF-Token` and a same-origin request, else 403.
- Each user is their own tenant (`user-<id>`, a generated column), so searches are isolated at the data layer. Results that need a store membership the user lacks (Costco, Sam's Club) are hidden.
- User routes are rate limited per plan (signed in, by user id) or per client IP (signed out), with `tower_governor`; 429 carries `Retry-After`.
- The web app serves `/es` (default) and `/en`. All UI text comes from two message catalogs that a lint step keeps in sync. Playwright tests with axe run against a database seeded by `db`'s `seed_e2e`.

## AI assistant

[ADR 0013](decisions/0013-assistant-sidecar-interface.md); details in [agent.md](modules/agent.md) and [api.md](modules/api.md):

- `POST /assistant/chat` (session, CSRF, plan rate limit) checks the tenant's daily token budget, mints a 120 s HMAC grant for the chat and calls the sidecar. The sidecar runs gg-agent with `maxTurns`, per-tool timeouts, a run timeout and an `AbortSignal` tied to the client disconnecting, and stops when the budget is spent.
- Its four tools (`search_offers`, `get_price_history`, `compute_roi`, `resolve_match`) call `/internal/agent/*`, which take the tenant only from the grant. "Tracked products" are the listings in the user's own searches of the last 90 days. Nothing starts a paid search.
- Tie-breaks are propose-then-confirm: the agent stores a suggestion for a `needs_review` match, and the user's `POST /matches/decisions` records a per-tenant decision with who decided. Decisions change only that tenant's search results and price stats/series; the global `offer_matches` table is untouched.

## Module map

Each module has a page in [`docs/modules/`](modules/) describing its purpose, public API, dependencies and the tables it owns.

| module | kind | purpose | doc |
|---|---|---|---|
| `api` | bin + lib | axum HTTP server: accounts, cookie sessions, CSRF and per-plan rate limits ([ADR 0012](decisions/0012-session-auth-csrf-rate-limits-i18n.md)); publishes the OpenAPI spec (utoipa) | [api.md](modules/api.md) |
| `worker` | bin + lib | background job runner: pgmq consumers, cron schedules, fetching, matching, ROI, price history ([ADR 0011](decisions/0011-pgmq-jobs-schedules-caps.md)) | [worker.md](modules/worker.md) |
| `domain` | lib | pure types and business logic, no I/O | [domain.md](modules/domain.md) |
| `app` | lib | application layer: use cases and ports, no I/O ([ADR 0009](decisions/0009-onion-architecture.md)) | [app.md](modules/app.md) |
| `db` | lib + bins `migrate`, `seed_e2e` | sqlx Postgres access, migrations, the pgmq job queues, users and sessions | [db.md](modules/db.md) |
| `http-guard` | lib | hardened outbound HTTP and WebSocket ([ADR 0007](decisions/0007-brightdata-browser-api.md)) | [http-guard.md](modules/http-guard.md) |
| `secrets` | lib | secret values and error redaction, no network code | [secrets.md](modules/secrets.md) |
| `brightdata` | lib | product-agnostic Bright Data client (Web Unlocker + Datasets v3 + Browser API) with durable, capped runs ([ADR 0002](decisions/0002-brightdata-web-unlocker-fetch-layer.md), [ADR 0006](decisions/0006-brightdata-agnostic-client-durable-runs.md), [ADR 0007](decisions/0007-brightdata-browser-api.md), [ADR 0008](decisions/0008-browser-api-settle-blocking-category-listings.md)) | [brightdata.md](modules/brightdata.md) |
| `retailers` | lib | Mexican store adapters: one `Retailer` per store parsing pages into `domain::Offer`, fixture sanitizer, parse-health alerts | [retailers.md](modules/retailers.md) |
| `amazon` | lib | Amazon.com.mx data: SP-API client (LWA tokens, rate limits) plus the Bright Data Amazon-dataset fallback, both behind `app::ports::AmazonSource` ([ADR 0003](decisions/0003-amazon-sp-api-primary-brightdata-fallback.md), [ADR 0010](decisions/0010-amazon-spapi-lwa-brightdata-dataset-fallback.md)) | [amazon.md](modules/amazon.md) |
| `matching` | lib | product matching: retailer offer → Amazon ASIN (GTIN first, then brand + title confidence); ROI is in `domain` | [matching.md](modules/matching.md) |
| `agent` | Node 22 service (`services/agent`) | headless gg-agent sidecar: assistant chat, zod tools that only call the `api` ([ADR 0004](decisions/0004-gg-agent-sidecar-for-ai.md), [ADR 0013](decisions/0013-assistant-sidecar-interface.md)) | [agent.md](modules/agent.md) |
| `web` | Next.js app | Spanish-first UI with an English toggle (`/es`, `/en`), Playwright + axe e2e tests; generated API client | [web.md](modules/web.md) |

## Allowed dependencies

Only the workspace dependencies listed here are allowed. `scripts/check_docs.py` (run in CI) fails if a crate's `Cargo.toml` depends on a workspace crate not in its row, or if a workspace crate is missing from this table, the module map, or `docs/modules/`.

| crate | may depend on |
|---|---|
| domain | — |
| app | domain |
| matching | domain |
| secrets | — |
| http-guard | secrets |
| brightdata | http-guard |
| amazon | domain, app, http-guard, brightdata |
| retailers | domain, app, brightdata |
| db | domain, app, secrets |
| api | domain, app, db, http-guard, matching, retailers |
| worker | domain, app, db, matching, retailers, amazon, brightdata, http-guard |

Rules:

- `domain` depends on no other workspace crate and does no I/O.
- `app` depends only on `domain` and non-I/O libraries: no infrastructure or entry-point crate, and no `sqlx`, `axum`, `hyper`, `tower`, `utoipa` or HTTP client crate. Its port signatures use only domain and app types; use cases receive ports as constructor arguments and never construct adapters ([ADR 0009](decisions/0009-onion-architecture.md)). `scripts/check_docs.py` enforces this.
- `brightdata` depends on no mex-scrape domain crate, so other projects can adopt it ([ADR 0006](decisions/0006-brightdata-agnostic-client-durable-runs.md)); its run tables live in `db` and `worker` bridges them.
- Adapters (`http-guard`, `brightdata`, `retailers`, `amazon`) never depend on `api`, `worker` or `db`.
- Only `api` and `worker` are entry points; nothing depends on them.
- All outbound network calls go through `http-guard`. No crate other than `http-guard` may depend on `reqwest`, `tungstenite`/`tokio-tungstenite` (or any other HTTP or WebSocket client crate) or build its own client; every outbound call uses `http_guard::GuardedClient` or, for the Bright Data Browser API only, `http_guard::GuardedWebSocket` ([policy](modules/http-guard.md), [ADR 0007](decisions/0007-brightdata-browser-api.md)). `scripts/check_docs.py` enforces this. Calls are public HTTPS only, with one opt-in exception: `TransportPolicy::private_service(port)` (plain http to an allowlisted host name on one port, still no link-local/metadata addresses), used only by the api's client for the assistant sidecar ([ADR 0013](decisions/0013-assistant-sidecar-interface.md)).
- Secrets (Bright Data key and zone names, SP-API credentials, model provider keys, and the `DATABASE_URL` with its password) are `secrets::Secret` values (re-exported as `http_guard::Secret`), loaded from the environment at start-up. They are never logged, serialized into API responses or stored in DB rows, and every error message passes through a `secrets::Redactor` before it is logged or returned (`db::DatabaseUrl::redactor` for database errors). `api` holds no provider secrets; the model provider key lives only in the assistant sidecar, and the api holds only the sidecar's service token and grant key.
- `web` reaches the backend only over HTTP via the client generated from the `api` OpenAPI spec (`npm --prefix web run gen:api`). The `agent` sidecar likewise reaches only the api's `/internal/agent/*` routes, with types generated from the same spec.

## Keeping docs current

See [ADR 0005](decisions/0005-docs-modular-and-current.md). Adding a crate means adding its `docs/modules/<crate>.md`, a module-map row and an allowed-dependencies row; changing a decision means a new ADR.
