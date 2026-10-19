# 0006. Product-agnostic Bright Data client with durable, capped runs

- Status: Accepted
- Date: 2026-10-19
- Extends: [0002](0002-brightdata-web-unlocker-fetch-layer.md), [0003](0003-amazon-sp-api-primary-brightdata-fallback.md)

## Context

Bright Data has no pre-built scrapers for .mx retailers, so store pages come from Web Unlocker and we parse them. The Amazon.com.mx fallback needs Datasets v3, which is asynchronous (trigger, poll, download). Paid calls must be capped, stoppable and must survive a worker restart. linkgo already has a tested blocking connector for Datasets v3 with run tracking, gates and recovery, and could reuse a shared crate.

## Decision

- `brightdata` is product-agnostic: it depends only on `http-guard` (the `domain` dependency is dropped). Results go to a caller-supplied `RunSink`; storage sits behind a `RunStore` trait.
- It covers both Web Unlocker (`POST /request`) and Datasets v3 (trigger, progress, download, cancel), ported from linkgo to async.
- Errors are `Terminal` or `Transient`; transient calls retry with capped exponential backoff (no jitter, so behaviour is deterministic), dataset triggers retry only when the request provably was not accepted, and requests are rate limited per zone.
- Runs are durable in Postgres (`brightdata_runs`, owned by `db`; `worker` implements `RunStore`). One active run per tenant + job (partial unique index). Every start passes fixed-order, fail-closed gates: kill switch, connector enabled, key present, per-tenant daily caps, one active run. The connector starts disabled.
- At start-up one worker (session advisory lock) recovers interrupted runs: dataset runs with a snapshot id stay resumable for 24 h, everything else fails.

## Consequences

- linkgo can adopt the crate without mex-scrape types.
- `db` gains no adapter dependency; the worker maps between row types and the run model.
- Daily caps use UTC days. Multi-worker driving will need run leases later; for now only the lock holder recovers runs.
- Retried Unlocker requests may be billed; retries are capped at 3.
