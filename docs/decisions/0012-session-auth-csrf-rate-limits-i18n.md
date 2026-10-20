# 0012. Accounts: cookie sessions, CSRF, per-plan rate limits and Spanish-first i18n

- Status: Accepted
- Date: 2026-10-19

## Context

The web app needs real users: sign-up and sign-in, per-user data, Costco and Sam's Club memberships that decide which offers a user can buy, and an English option next to the Spanish default. Until now every search ran in tenant `default` behind the admin bearer (ADR 0011), which no browser user can hold.

Constraints found while planning:

- The backend is axum 0.8 with sqlx 0.9. `tower-sessions-sqlx-store` is built on sqlx 0.8, so it cannot share our pool or migrations.
- `tower_governor` applies one quota per layer, but quotas differ by plan.
- The browser reaches the API through the Next.js `/backend` rewrite, so API requests are same-origin from the browser's view. Behind a proxy, the peer address is the proxy, and forwarded headers can be forged by any client.
- `app` must stay free of I/O and web crates (ADR 0009).
- Next.js 16 renamed middleware to `proxy` and has no built-in i18n routing for the App Router.

## Decision

- **Accounts.** `domain` gains `UserId`, `Email` (trimmed, lowercased, ≤ 254 bytes), the password length policy (8–128 characters), `Plan { Free, Pro }`, `Locale { Es, En }`, `Memberships` and `TenantId::for_user` (`user-<id>`). `app` gains the ports `UserRepo` and `PasswordHasher` and the use cases `SignUp`, `SignIn`, `Profile` and `UpdateOnboarding`. `SignIn` verifies a dummy hash for unknown e-mails, so timing does not reveal which accounts exist. Passwords are hashed with Argon2id (`password-auth`) in `api`, on the blocking pool.
- **Tenant per user.** `users.tenant_id` is a generated column (`'user-' || id`), so no request can choose a tenant. Search routes take the tenant from the session and read through the existing tenant-scoped queries; another user's search is 404.
- **Sessions.** `axum-login` over `tower-sessions`, with our own `SessionStore` (`api::auth::store::PgSessionStore` over `db::sessions`, table `sessions` in migration `0008`) instead of `tower-sessions-sqlx-store`. The cookie `mex_session` holds only the random id: `HttpOnly`, `SameSite=Lax`, `Secure` unless `SESSION_COOKIE_SECURE=false` (local HTTP only), 7 days after the last request (the expiry slides on every request). Sign-in rotates the session id and the CSRF token. The user is reloaded on every request, so plan and membership changes apply at once.
- **CSRF.** Synchronizer token: `GET /auth/csrf` (and the sign-in/sign-up response) returns a per-session token, and every non-GET user route needs it in `X-CSRF-Token` (constant-time compare). When present, `Sec-Fetch-Site` must be `same-origin` and `Origin` one of `APP_ORIGINS`. Failures are 403; nothing passes by default.
- **Memberships.** The worker stores `membership_required` and `availability` on each `search_results` row. `app::SearchResults::get` drops results the user's `Memberships` do not allow and reports the count as `hidden_results`; an unknown membership store is always hidden.
- **One ROI endpoint.** `POST /roi` wraps `domain::calculate_roi` for up to 100 items (`app::RoiBatch`). The calculator and the search page both call it, so their numbers always match each other and the worker.
- **Price history.** `GET /products/{asin}/price-history` (`app::PriceSeries`) returns daily low/high per listing, computed in `app` from the existing `PriceHistoryRepo::history` rather than a new SQL aggregate.
- **Rate limits.** One `GovernorLayer` per quota (free, pro, anonymous) and a small dispatch middleware that routes each request to its caller's layer: signed-in users by user id, others by client IP. Quotas come from `RATE_LIMIT_*` env vars (defaults 60/min burst 30, 300/min burst 100, 20/min burst 10 per IP). 429 carries `Retry-After`. Forwarded headers are trusted only with `TRUST_PROXY_HEADERS=true`.
- **i18n.** Routes live under `src/app/[lang]` (`es` default, `en`). `src/proxy.ts` redirects paths without a language to the `lang` cookie's language or Spanish. All UI text is in `src/i18n/messages/{es,en}.json`. ESLint `react/jsx-no-literals` rejects text in JSX, and `scripts/check-messages.mjs` (part of `npm run lint`) fails when the catalogs' keys differ or a value is empty.
- **End-to-end tests.** Playwright with `@axe-core/playwright` (both pinned) runs sign in → search → product page in both languages, keyboard only, with zero axe violations (WCAG 2.1 A/AA) on every page. The `db` binary `seed_e2e` seeds the account and history and completes searches, so CI needs no worker, Bright Data or Amazon access.

## Consequences

- Breaking API change: `/searches` and `/products/*` need a session instead of the admin bearer. The web client is the only consumer and was regenerated. `/admin/*` keeps the bearer.
- We maintain about 150 lines of session-store code, tested with the db integration suite, instead of an external store.
- Every mutating call from the web app costs one extra `GET /auth/csrf` request.
- New users are `free`; changing a plan is an operator action (`db::users::set_plan`) until billing exists.
- Adding UI text means editing both catalogs; CI fails otherwise.
- `seed_e2e` creates an account with a known password and must never run against a real database.

## Amendment (2026-10-20): client IP behind the web proxy

`TRUST_PROXY_HEADERS=true` read `X-Forwarded-For`, `X-Real-IP` and `Forwarded` (tower_governor's `SmartIpKeyExtractor`). The Next.js `/backend` rewrite forwards client-sent headers verbatim, so any browser could choose its own bucket; with the flag off, every visitor shared the Next server's bucket.

Now the api reads only the single header named by `CLIENT_IP_HEADER` (for example `x-real-ip`), which a front proxy before Next must overwrite on every request. The header must hold exactly one IP; anything else falls back to the TCP peer. `X-Forwarded-For` is never read unless it is that header, and never as a list. `TRUST_PROXY_HEADERS=true` is a deprecated alias for `CLIENT_IP_HEADER=x-real-ip`. Without a header the api warns at startup that anonymous limits are shared by everyone behind the web proxy.
