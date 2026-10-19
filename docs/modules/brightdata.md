# brightdata

## Purpose

Async Bright Data client — Web Unlocker (store pages, [ADR 0002](../decisions/0002-brightdata-web-unlocker-fetch-layer.md)) and Datasets v3 (Amazon.com.mx fallback, [ADR 0003](../decisions/0003-amazon-sp-api-primary-brightdata-fallback.md)) — plus durable, capped run tracking. Product-agnostic: no mex-scrape domain types, so linkgo can adopt it ([ADR 0006](../decisions/0006-brightdata-agnostic-client-durable-runs.md)). Ported from linkgo's blocking connector (`direct.rs`, `runs.rs`, `recovery.rs`, `store.rs`), now async over `http-guard`.

## Endpoints

All requests go to the fixed base `https://api.brightdata.com` (the only host on the client's `http-guard` allowlist) with `Authorization: Bearer <BRIGHTDATA_API_KEY>`.

| call | request | result |
|---|---|---|
| `unlock` | `POST /request`, JSON `{"zone", "url", "format": "raw", "country"?}` | `UnlockedPage { status, body }` (body ≤ 8 MiB) |
| `trigger` | `POST /datasets/v3/trigger?dataset_id=…&include_errors=true[&type=discover_new&discover_by=…][&limit_per_input=…]`, JSON array of 1–1000 input objects | `SnapshotId` (validated) |
| `progress` | `GET /datasets/v3/progress/{id}` | `Running` (starting/running/building/collecting/digesting), `Ready`, `Failed`, `Cancelled`; anything else is terminal |
| `download` | `GET /datasets/v3/snapshot/{id}?format=json` | `NotReady` on 202 or 409, else `Records(Vec<Value>)` (must be an array, ≤ 8 MiB) |
| `cancel` | `POST /datasets/v3/snapshot/{id}/cancel` | `()` |

The zone comes from `BRIGHTDATA_UNLOCKER_ZONE` and is treated as a secret. Timeouts: 120 s for Unlocker, 30 s for Datasets; connect 10 s.

Inputs are validated before any I/O: snapshot and dataset ids are 1–200 characters of `[A-Za-z0-9_-]` (a snapshot id returned by Bright Data is validated too), target URLs are absolute `http(s)` without userinfo and at most 2000 characters, country codes are two letters.

## Public API

- `BrightDataConfig::from_secrets(key, zone)` (`ConfigError::MissingApiKey` / `MissingUnlockerZone`), with public `unlocker_timeout`, `datasets_timeout`, `retry`, `rate`.
- `BrightDataClient::new(config)`; implements `Transport` (`unlock`, `trigger`, `progress`, `download`, `cancel`). Tests inject fakes through `Transport`.
- Ids and inputs: `SnapshotId`, `DatasetId`, `DiscoverBy`, `TargetUrl`, `CountryCode`, `UnlockRequest`, `DatasetTrigger`.
- `runs`: `Runner { transport, store, clock, timing, caps }` with `start_unlocker_run`, `start_dataset_run`, `resume_run`, `cancel_run`; traits `RunStore` (storage), `RunSink` (where pages and records go; callers parse them), `Clock`; pure `check_gates`.
- `recovery`: pure `recovery_action` and `recover_interrupted(store, clock)`, which returns `RecoveryReport { failed, kept }` (`kept` lists the run ids left active).

## Error classes

`BrightDataError::Terminal` or `Transient`. Messages never contain the URL; the API key and zone are redacted, and text is cut to 500 characters.

- **Terminal**: 400, 401, 403, 404, 422 and every other 4xx except 408/429; invalid ids or inputs; non-JSON or unexpected response shapes; unknown progress status; body over the cap; requests refused by `http-guard`.
- **Transient**: 408, 429, 5xx, timeouts, connection and transport errors.

The detail is Bright Data's `x-brd-error` header plus the body's `error`/`message` field (or trimmed text). With `format: raw`, a non-2xx Unlocker answer is classified by its HTTP status.

## Retry and rate limits

- Transient failures retry up to 3 times with capped exponential backoff: 1 s, 2 s, 4 s … capped at 30 s, no jitter (deterministic). A numeric `Retry-After` on 429 replaces the backoff (clamped to 30 s). Terminal errors are never retried.
- `trigger` starts a paid collection, so it retries only 429 and failed connections (the request was provably not accepted), never a 5xx or timeout after sending.
- Retried Unlocker requests may be billed by Bright Data; at most 4 attempts per page.
- `ZoneLimiter` spaces requests per zone: 500 ms between Unlocker requests, 1 s between Datasets requests (per client instance; no global state).

## Runs, gates and caps

Every `start_*` checks, in order, failing closed (first failure wins):

1. Global kill switch off (`safety_settings.global_kill_switch`).
2. Connector enabled (`safety_settings.brightdata_enabled`).
3. API key present (`Runner.transport` is `Some`).
4. Per-tenant runs per UTC day < 20.
5. Items in this run ≤ 50.
6. Per-tenant items per UTC day ≤ 500.

Items are billable units: one per Unlocker URL; for a dataset trigger, one per input, or `inputs × limit_per_input` when `limit_per_input` is set (overflow fails gate 5). A discovery trigger (`discover_by`) without `limit_per_input`, or any `limit_per_input` of 0, is rejected as invalid before any row or request, because its record count would be unbounded. The stored `requested_count` uses the same number, so the daily sum matches.
7. No other active run for the scope (tenant + job).

A missing settings row counts as kill switch on / connector disabled. The store re-checks 4–7 atomically when inserting (per-tenant advisory lock plus the partial unique index), so concurrent starts cannot exceed the caps. Daily caps reset at UTC midnight (not Mexico City time). `resume_run` checks gates 1–3 only; `cancel_run` is never gated.

Lifecycle: `starting` → `running` → (`ready`) → `imported` | `failed` | `cancelled`. The first three are active.

- **Unlocker run**: fetches each URL and hands the page to the sink; counts `row_count`/`failed_count`. Ends `imported` if any page was accepted, else `failed` with the first error.
- **Dataset run**: trigger → store snapshot id, `running` → poll progress every 10 s → `ready` → download (202/409 keeps polling) → sink → `imported`. Bright Data `failed` or a terminal error → `failed`; `cancelled` → `cancelled`. A transient error, a sink error or the 10-minute poll budget running out leaves the run active with a message, resumable by snapshot id.
- Before each URL or poll the runner stops if the kill switch is on (run `cancelled`, snapshot cancelled best effort) or the run was finished elsewhere. Storage never overwrites a terminal status, so a cancel sticks.
- Stored error messages are redacted and bounded to 1000 characters; inputs to 20 000 bytes of JSON.

## Recovery

At worker start-up (under the driver lock) every active run is interrupted:

- Web Unlocker run → `failed` ("Interrupted when the worker stopped").
- Dataset run without a snapshot id → `failed`.
- Dataset run with a snapshot id, last updated ≤ 24 h ago → kept active; its id is returned in `RecoveryReport::kept`. The caller must resume it with `resume_run` while still holding the driver lock: an active run blocks new runs for its scope (`brightdata_runs_one_active`).
- Older → `failed` ("not collected within 24 hours").

The worker resumes every kept run right after recovery (see [worker](worker.md)). Until a product importer exists it passes a sink that refuses every record with fixed text, so a downloaded run stays `ready` and resumable rather than failed (the paid snapshot stays collectable); the run keeps its scope's active slot until it is imported or expires at a later start-up. Without `BRIGHTDATA_API_KEY` and `BRIGHTDATA_UNLOCKER_ZONE` the resume fails the key gate and makes no request.

## Permitted use

- Fetch only public product and price pages of Mexican retailers and Amazon.com.mx for the signed-in seller's own price comparison; no personal data, no logged-in pages.
- The connector is off by default. An operator enables it, and can stop everything at once with the kill switch:

  ```sql
  UPDATE safety_settings SET brightdata_enabled = TRUE, updated_at = now() WHERE id = 1;
  UPDATE safety_settings SET global_kill_switch = TRUE, updated_at = now() WHERE id = 1;
  ```

- Cost is bounded by the caps above; each Unlocker page and dataset record is billed by Bright Data. Discovery runs always carry `limit_per_input`, so records per run are capped at `inputs × limit_per_input`. A plain (non-discovery) trigger without a limit is counted as one record per input.

## Live smoke test

`crates/brightdata/tests/live_smoke.rs` (ignored by default, hard cap of 3 requests, retries off): one Unlocker fetch of `https://geo.brdtest.com/mygeo.json` through Mexico, and one `progress` call on an unknown snapshot id (checks Datasets auth without a paid trigger). It prints only statuses, sizes and redacted errors.

```bash
BRIGHTDATA_API_KEY=... BRIGHTDATA_UNLOCKER_ZONE=... \
  cargo test -p brightdata --test live_smoke -- --ignored --nocapture
```

Results: [docs/verification/brightdata-live-smoke.md](../verification/brightdata-live-smoke.md).

## Dependencies

- Workspace: `http-guard` only (transport, `Secret`, `Redactor`).
- External: `serde`, `serde_json`, `thiserror`, `tokio` (`time`, `sync`).
- Dev: `http-guard` with `test-support` (loopback fake servers), `tokio` test utilities.

## Owned tables

None. `brightdata_runs` and `safety_settings` are owned by [`db`](db.md); `worker` implements `RunStore` over them.
