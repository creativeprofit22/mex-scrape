# Architecture

mex-scrape is a Spanish-first web app for Mexican Amazon retail-arbitrage sellers: search a product, compare prices across Mexican retailers against Amazon.com.mx, and see ROI and price history.

It is a monorepo:

- **Rust backend** — a Cargo workspace in `crates/` (decision: [ADR 0001](decisions/0001-rust-backend-nextjs-web.md)).
- **Web app** — Next.js in `web/`, talking to the backend only through the OpenAPI contract.
- **Postgres** — schema owned by the `db` crate; local instance from `compose.yaml`.
- **`fathom_transcript.py`** — standalone Python helper, unrelated to the app.

```
web (Next.js) ──HTTP/OpenAPI──▶ api ─┐
                                     ├─▶ db ──▶ Postgres
                    worker ──────────┘
api, worker ─▶ matching ─▶ domain
worker ─▶ retailers ─▶ brightdata ─▶ http-guard ─▶ internet
worker ──────────────▶ brightdata   (run store + recovery; Amazon.com.mx fallback, ADR 0003)
worker ─▶ amazon ─────────────────▶ http-guard ─▶ internet
worker ─▶ http-guard                (Secret, Redactor: env secrets loaded at start-up)
http-guard, db ─▶ secrets           (Secret, Redactor; db holds DATABASE_URL as a Secret)
```

## Module map

Each module has a page in [`docs/modules/`](modules/) describing its purpose, public API, dependencies and the tables it owns.

| module | kind | purpose | doc |
|---|---|---|---|
| `api` | bin + lib | axum HTTP server; publishes the OpenAPI spec (utoipa) | [api.md](modules/api.md) |
| `worker` | bin | background job runner (fetching, matching, price history) | [worker.md](modules/worker.md) |
| `domain` | lib | pure types and business logic, no I/O | [domain.md](modules/domain.md) |
| `db` | lib + bin `migrate` | sqlx Postgres access and migrations | [db.md](modules/db.md) |
| `http-guard` | lib | hardened outbound HTTP and WebSocket ([ADR 0007](decisions/0007-brightdata-browser-api.md)) | [http-guard.md](modules/http-guard.md) |
| `secrets` | lib | secret values and error redaction, no network code | [secrets.md](modules/secrets.md) |
| `brightdata` | lib | product-agnostic Bright Data client (Web Unlocker + Datasets v3 + Browser API) with durable, capped runs ([ADR 0002](decisions/0002-brightdata-web-unlocker-fetch-layer.md), [ADR 0006](decisions/0006-brightdata-agnostic-client-durable-runs.md), [ADR 0007](decisions/0007-brightdata-browser-api.md), [ADR 0008](decisions/0008-browser-api-settle-blocking-category-listings.md)) | [brightdata.md](modules/brightdata.md) |
| `retailers` | lib | Mexican store adapters: one `Retailer` per store parsing pages into `domain::Offer`, fixture sanitizer, parse-health alerts | [retailers.md](modules/retailers.md) |
| `amazon` | lib | Amazon SP-API client ([ADR 0003](decisions/0003-amazon-sp-api-primary-brightdata-fallback.md)) | [amazon.md](modules/amazon.md) |
| `matching` | lib | product matching and ROI | [matching.md](modules/matching.md) |
| `web` | Next.js app | Spanish UI; generated API client | [web.md](modules/web.md) |

## Allowed dependencies

Only the workspace dependencies listed here are allowed. `scripts/check_docs.py` (run in CI) fails if a crate's `Cargo.toml` depends on a workspace crate not in its row, or if a workspace crate is missing from this table, the module map, or `docs/modules/`.

| crate | may depend on |
|---|---|
| domain | — |
| matching | domain |
| secrets | — |
| http-guard | secrets |
| brightdata | http-guard |
| amazon | domain, http-guard |
| retailers | domain, brightdata |
| db | domain, secrets |
| api | domain, db, matching, retailers |
| worker | domain, db, matching, retailers, amazon, brightdata, http-guard |

Rules:

- `domain` depends on no other workspace crate and does no I/O.
- `brightdata` depends on no mex-scrape domain crate, so other projects can adopt it ([ADR 0006](decisions/0006-brightdata-agnostic-client-durable-runs.md)); its run tables live in `db` and `worker` bridges them.
- Adapters (`http-guard`, `brightdata`, `retailers`, `amazon`) never depend on `api`, `worker` or `db`.
- Only `api` and `worker` are entry points; nothing depends on them.
- All outbound network calls go through `http-guard`. No crate other than `http-guard` may depend on `reqwest`, `tungstenite`/`tokio-tungstenite` (or any other HTTP or WebSocket client crate) or build its own client; every outbound call uses `http_guard::GuardedClient` or, for the Bright Data Browser API only, `http_guard::GuardedWebSocket` ([policy](modules/http-guard.md), [ADR 0007](decisions/0007-brightdata-browser-api.md)). `scripts/check_docs.py` enforces this.
- Secrets (Bright Data key and zone names, SP-API credentials, model provider keys, and the `DATABASE_URL` with its password) are `secrets::Secret` values (re-exported as `http_guard::Secret`), loaded from the environment at start-up. They are never logged, serialized into API responses or stored in DB rows, and every error message passes through a `secrets::Redactor` before it is logged or returned (`db::DatabaseUrl::redactor` for database errors). `api` holds no provider secrets.
- `web` reaches the backend only over HTTP via the client generated from the `api` OpenAPI spec (`npm --prefix web run gen:api`).

## Keeping docs current

See [ADR 0005](decisions/0005-docs-modular-and-current.md). Adding a crate means adding its `docs/modules/<crate>.md`, a module-map row and an allowed-dependencies row; changing a decision means a new ADR.
