# AGENTS.md

mex-scrape: Spanish-first web app for Mexican Amazon retail-arbitrage sellers. Monorepo with a Rust backend (Cargo workspace in `crates/`), a Next.js web app (`web/`), Postgres, and a standalone Python helper (`fathom_transcript.py`, extracts transcripts from public Fathom share links, managed with [uv](https://docs.astral.sh/uv/)).

Layout, module map and allowed crate dependencies: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md). Per-crate docs: `docs/modules/`. Decisions: `docs/decisions/`.

## Commands

### Rust (toolchain pinned in `rust-toolchain.toml`)

- Format check: `cargo fmt --all --check` (fix with `cargo fmt --all`)
- Lint: `cargo clippy --workspace --all-targets --locked -- -D warnings`
- Test: `cargo test --workspace --locked`
- Run API: `cargo run -p api` (listens on `API_ADDR`, default `127.0.0.1:8080`; `GET /health`, spec at `/api-docs/openapi.json`)
- Run worker: `cargo run -p worker`

### Database

- Env vars: nothing auto-loads `.env.example` or a root `.env`; the binaries read only the process environment. Export the defaults first with `set -a && . ./.env.example && set +a` (bash), or pass vars inline as below.
- Start Postgres: `docker compose up -d --wait db` (from `compose.yaml`; defaults in `.env.example`)
- Apply migrations: `DATABASE_URL=postgres://mex:mex@127.0.0.1:5432/mex_scrape cargo run -p db --bin migrate`
- New migration: add `crates/db/migrations/NNNN_name.sql`; update `docs/modules/db.md` (Owned tables).

### Demo data (local only)

Fake but realistic data on every screen, through the real worker → db → API path ([ADR 0016](docs/decisions/0016-demo-data-mode.md); details and full commands in [docs/modules/db/demo-data.md](docs/modules/db/demo-data.md)). Use a separate local database (e.g. `mex_scrape_demo`) and run `migrate` on it first.

1. `cargo run -p db --bin seed_demo`: the demo login `demo@example.com` / `demo-password-123` (Costco membership, no Sam's Club, so Sam's results show as hidden), a seasonal year of prices for the 25 products in `config/demo_catalog.json`, and one queued search per keyword group (`cocina`, `electronica`, `hogar`, `herramientas`, `despensa`). It needs the env above, refuses a non-local `DATABASE_URL`, and is safe to rerun (no duplicate searches).
2. Start the worker with `DEMO_DATA=true cargo run -p worker --bin worker`: a fake Bright Data connector serves the catalog and runs the queued searches, with no network calls and no change to the real connector flag.
3. Sign in as the demo account: product pages (e.g. `/es/products/B0DEMO0004`) and the assistant already have data, with no manual search; `/es/search?q=electronica` opens a group.
4. Assistant: start the api with `AGENT_URL`, `AGENT_SERVICE_TOKEN` and `AGENT_GRANT_KEY`, and the sidecar with `AGENT_PROVIDER=palsu AGENT_ALLOW_FAKE_PROVIDER=true` (scripted fake model, no provider key).

The demo worker runs only the demo account's searches, so it can stay up during `npm --prefix web run e2e` (e2e searches stay for `seed_e2e complete`).

### Web (`web/`)

- Install: `npm --prefix web ci`
- Dev server: `npm --prefix web run dev` (proxies `/backend/*` to `API_URL`, default `http://127.0.0.1:8080`; override it in `web/.env.local` or the shell — Next.js does not read the repo-root `.env`)
- Lint: `npm --prefix web run lint`
- Typecheck: `npm --prefix web run typecheck`
- Build: `npm --prefix web run build` (bakes `API_URL` into the `/backend/*` rewrite; `next start` ignores a runtime `API_URL`, so set it at build time and rebuild to change it)
- Regenerate the API client: `npm --prefix web run gen:api` (writes `web/openapi.json` and `web/src/lib/api/schema.ts`; commit both — CI fails on drift)

### Agent sidecar (`services/agent/`, Node 22)

- Install: `npm --prefix services/agent ci`
- Typecheck: `npm --prefix services/agent run typecheck`
- Test: `npm --prefix services/agent test` (fake model provider and a fake api; no network)
- Run: `npm --prefix services/agent start` (needs `AGENT_SERVICE_TOKEN`, `AGENT_API_URL`, `AGENT_PROVIDER`, `AGENT_MODEL`, `AGENT_PROVIDER_API_KEY`; see `.env.example` and [docs/modules/agent.md](docs/modules/agent.md))
- Regenerate its api types after `gen:api`: `npm --prefix services/agent run gen:api` (commit `src/api/schema.ts`; CI fails on drift)

### Python (`fathom_transcript.py`, `scripts/`)

- Install: `uv sync --locked`
- Run: `uv run python fathom_transcript.py <share_url> [out_file]`
- Lint: `uvx ruff@0.15.20 check .`
- Format check: `uvx ruff@0.15.20 format --check .` (fix with `uvx ruff@0.15.20 format .`)
- Typecheck: `uvx pyright@1.1.414` (strict mode)

### Docs check

- `uv run python scripts/check_docs.py` — fails if a crate lacks `docs/modules/<crate>.md`, is missing from `docs/ARCHITECTURE.md`, depends on a crate its allowed-dependencies row does not list, depends directly on an HTTP or WebSocket client crate (`reqwest`, `ureq`, `isahc`, `attohttpc`, `surf`, `tungstenite`, `tokio-tungstenite`) unless it is `http-guard`, or enables the `http-guard` feature `test-support` outside `[dev-dependencies]` (including via `[workspace.dependencies]`); also fails if `domain` has any workspace dependency, or if `app` depends on an infrastructure or entry-point crate or directly on `sqlx`, `axum`, `hyper`, `tower`, `utoipa` or an HTTP client crate (ADR 0009).
- Docs check tests: `uv run python -m unittest discover -s scripts -p "test_*.py"`

## Rules

- CI lives in `.github/workflows/ci.yml` and must stay green.
- Never commit with `--no-verify`.
- Never commit transcript output (`transcript.txt`): it may contain private meeting data.
- New crate ⇒ add `docs/modules/<crate>.md`, a module-map row and an allowed-dependencies row in `docs/ARCHITECTURE.md`.
- Changing a dependency direction ⇒ update the allowed-dependencies table; changing a decision ⇒ add a new ADR in `docs/decisions/`.
- `domain` stays pure: no I/O and no workspace dependencies. Adapters never depend on `api`, `worker` or `db`.
- Changing the API ⇒ run `npm --prefix web run gen:api` and commit the generated files.
- Never commit secrets; `.env` files are ignored (only `.env.example` is tracked).
