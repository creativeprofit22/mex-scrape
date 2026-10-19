# worker

## Purpose

Background job runner: fetching retailer and Amazon prices, matching, price history. For now it loads server secrets, logs which ones are set, and exits.

## Public API

- Binary `worker` (`cargo run -p worker`).
- Environment (all optional for now; each adapter will require its own once it exists):
  - `BRIGHTDATA_API_KEY`, `BRIGHTDATA_UNLOCKER_ZONE`
  - `SPAPI_LWA_CLIENT_ID`, `SPAPI_LWA_CLIENT_SECRET`, `SPAPI_REFRESH_TOKEN`
  - `MODEL_PROVIDER_API_KEY`
- Secrets load once at start-up into `http_guard::Secret`. A variable that is set but empty, whitespace-only, contains control characters or is not UTF-8 stops start-up with an error that names the variable, never the value. The log shows only which names are set. Top-level errors pass through `http_guard::Redactor` before they are logged.

## Dependencies

- Workspace: `domain`, `db`, `matching`, `retailers`, `amazon`, `http-guard` (secret types and redaction).
- Also allowed: `brightdata`, for the Amazon.com.mx page fallback when SP-API cannot answer ([ADR 0003](../decisions/0003-amazon-sp-api-primary-brightdata-fallback.md)). Not a Cargo dependency yet.
- External: `tokio`, `tracing`, `tracing-subscriber`, `anyhow`.

## Owned tables

None directly; data access goes through `db`.
