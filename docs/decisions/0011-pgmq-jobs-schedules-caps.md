# 0011. Background jobs: pgmq over sqlx, cron schedules, one driving worker, Amazon request budget

- Status: Accepted
- Date: 2026-10-19

## Context

The worker must run searches, scheduled price refreshes and scheduled category browses (Sears MX and Sanborns, [ADR 0008](0008-browser-api-settle-blocking-category-listings.md)) as durable background jobs with retries, a dead-letter queue, daily caps and a global kill switch. We already run Postgres and want no Redis.

Constraints found while planning:

- pgmq is normally installed as a Postgres extension, which the stock `postgres:17.11-alpine` image (compose and CI) does not ship. pgmq also publishes a SQL-only install (`pgmq-extension/sql/pgmq.sql`) that creates the `pgmq` schema with plain SQL and PL/pgSQL.
- The `pgmq` Rust crate's stable release (0.33.x) needs sqlx 0.8; the workspace uses sqlx 0.9, and pgmq 0.34 is still alpha.
- `tokio-cron-scheduler` reads the wall clock internally, which breaks the style rule that tests use injected clocks.
- Bright Data runs are driven by one process holding the driver lock (ADR 0006); a second worker starting runs would race the lock holder's start-up recovery.
- The Bright Data gates (kill switch → enabled → key → caps → one active run) already protect Bright Data calls, but SP-API calls had no kill switch and no per-tenant budget.

## Decision

- **pgmq, SQL-only, via our sqlx.** Migration `0005_pgmq.sql` vendors `pgmq-extension/sql/pgmq.sql` from tag `v1.10.0` verbatim (PostgreSQL license, header records source and tag). `db::queue` calls `pgmq.send`, `pgmq.read`, `pgmq.delete`, `pgmq.set_vt` (the same calls as the `pgmq-rs` example) through our sqlx pool. Queue names come from a closed `db::queue::Queue` enum. Upgrading pgmq means a new migration following pgmq's `UPDATING.md`.
- **Queues.** `search_jobs` and `refresh_jobs` (the latter carries `product_refresh` and `category_browse` messages), each with a `_dlq` twin. A message read is invisible for `JOB_VISIBILITY_SECS` (default 900 s, above the 600 s dataset poll budget). Transient failures are retried with exponential backoff via `set_vt`; after `JOB_MAX_ATTEMPTS` reads the message moves to the DLQ (send + delete in one transaction). Refusals (kill switch, connector disabled, missing key, daily cap) finish the job as `refused` and delete the message, so a refusal never becomes a retry storm.
- **Schedules: `croner` + our own loop.** Cron expressions (`REFRESH_CRON`, `CATEGORY_BROWSE_CRON`, UTC) are parsed with `croner`; a small tokio loop with an injected clock sleeps until the next tick and enqueues messages. Tests drive the clock.
- **One driving worker.** Only the worker holding the Bright Data driver lock runs schedules and consumers, one message at a time. A worker without the lock retries the lock every 60 s; on success it runs recovery first. Ticks therefore never double-fire and jobs never clash on the one-active-run gate.
- **Tenants.** Every job message and job row carries `tenant_id`. Until accounts exist, searches started through the API use tenant `default` (admin bearer only) and scheduled jobs use tenant `system`.
- **Amazon request budget.** A new port `app::ports::RequestBudget` and decorator `app::GatedAmazonSource` consume one unit before every Amazon call (SP-API or the Bright Data fallback). `db::usage::PgRequestBudget` checks the global kill switch (missing settings ⇒ on) and the per-tenant daily cap `AMAZON_MAX_REQUESTS_PER_TENANT_PER_DAY` atomically (conditional upsert on `provider_usage`). A refusal is `AmazonSourceError::Refused`, which never falls back.

## Consequences

- About 2 000 lines of third-party SQL live in our migrations; they are pinned and never edited in place.
- Throughput is bounded by the Bright Data caps (default 20 runs per tenant per day). Jobs batch their URLs into few runs; operators raise caps with env variables (runbook in [`docs/modules/worker.md`](../modules/worker.md)).
- Adding a second worker gives fail-over, not parallelism, until per-scope locking is designed.
- The search API is admin-only until accounts exist.
