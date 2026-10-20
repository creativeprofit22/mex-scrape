# worker

## Purpose

Background job runner ([ADR 0011](../decisions/0011-pgmq-jobs-schedules-caps.md)): it owns all slow, paid work — Bright Data fetches, Amazon (SP-API) calls, matching and price-history writes. The `api` only enqueues searches and reads results.

At start-up it loads secrets and settings (failing closed), and, when `DATABASE_URL` is set, waits for the Bright Data driver lock, recovers interrupted runs, then consumes the job queues and runs the cron schedules until Ctrl-C/SIGTERM. Without `DATABASE_URL` it logs "no jobs run" and exits.

The crate is a library (`src/lib.rs`: `config`, `clock`, `jobs`, `schedule`, `metrics`, `sinks`, plus the Bright Data and Amazon wiring) with `src/main.rs` as the composition root, so `tests/` can drive the jobs.

## Queues

pgmq queues (migrations `0005_pgmq.sql`, `0006_jobs.sql`), accessed only through `db::queue` and its closed `Queue` enum.

| Queue | Messages | Dead-letter queue |
|---|---|---|
| `search_jobs` | `{"kind":"search","search_id":N,"tenant_id":"user-7"}` (sent by `POST /searches` in the same transaction as the `searches` row) | `search_jobs_dlq` |
| `refresh_jobs` | `{"kind":"product_refresh","retailer":"liverpool","skus":["…",…]}` (1–50 SKUs) and `{"kind":"category_browse","retailer":"sears_mx"}` (sent by the schedules) | `refresh_jobs_dlq` |

Messages are decoded strictly (`jobs::JobMessage::parse`): unknown kinds, missing or extra fields and invalid values are malformed.

Consumer (`jobs::consumer`), one message at a time, `search_jobs` first, then `refresh_jobs`; sleeps `JOB_POLL_IDLE_MS` when both are empty:

- A read hides the message for `JOB_VISIBILITY_SECS` (default 900, above the 600 s Bright Data poll budget). A worker that dies mid-job lets it reappear with a higher `read_ct`.
- Success or **refusal** ⇒ delete. A refusal (kill switch, connector disabled, missing key, a daily or per-run cap) is never retried: a search is finished `refused` with the gate code as `reason`.
- **Transient** failure (storage, another run active in the scope) ⇒ `set_vt` backoff of 30 s · 2^(attempt−1), at most 15 min, until `read_ct` reaches `JOB_MAX_ATTEMPTS` (default 5); then dead-letter. A message read more than `JOB_MAX_ATTEMPTS` times without finishing (crashing worker) is dead-lettered before it runs again.
- **Permanent** failure (unknown search, tenant mismatch, no valid SKUs) or malformed message ⇒ dead-letter at once.
- Dead letters are `{error_code, attempts, queue, message}` (the original body), moved with send + delete in one transaction. A search that is dead-lettered is finished `failed` with the error code as reason.

## Jobs

All three use one gated Bright Data run each (`Runner::start_unlocker_run`, so the gate order applies: kill switch, connector enabled, key present, caps, one active run per scope; robots.txt pre-check over `robots_checker()`), parse every page through `retailers::parse_page` with the shared `ParseHealth`, and emit per-retailer metrics.

- **search** (`jobs::search`, tenant from the message, scope `<tenant>/search`): marks the search `running`, builds each adapter's search URL — stores without keyword search (Sears MX, Sanborns; ADR 0008) get outcome `category_only` and no request — and fetches all of them in one run per access mode. Per retailer it writes `search_retailers` (`ok`, `no_offers`, `blocked_needs_kyc`, `skipped_robots`, `failed`, `category_only`). For the first `SEARCH_MAX_OFFERS_PER_RETAILER` distinct offers it runs `match_and_store` (price recorded in `price_observations`, match upserted); for accepted / needs-review matches it fetches Amazon offers (buy box, else lowest new) and FBA fees and computes ROI (`ROI_PREP_FEE_CENTAVOS`, `ROI_SHIPPING_CENTAVOS`), writing `search_results` with the offer's `availability` and `membership_required` (the API hides membership offers the user lacks, ADR 0012). Accepted matches are added to `tracked_products`. Final status: `done`; `partial` with reason `amazon_refused` / `retailers_failed` / `amazon_unavailable`; `failed` (`all_retailers_failed`); or `refused`. After the first Amazon refusal no more Amazon calls are made; remaining prices are still recorded.
- **product_refresh** (`jobs::refresh`, tenant `system`, scope `system/refresh`): fetches the product pages of the batch (Unlocker or Browser per `RetailerInfo::access_for(Product)`), records each parsed offer in `price_observations` and sets `tracked_products.last_refreshed_at`. Once the run returns, every SKU of the batch gets `last_attempted_at`, whether or not an offer parsed (blocked, robots, delisted, unparseable), so failing listings rotate behind healthy ones instead of filling every tick.
- **category_browse** (`jobs::category`, tenant `system`, scope `system/category`): fetches the first `CATEGORY_MAX_PAGES_PER_RUN` configured listings of the retailer through the Browser API in one run, then runs `match_and_store` for every offer (writing `price_observations` and matches) and tracks accepted matches.

## Schedules

`schedule::run` (UTC 5-field cron via `croner`; the loop reads an injected clock and wakes at least once a minute). Only the driver-lock holder runs it, so ticks never double-fire.

| Variable | Default | Enqueues |
|---|---|---|
| `REFRESH_CRON` | `0 */6 * * *` | `product_refresh` batches (`REFRESH_BATCH_SIZE` SKUs, one retailer each) of tracked listings not tried (attempted, else refreshed) for 1 h, oldest first, at most 4 batches per tick. Skipped while `refresh_jobs` still holds messages. |
| `CATEGORY_BROWSE_CRON` | `30 3 * * *` | one `category_browse` per retailer with configured category listings. |

Category listings: `CATEGORY_LISTINGS_PATH` (default `config/category_listings.json`), validated at start-up by `retailers::category::parse_listings` (registered category-only retailer, full menu link on one of its hosts with a `/cat/` path and a numeric `id=` parameter, no duplicates; see [retailers.md](retailers.md)). An invalid file stops the worker; the log shows per retailer how many listings are configured, browsed and skipped by the page cap.

## Caps and kill switch

Checked before any paid call; every refusal is fixed text with no secrets.

- **Bright Data** (`brightdata::runs::check_gates`, inside `Runner`): global kill switch and connector-enabled flag (`safety_settings.global_kill_switch` and `brightdata_enabled`; a missing row reads as kill switch on), API key present, `BRIGHTDATA_MAX_RUNS_PER_TENANT_PER_DAY` (20), `BRIGHTDATA_MAX_ITEMS_PER_TENANT_PER_DAY` (500), `BRIGHTDATA_MAX_ITEMS_PER_RUN` (50), one active run per scope. A search is one run, so the default allows 20 searches + scheduled jobs per tenant per day.
- **Amazon** (SP-API and its Bright Data fallback): jobs wrap the source in `app::GatedAmazonSource` for the paying tenant. Before each call, `db::usage::PgRequestBudget` atomically checks the same kill switch and consumes one of `AMAZON_MAX_REQUESTS_PER_TENANT_PER_DAY` (1000) in `provider_usage` (UTC day). A refusal is `AmazonSourceError::Refused` and never falls back.

## Metrics and logs

`tracing` with `RUST_LOG`-style filtering (default `info`). Each job runs in a `job` span with `kind`, `msg_id`, `attempt` and `tenant`; errors pass through the `Redactor` before logging.

`metrics::JobMetrics` logs one event per retailer per job, sorted by retailer, target `mex_scrape::metrics`, message "job retailer metrics", fields `job`, `msg_id`, `tenant`, `retailer`, `requests`, `parse_ok`, `parse_failed`, `blocked_needs_kyc`, `skipped_robots`, `est_cost_microusd`. The cost is an **estimate**: Unlocker pages × `BRIGHTDATA_UNLOCKER_MICROUSD_PER_REQUEST` (1500) plus Browser HTML bytes × `BRIGHTDATA_BROWSER_MICROUSD_PER_MB` (8000); Browser traffic is underestimated (scripts, images and fonts are not counted). Parse-rate alerts come from the shared `ParseHealth`.

## Runbook

All SQL runs against `DATABASE_URL` (e.g. `docker compose exec db psql -U mex mex_scrape`).

- **Stop all paid calls now:** `UPDATE safety_settings SET global_kill_switch = true, updated_at = now() WHERE id = 1;` Takes effect on the next gate check (Bright Data run start and every Amazon call); jobs then finish `refused` and their messages are deleted, not retried. Turn it off with `false`.
- **Check queues:** `SELECT * FROM pgmq.metrics_all();` and peek without consuming: `SELECT msg_id, read_ct, vt, message FROM pgmq.q_search_jobs ORDER BY msg_id;` (same for `q_refresh_jobs`, `q_search_jobs_dlq`, `q_refresh_jobs_dlq`).
- **Inspect the DLQ:** `SELECT msg_id, enqueued_at, message->>'error_code', message->'attempts', message->'message' FROM pgmq.q_search_jobs_dlq;`
- **Replay a dead letter** (after fixing the cause): `SELECT pgmq.send('search_jobs', message->'message') FROM pgmq.q_search_jobs_dlq WHERE msg_id = <id>;` then `SELECT pgmq.delete('search_jobs_dlq', <id>);`. For a search, first reset it: `UPDATE searches SET status = 'queued', reason = '', finished_at = NULL, updated_at = now() WHERE id = <search_id>;` (terminal searches are skipped).
- **Stuck message** (a job that keeps timing out): it reappears after `JOB_VISIBILITY_SECS` and is dead-lettered once read more than `JOB_MAX_ATTEMPTS` times. To drop it now: `SELECT pgmq.delete('<queue>', <msg_id>);`. To run it now: `SELECT pgmq.set_vt('<queue>', <msg_id>, 0);`. A job blocked by `active_run` waits for the active Bright Data run of its scope (see `brightdata_runs`), retrying with backoff.
- **Raise caps:** set the `BRIGHTDATA_MAX_*` / `AMAZON_MAX_REQUESTS_PER_TENANT_PER_DAY` variables and restart the worker. Today's Amazon usage: `SELECT * FROM provider_usage WHERE day = current_date;`
- **Add category links:** append `{"retailer": "sears_mx", "url": "https://www.sears.com.mx/cat/…?id=…"}` entries (full menu links from the store's menu) to the `listings` array in `config/category_listings.json`, keep the browsed ones within `CATEGORY_MAX_PAGES_PER_RUN`, run `cargo test -p retailers` (the checked-in file is validated by a test) and restart; the start-up log shows the counts.
- **Track a listing by hand:** `INSERT INTO tracked_products (retailer, retailer_sku, tracked_at) VALUES ('liverpool', '<sku>', now()) ON CONFLICT DO NOTHING;`
- **Second worker:** it waits for the driver lock, retrying every 60 s, and takes over (recovery first) when the first one stops.

## Public API

- Binary `worker` (`cargo run -p worker`).
- Environment (secrets; all optional, each feature fails closed without its own):
  - `BRIGHTDATA_API_KEY`, `BRIGHTDATA_UNLOCKER_ZONE`
  - `BRIGHTDATA_BROWSER_ZONE` (optional): Bright Data Browser API zone, only for store pages proven to need JavaScript ([ADR 0007](../decisions/0007-brightdata-browser-api.md)). Without it, Browser API fetches (category browses) fail closed.
  - `SPAPI_LWA_CLIENT_ID`, `SPAPI_LWA_CLIENT_SECRET`, `SPAPI_REFRESH_TOKEN`: Amazon SP-API (LWA) credentials, read through `ServerSecrets::spapi()`. All three are needed; with only some set, the worker logs a warning naming the first missing one and uses the fallback only. How to obtain them: [amazon.md](amazon.md#owner-setup-sp-api-registration).
  - `MODEL_PROVIDER_API_KEY`
  - `DATABASE_URL`: enables recovery, jobs and schedules.
- Settings (`config::WorkerConfig::from_lookup`; unset keeps the default, a malformed or out-of-range value stops start-up naming the variable only): `REFRESH_CRON`, `CATEGORY_BROWSE_CRON`, `JOB_VISIBILITY_SECS` (900, 660–86400), `JOB_MAX_ATTEMPTS` (5, 1–50), `JOB_POLL_IDLE_MS` (2000, 100–60000), the Bright Data and Amazon caps above, `SEARCH_MAX_OFFERS_PER_RETAILER` (3, 1–20), `REFRESH_BATCH_SIZE` (25, 1–50), `CATEGORY_MAX_PAGES_PER_RUN` (4, 1–50), `ROI_PREP_FEE_CENTAVOS` / `ROI_SHIPPING_CENTAVOS` (0), the two cost rates, `CATEGORY_LISTINGS_PATH`. All listed in `.env.example`.
  - `RETAILER_PARSE_ALERT_THRESHOLD` (default 0.8, in 0–1) and `RETAILER_PARSE_ALERT_MIN_SAMPLES` (default 20): the parse-health alert settings (`retailers::HealthConfig`) of the shared `ParseHealth` the jobs feed; a per-retailer success rate below the threshold logs a warning.
- Secrets load once at start-up into `http_guard::Secret`. A variable that is set but empty, whitespace-only, contains control characters or is not UTF-8 stops start-up with an error that names the variable, never the value. The log shows only which names are set. Top-level errors pass through `http_guard::Redactor` before they are logged.
- Driver lock and recovery: the worker takes the Bright Data driver lock (`db::brightdata_runs::try_driver_lock`), retrying every 60 s while another worker holds it, and holds it until it exits. Under the lock it runs `brightdata::recovery::recover_interrupted`, logging how many runs were failed and kept, and resumes each kept dataset run with `Runner::resume_run` (one resume may poll for up to 10 minutes). Kept Amazon fallback runs (job `amazon::FALLBACK_JOB`, any tenant) are not resumed: `resume_kept` fails them with `FALLBACK_INTERRUPTED` and, best effort, cancels their snapshot (counted as `abandoned`). Kept job runs (scopes `…/search`, `…/refresh`, `…/category`) are Unlocker/Browser runs, which recovery fails rather than keeps; their messages reappear and run again. Database errors pass through `DatabaseUrl::redactor`.
- Dataset resume sink: no dataset importer exists, so resumed dataset runs go to `RefusingSink`, which refuses every page and record with fixed text. A downloaded run therefore stays `ready` and resumable (so the paid snapshot is not given up) until an importer lands or it expires at a start-up more than 24 h later. Job runs use `sinks::OfferCollector` instead.
- Bright Data client (`src/brightdata_resume.rs`): built from `BRIGHTDATA_API_KEY` and `BRIGHTDATA_UNLOCKER_ZONE` via `BrightDataConfig::from_secrets` (plus the optional browser zone). If either is missing the log names the setting and the runner gets `transport: None`, so every run fails the key gate (refused) without an HTTP request.
- Robots pre-check: `robots_checker()` builds a `RobotsChecker` over the `retailers` hosts; the job `Runner` uses it, so disallowed store paths are skipped without a paid request (`skipped_robots`).
- Access metrics (`src/access_metrics.rs`): after recovery, logs per-retailer `ok` / `blocked_needs_kyc` / `skipped_robots` / `failed` totals from `app::AccessReport::per_retailer`.
- Binary `access_check` (`cargo run -p worker --bin access_check [--live] [--max-urls N] [--out PATH]`): checks robots.txt for the registry's sample URLs (free) and writes the dated matrix `docs/brightdata-access.md`. `--live` also fetches each allowed sample once through the channel production uses (needs `BRIGHTDATA_API_KEY` and `BRIGHTDATA_UNLOCKER_ZONE`; retries off, one request per URL, hard cap 36): page kinds where `RetailerInfo::access_for(kind)` is `Browser` (Sears MX and Sanborns categories) go through the Browser API and are marked *(Browser API)* in the matrix; everything else goes through Web Unlocker. Browser pages need the optional `BRIGHTDATA_BROWSER_ZONE`; without it they are recorded as "not checked (needs BRIGHTDATA_BROWSER_ZONE)" with no request. No bodies or secrets are stored.
- Binary `capture_fixtures` (`cargo run -p worker --bin capture_fixtures -- --retailer ID --kind search|product|both [--query Q] [--url URL] [--out-dir DIR] [--browser [--no-block]] [--resanitize]`): captures parser fixtures to `crates/retailers/tests/fixtures/<id>/`. It checks robots.txt first and skips disallowed pages without fetching them. It then makes one request per page (Web Unlocker, or the Browser API with `--browser`, which needs `BRIGHTDATA_BROWSER_ZONE`; `--no-block` lets that session download images, fonts and stylesheets too, at a higher cost), country MX, retries off, hard cap 2 pages per run. Bodies pass through `retailers::fixture::sanitize` before they are written. `--resanitize` re-sanitizes existing fixtures offline.
- Amazon source (`src/amazon_source.rs`): `amazon_source(spapi_secrets, fallback, caps, redactor)` returns `TenantAmazonSources`, a `TenantAmazon`: `for_tenant(tenant)` builds `app::FallbackAmazonSource` over the shared `amazon::SpApiAmazonSource` (when all three SP-API secrets are set) and a fresh `amazon::BrightDataAmazonSource::for_tenant` (when the Bright Data client exists; it uses `PgRunStore`, so the kill switch and durable runs apply, with `WorkerConfig.caps` (the `BRIGHTDATA_MAX_*` settings, passed via `with_caps`) as its caps, scope `<tenant>` / `amazon_mx_fallback`). Fallback dataset runs therefore count against the paying tenant's daily Bright Data caps, not `system`'s. It logs `sp_api` / `bright_data` booleans, never values. `JobContext::amazon_for(tenant)` wraps the tenant's source in `GatedAmazonSource` for the same tenant (see Caps). `SharedAmazon` gives one source to every tenant (tests).
- Offer matching (`src/offer_matching.rs`): `match_and_store(amazon, matches, prices, &Offer, &MatchConfig, now)` records the offer's price (`PriceHistoryRepo::record`, even if the lookup then fails), looks it up on Amazon by GTIN when it has one and by `brand + title` keywords only when the GTIN finds nothing (cut on a word boundary to at most `amazon::MAX_KEYWORD_CHARS` = 200 characters), runs `matching::match_offer` and upserts the result (`MatchRepo`). Returns `MatchOutcome { Stored(ProductMatch), KeptRejected, NoCandidate }` or `OfferMatchError { Storage, Amazon(AmazonSourceError) }` (an Amazon error stores no match). Used by the search and category jobs.
- `PgRunStore` (`src/brightdata_store.rs`) implements `brightdata::runs::RunStore` over `db::brightdata_runs`; storage failures return fixed text and log the redacted detail. A missing settings row reads as kill switch on and connector disabled.

## Tests

- Unit tests (fakes, injected clocks): message decoding, backoff and retry/dead-letter decisions, gate → refusal mapping, cron next tick and schedule planning, metrics and cost math, settings parsing, search outcome/status rules.
- `tests/jobs.rs` (ignored; needs Postgres at `DATABASE_URL` with migrations applied: `cargo test -p worker --test jobs -- --ignored`): runs the real consumer and jobs against Postgres with a fake Bright Data `Transport` serving retailer fixtures and a fake `AmazonSource`. It covers a search writing results, ROI and `category_only` rows; retry then dead-letter; the kill switch refusing with zero transport and Amazon calls; a refresh writing `price_observations`; and a category browse respecting the page cap. CI runs it in the `migrations` job.

## Dependencies

- Workspace: `domain`, `app` (use cases and ports; the worker is a composition root), `db`, `matching`, `retailers`, `amazon`, `http-guard` (secret types and redaction), `brightdata` (run store, recovery, runs, and the client handed to the Amazon fallback, [ADR 0010](../decisions/0010-amazon-spapi-lwa-brightdata-dataset-fallback.md)).
- External: `tokio`, `tracing`, `tracing-subscriber`, `anyhow`, `thiserror`, `serde_json`, `croner` + `chrono` (cron parsing, ADR 0011).

## Owned tables

None directly; data access (including the pgmq queues) goes through `db`.
