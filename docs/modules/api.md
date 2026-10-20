# api

## Purpose

axum HTTP server for the web app. Publishes its OpenAPI spec with utoipa's `OpenApiRouter`; the spec is the contract the `web` client is generated from. Owns accounts, cookie sessions, CSRF and rate limits ([ADR 0012](../decisions/0012-session-auth-csrf-rate-limits-i18n.md)).

## Public API

Errors on the user routes are `{"error": "<code>"}` with a fixed, machine-readable code (`invalid_email`, `weak_password`, `email_taken`, `invalid_credentials`, `unauthenticated`, `csrf`, `rate_limited`, `invalid_body`, `invalid_json`, `unavailable`, `internal`, …). The web app translates them; they never carry detail.

### Public routes

- `GET /health` → `{"status":"ok"}` (liveness only, no database call).
- `GET /api-docs/openapi.json`: the OpenAPI spec. Security schemes: `admin_bearer` (HTTP bearer, `ADMIN_BEARER`), `session_cookie` (cookie `mex_session`, `SESSION_SCHEME`) and `csrf_token` (header `X-CSRF-Token`, `CSRF_SCHEME`).

### Accounts and sessions (module `auth`)

- `POST /auth/signup` `{email, password, locale?}` → `201 {user, csrf_token}`: the e-mail is trimmed and lowercased (`domain::Email`, ≤ 254 bytes), the password needs 8–128 characters, and `locale` is `es` (default) or `en`. Signs the new user in. 409 `email_taken`; 422 `invalid_email`, `weak_password` or `invalid_locale`.
- `POST /auth/signin` `{email, password}` → `200 {user, csrf_token}`; 401 `invalid_credentials` for an unknown e-mail or a wrong password (same answer and similar timing: `app::SignIn` verifies a dummy hash for unknown e-mails).
- `POST /auth/signout` → 204; the session is deleted.
- `GET /auth/csrf` → `{csrf_token}`: the session's token (created on first request).
- `GET /me` → `{id, email, plan, has_costco, has_sams, onboarded, locale}`; 401 when signed out.
- `PUT /me/onboarding` `{has_costco, has_sams}` → the updated account, with `onboarded` set to true.
- `PUT /me/locale` `{locale}` → the updated account; 422 `invalid_locale`.
- Sessions: `axum-login` over `tower-sessions`. The cookie `mex_session` holds only the random session id and is `HttpOnly`, `SameSite=Lax`, `Path=/`, `Secure` (unless `SESSION_COOKIE_SECURE=false`, for local plain HTTP only), and expires 7 days after the last request: the layer saves every non-empty session on each user-route request (`with_always_save`), which costs one `sessions` UPSERT per such request and refreshes the cookie's `Max-Age`. The session id and CSRF token rotate on every sign-in. Records live in Postgres (`sessions`, through `auth::store::PgSessionStore` over `db::sessions`); the binary deletes expired rows hourly. The user is reloaded from storage on every request, so plan and membership changes apply at once.
- Passwords: `auth::hasher::Argon2Hasher` (`password-auth`, Argon2id PHC strings) implements `app::PasswordHasher`; hashing runs on the blocking pool.
- CSRF (`auth::csrf_guard`, fails closed with 403 `csrf`): every non-GET/HEAD/OPTIONS user route needs `X-CSRF-Token` equal to the session's token (constant-time compare). When the browser sends them, `Sec-Fetch-Site` must be `same-origin` and `Origin` must be one of `APP_ORIGINS` (comma-separated; default `http://localhost:3000,http://127.0.0.1:3000`).

### Routes for signed-in users

All need the session cookie (401 `unauthenticated` otherwise) and work in the user's own tenant, `user-<id>` (`domain::TenantId::for_user`).

- `POST /searches` (operation `start_search`) `{query}` → `202 {id}`: validates the query (`domain::SearchQuery`: trimmed, 1–100 characters) and stores the search with its `search_jobs` message in one transaction (`app::StartSearch` over `db::searches::PgSearches`); the worker runs it.
- `GET /searches/{id}` (operation `get_search`) → `{id, query, status, reason, created_at_ms, finished_at_ms, hidden_results, retailers: [{retailer, outcome, offers}], results: [{retailer, retailer_sku, title, url, price_centavos, availability, membership_required, amazon: null | {asin, match_status, confidence_bp, amazon_price_centavos, fees_centavos, profit_centavos, roi_bp, margin_bp}}]}`. `outcome` includes `category_only` for stores without keyword search (ADR 0008). Results that need a membership the user lacks (Costco, Sam's Club; any other membership store always) are removed and counted in `hidden_results` (`app::SearchResults::get` with the user's `Memberships`). Another tenant's id is 404.
- `GET /products/{asin}/price-stats` (operation `product_price_stats`) → `{asin, as_of_ms, hidden_listings, listings: [{retailer, retailer_sku, windows: [{days, low_centavos, high_centavos, average_centavos, observations}]}]}`: low, high and average for 30, 90 and 365 days per retailer listing with an accepted match to the ASIN (`app::PriceStats`; rules in [domain.md](domain.md#price-history)). `null` for an empty window. Listings from a membership store the user lacks (Costco, Sam's Club) are removed and counted in `hidden_listings`, exactly as `GET /searches/{id}` hides results (the handler passes the user's `Memberships`).
- `GET /products/{asin}/price-history?days=1..365` (operation `product_price_history`, default 365) → `{asin, since_ms, as_of_ms, hidden_listings, listings: [{retailer, retailer_sku, points: [{day_ms, low_centavos, high_centavos, in_stock}]}]}`: one point per UTC day with observations, oldest first, for the chart (`app::PriceSeries`), with the same membership hiding and `hidden_listings` count. 422 for `days` outside 1–365.
- `POST /roi` (operation `calculate_roi`) `{items: [{sell_price_centavos, buy_price_centavos, amazon_fees_centavos, prep_fee_centavos?, shipping_centavos?, monthly_fixed_centavos?, units_per_month?}]}` → `{results: [{error, fixed_cost_per_unit_centavos, total_cost_centavos, profit_centavos, roi_bp, margin_bp}]}` in input order (`app::RoiBatch` over `domain::calculate_roi`, the same function the worker uses). 1–100 items; 422 `empty`, `too_many` or `invalid_amount` (negative). A per-item `error` is `zero_units` or `overflow`. The calculator and search pages both use it.
- Common statuses: 400 for a malformed body or invalid ASIN, 403 for CSRF, 422 for invalid input, 500 (fixed log message, no detail) on a storage error, 503 when the database is not configured.

### Rate limits (module `ratelimit`)

- `tower_governor` with one `GovernorLayer` per quota; a dispatch middleware sends each user-route request through its caller's layer: signed-in users by user id with their plan's quota, everyone else (sign-up, sign-in, CSRF token, 401s) by client IP.
- Quotas (sustained per minute / burst), set with `RATE_LIMIT_{FREE,PRO,ANON}_{PER_MINUTE,BURST}`: free 60/30, pro 300/100, anonymous 20/10 per IP. Invalid values keep the default (with a warning).
- Over the limit: 429 `rate_limited` with `Retry-After` (whole seconds, at least 1).
- The client IP is the TCP peer, unless `CLIENT_IP_HEADER` names one header (for example `x-real-ip`) that a trusted front proxy **overwrites** on every request. That header must hold exactly one IP; a missing, repeated, list-valued or malformed header falls back to the peer. No other forwarded header is ever read, and the leftmost `X-Forwarded-For` value is never used. Unknown IPs share one bucket.
- Topology: browser → front proxy (nginx/Caddy, sets the header) → Next.js `/backend` rewrite → api. The rewrite forwards client-sent headers verbatim and adds none, so without a front proxy that overwrites the header, a client could pick its own bucket. Without `CLIENT_IP_HEADER` the peer is always the Next server, so every anonymous visitor shares one bucket; the api logs a warning at startup.
- `TRUST_PROXY_HEADERS=true` is deprecated and means `CLIENT_IP_HEADER=x-real-ip` (with a warning); `CLIENT_IP_HEADER` wins when both are set. An invalid header name is ignored with a warning.

### Admin routes

- `GET /admin/brightdata/access` → `[{retailer, host, ok, blocked_needs_kyc, skipped_robots, failed}]`: Bright Data access outcomes per host (from the `app::AccessReport` use case: retailer id from the `retailers` registry, else the host). Read-only. Needs the `ADMIN_API_TOKEN` bearer (constant-time compare), not a session. Fails closed: 503 when `ADMIN_API_TOKEN` is unset or shorter than 16 bytes, or `DATABASE_URL` is missing or unreachable; 401 for a missing or wrong token; 500 (no detail) if the counts cannot be read.

### Library and binaries

- `build(ApiConfig)` returns the router and spec. `ApiConfig { admin, products, searches, accounts, rate_limits }`; `ApiConfig::disabled()` makes every route except `/health` and the spec answer 503. `router()` is `build(ApiConfig::disabled())`, `router_with(AdminState)` enables only the admin route, `openapi()` returns the spec.
- States: `AdminState::new(token, Arc<AccessReport>)`, `ProductsState::new(Arc<PriceStats>, Arc<PriceSeries>)`, `SearchesState::new(StartSearch, SearchResults)`, `AccountsState::new(Arc<dyn UserRepo>, Arc<dyn PasswordHasher>, SessionStoreHandle, SessionConfig)`; each has `disabled()`. Without accounts, every user route answers 503 before any extractor runs. `RateLimits::defaults()` holds the default quotas.
- Binary `api`: serves on `API_ADDR` (default `127.0.0.1:8080`) with peer addresses for the IP limits. It is the composition root: with `DATABASE_URL` it builds one pool and injects `db::users::PgUsers`, `PgSessionStore`, `Argon2Hasher`, `db::searches::PgSearches`, `db::price_history::{PgPriceHistory, PgMatches}`, `db::access::PgAccessCounts`, `retailers::RetailerOfferSource::lookup_only()` and a `SystemClock` into the use cases. `ADMIN_API_TOKEN` also enables the admin route. Env: `SESSION_COOKIE_SECURE`, `APP_ORIGINS`, `CLIENT_IP_HEADER` (deprecated alias `TRUST_PROXY_HEADERS`), the `RATE_LIMIT_*` quotas (see `.env.example`).
- Binary `openapi <file>`: writes the spec as JSON (used by `npm --prefix web run gen:api`).
- Tests (fakes, fixed clocks, in-memory session store; no database): `tests/accounts.rs` (sign-up/in/out, cookie flags, session rotation, CSRF and origin 403s, onboarding, locale), `tests/searches.rs` (tenant isolation, membership hiding, statuses, spec), `tests/roi.rs` (parity with `domain::calculate_roi`, limits), `tests/rate_limits.rs` (429 and `Retry-After` per plan and per IP), `tests/price_stats.rs` (stats and history shapes, membership hiding), `tests/admin.rs`, `tests/health.rs`.

## Dependencies

- Workspace: `domain`, `app` (use cases and ports), `db` (`PgAccessCounts`, `PgPriceHistory`, `PgMatches`, `PgSearches`, `PgUsers`, `sessions`), `matching`, `retailers` (`RetailerOfferSource`, wired in the binary).
- External: `axum`, `axum-login`, `tower-sessions`, `tower`, `tower_governor`, `governor`, `password-auth`, `subtle`, `time`, `utoipa`, `utoipa-axum`, `serde`, `serde_json`, `tokio`, `tracing`, `thiserror`, `anyhow`, `async-trait`.

## Owned tables

None directly; data access goes through `db` (`users`, `sessions`, `searches`, price history).
