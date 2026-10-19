# db

## Purpose

Postgres access through sqlx, plus the schema migrations in `crates/db/migrations/`. Queries use runtime-checked `sqlx::query` so builds never need a live database.

## Public API

- `MIGRATOR`: embedded migrations.
- `DatabaseUrl`: the connection URL held as a `Secret` (no `Debug`/`Display`/`Serialize`). `from_env()`/`from_lookup()` read `DATABASE_URL` (`DATABASE_URL_ENV`): absent is `None`; empty, non-UTF-8 or malformed (not a `postgres://`/`postgresql://` URL that sqlx can parse) is an error at load time naming the variable, never the value. `new(Secret)` applies the same URL check. `redactor()` removes the URL and its password (raw and percent-decoded, userinfo or `password=` query) from error text; apply it before logging any database error.
- `connect(&DatabaseUrl)`: opens a connection pool.
- `migrate(&pool)`: applies pending migrations.
- `PgPool`: re-exported pool type.
- `brightdata_runs`: queries for the Bright Data run tables, with plain row types (`RunRow`, `NewRunRow`, `RunPatch`, `SafetySettings`, `GateCounts`, `RunCaps`) so `db` needs no adapter types. Timestamps cross as epoch milliseconds; JSON as text.
  - `safety_settings` / `set_safety_settings`: the single settings row (`None` when missing; callers fail closed).
  - `gate_counts`: the tenant's runs and items since a day start, and whether the scope has an active run.
  - `insert_run_capped`: one transaction that takes `pg_advisory_xact_lock(hashtext(tenant_id))`, re-checks runs/day, items/run and items/day, then inserts; the partial unique index maps to `InsertRunError::ActiveRun`.
  - `update_if_active`: changes a run only while it is `starting`/`running`/`ready`, so a terminal status is never overwritten.
  - `load_run`, `active_runs`.
  - `insert_access_event` / `access_counts_by_host`: per-URL access outcomes (`ok`, `blocked_needs_kyc`, `skipped_robots`, `failed`) and their per-host totals.
  - `try_driver_lock`: session advisory lock on a dedicated connection (`DriverLock`); the worker holding it drives runs and runs start-up recovery.
- `access::PgAccessCounts(PgPool)`: implements `app::AccessCountsRepo` over `access_counts_by_host`; errors become `app::RepoError::Unavailable` with no detail and no logging (callers log a fixed message).
- `price_history`: matches and price observations, with plain row types (`NewObservation`, `ObservationRow`, `MatchRow`). Timestamps cross as epoch milliseconds; money as centavos.
  - `insert_observation`, `observations_since(retailer, sku, since_ms)` (sorted by `observed_at`, then `id`).
  - `upsert_match`: inserts or replaces the listing's match, except that a stored `rejected` match is never overwritten (a reviewer's decision wins); returns the rows written (1, or 0 when a rejection was kept), which `PgMatches` maps to `app::UpsertOutcome::{Written, KeptRejected}`. `match_status` reads one status.
  - `accepted_listings(asin)`: the `(retailer, retailer_sku)` pairs with an `accepted` match to that ASIN, sorted by retailer then SKU.
  - Adapters `PgPriceHistory(PgPool)` (implements `app::PriceHistoryRepo`) and `PgMatches(PgPool)` (implements `app::MatchRepo`). Errors become `app::RepoError::Unavailable` with no detail; stored text that fails domain validation is also `Unavailable`.
- Binary `migrate`: applies migrations to `DATABASE_URL` (`cargo run -p db --bin migrate`). Safe to run repeatedly. Errors are logged through the URL's redactor.

## Dependencies

- Workspace: `domain`, `app` (port traits it implements), `secrets` (`Secret`, `Redactor` for `DATABASE_URL`).
- External: `sqlx` (Postgres, rustls), `thiserror`, `tokio`, `tracing`, `anyhow`.
- Tests: `crates/db/tests/brightdata_runs.rs` and `crates/db/tests/price_history.rs` need Postgres, so they are ignored by default; run `cargo test -p db --test brightdata_runs -- --ignored` and `cargo test -p db --test price_history -- --ignored` with `DATABASE_URL` set (CI runs both in the `migrations` job). The price-history tests cannot delete their rows (append-only), so each uses a unique SKU.

## Owned tables

All tables. Currently:

- `app_meta(key, value)`: baseline key/value metadata (migration `0001_init.sql`).
- `safety_settings(id = 1, global_kill_switch, brightdata_enabled, updated_at)`: one row, seeded with both off, so the Bright Data connector starts disabled (migration `0002_brightdata_runs.sql`).
- `brightdata_runs(id, tenant_id, job, mode, input_json, snapshot_id, status, requested_count, row_count, failed_count, error_message, created_at, updated_at)`: durable Bright Data runs (migration `0002_brightdata_runs.sql`). CHECKs: tenant/job `[A-Za-z0-9_.:-]{1,100}`, mode `unlocker|dataset`, status `starting|running|ready|imported|failed|cancelled|blocked_needs_kyc` (the last added by `0003_blocked_needs_kyc.sql`), snapshot id `[A-Za-z0-9_-]{1,200}`, counts ≥ 0, error ≤ 1000 characters, input ≤ 32 KiB. The partial unique index `brightdata_runs_one_active` allows one active run per (tenant, job); indexes on `(tenant_id, created_at)` and `status`.
- `brightdata_access_events(id, run_id, host, outcome, created_at)`: one row per URL attempt, host only (migration `0003_blocked_needs_kyc.sql`). `run_id` references `brightdata_runs` (cascade); outcome CHECK `ok|blocked_needs_kyc|skipped_robots|failed`; index on `(host, outcome)`.
- `offer_matches(retailer, retailer_sku, asin, method, confidence_bp, status, matched_at)`: one Amazon match per retailer listing, primary key `(retailer, retailer_sku)` (migration `0004_matching_price_history.sql`). CHECKs mirror the domain: retailer `^[a-z0-9_]{1,40}$`, SKU 1–100 characters, ASIN `^[A-Z0-9]{10}$`, method `gtin|title`, confidence 0–10000 bp, status `accepted|needs_review|rejected`. Indexes on `(asin, status)` and a partial index on `matched_at WHERE status = 'needs_review'` (the review queue).
- `price_observations(id, retailer, retailer_sku, price_mxn, list_price_mxn, availability, observed_at)`: every observed retailer price (migration `0004_matching_price_history.sql`). Money is `BIGINT` centavos (`price_mxn ≥ 0`, `list_price_mxn` NULL or above the price); availability `in_stock|out_of_stock|pre_order|unknown`. Index `(retailer, retailer_sku, observed_at)` for per-product time ranges. **Append-only**: the function `price_observations_append_only()` raises on UPDATE/DELETE (row trigger) and TRUNCATE (statement trigger). Retention: kept indefinitely; pruning needs a new migration and an ADR.
- `_sqlx_migrations`: sqlx migration history (managed by sqlx).
