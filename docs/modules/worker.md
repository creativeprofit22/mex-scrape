# worker

## Purpose

Background job runner: fetching retailer and Amazon prices, matching, price history. For now it loads server secrets, logs which ones are set, recovers interrupted Bright Data runs and resumes the kept ones when `DATABASE_URL` is set, and exits.

## Public API

- Binary `worker` (`cargo run -p worker`).
- Environment (all optional for now; each adapter will require its own once it exists):
  - `BRIGHTDATA_API_KEY`, `BRIGHTDATA_UNLOCKER_ZONE`
  - `SPAPI_LWA_CLIENT_ID`, `SPAPI_LWA_CLIENT_SECRET`, `SPAPI_REFRESH_TOKEN`
  - `MODEL_PROVIDER_API_KEY`
  - `DATABASE_URL` (optional for now): enables Bright Data run recovery.
- Secrets load once at start-up into `http_guard::Secret`. A variable that is set but empty, whitespace-only, contains control characters or is not UTF-8 stops start-up with an error that names the variable, never the value. The log shows only which names are set. Top-level errors pass through `http_guard::Redactor` before they are logged.
- Bright Data run recovery: with `DATABASE_URL` set, the worker connects, takes the Bright Data driver lock (`db::brightdata_runs::try_driver_lock`) and runs `brightdata::recovery::recover_interrupted`, logging how many runs were failed and kept; the lock is held until the worker exits. Still under the lock it resumes each kept dataset run with `Runner::resume_run` (default timing and caps, so one resume may poll for up to 10 minutes) and logs each outcome and a finished/active/refused summary. If another worker holds the lock, recovery is skipped (its runs are live). Without `DATABASE_URL`, recovery is skipped and logged. Database errors pass through `DatabaseUrl::redactor`.
- Bright Data client (`src/brightdata_resume.rs`): built from `BRIGHTDATA_API_KEY` and `BRIGHTDATA_UNLOCKER_ZONE` via `BrightDataConfig::from_secrets` only when `DATABASE_URL` is set. If either is missing the log names the setting and the runner gets `transport: None`, so every resume fails the key gate (closed) without an HTTP request. Resume messages pass through `ServerSecrets::redactor`.
- Result sink: no product importer exists yet, so the worker uses `RefusingSink`, which refuses every page and record with fixed text. A downloaded run therefore stays `ready` and resumable (chosen over failing it, so the paid snapshot is not given up); it keeps its scope's active slot until a real sink imports it or it expires at a start-up more than 24 h later. Replace `RefusingSink` when the first importer lands.
- `PgRunStore` (`src/brightdata_store.rs`) implements `brightdata::runs::RunStore` over `db::brightdata_runs`; storage failures return fixed text and log the redacted detail. A missing settings row reads as kill switch on and connector disabled.

## Dependencies

- Workspace: `domain`, `db`, `matching`, `retailers`, `amazon`, `http-guard` (secret types and redaction), `brightdata` (run store and recovery now; the Amazon.com.mx fallback when SP-API cannot answer later, [ADR 0003](../decisions/0003-amazon-sp-api-primary-brightdata-fallback.md)).
- External: `tokio`, `tracing`, `tracing-subscriber`, `anyhow`, `serde_json` (the `RunSink` record type).

## Owned tables

None directly; data access goes through `db`.
