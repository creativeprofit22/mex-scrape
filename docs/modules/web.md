# web

## Purpose

Next.js 16 (App Router, TypeScript) front end in `web/`. Spanish by default, English on request ([ADR 0012](../decisions/0012-session-auth-csrf-rate-limits-i18n.md)). Talks to the backend only through the typed client generated from the `api` OpenAPI spec. Styling is plain layered CSS with one token set and Archivo ([ADR 0014](../decisions/0014-web-design-system-and-app-shell.md)).

## Sub-docs

| Doc | Covers |
| --- | --- |
| [design-system](web/design-system.md) | tokens (light and dark), type roles, components and classes, CSS file ownership and layer order, adding a component stylesheet |
| [app-shell](web/app-shell.md) | route groups, the app and public frames, session states and the placeholder rule, current-page rule, breakpoints, scroll model, safe areas, account page |
| [search](web/search.md) | the search workspace: SearchPanel as orchestrator, its components and pure helpers, sort/filter/selection contract, browser storage keys and validation, the `?q=` rerun, every state, the per-store progress gap |
| [detail-panel](web/detail-panel.md) | the selected result's panel (inline aside ≥1200px, native `<dialog>` sheet below), Confirmar / Descartar decisions, profit breakdown, match meter, panel states; the shared price chart: scale, date and band helpers, colours, accessibility |

## Public API

Every page lives under `src/app/[lang]/` (`lang` is `es` or `en`), in the `(public)` or `(app)` route group ([app-shell](web/app-shell.md)):

- `/[lang]`: home with an API status indicator (`GET /health`).
- `/[lang]/signup`, `/[lang]/signin`: `AuthForm` (`POST /auth/signup`, `POST /auth/signin`). Afterwards it goes to `onboarding` while `onboarded` is false, else to `search`. Error codes from the API are shown in the page language. The link to the other form keeps the validated `?next=`.
- `/[lang]/onboarding`: `OnboardingForm`, Costco and Sam's Club membership checkboxes (`GET /me`, `PUT /me/onboarding`).
- `/[lang]/search`: `SearchPanel`. On mount it reads `GET /me` (`useRequireOnboarded` in `src/lib/`): a 401 goes to sign-in and `onboarded=false` goes to `/[lang]/onboarding`, so membership offers are never hidden from a user who was never asked; until `/me` answers only a `role="status"` loading line shows. The main workspace ([search](web/search.md)): a toolbar with the query, a costs pill (prep and shipping, saved in the browser) and Buscar; `POST /searches`, then polls `GET /searches/{id}` every 1.5 s, for up to 180 s, with a status line and per-store outcome chips. Then KPI tiles (best profit, offers, stores, hidden by membership), filter chips and the results sorted by profit (highest first; price, profit and ROI headers re-sort), as a table when the results column is at least 53rem wide and labelled cards otherwise. Each row has the store, price, availability, the Amazon match or a "needs review" tag, a store link (`target="_blank" rel="noopener noreferrer"`) and a title button that selects it. The selected result opens the [detail panel](web/detail-panel.md): inline beside the results at ≥1200px, a native `<dialog>` side or bottom sheet below. It shows the match confidence, Confirmar / Descartar (`POST /matches/decisions`, accept or reject), the profit breakdown and a 90-day price chart. Profit and ROI come from `POST /roi` batches of up to 100 over the matched results plus the saved costs. Recent searches (browser only) show in the sidebar and on the phone search view; one reruns through `?q=`. Every state, including timeout and a network drop, has a next action.
- `/[lang]/products/[asin]`: `PriceHistory`, gated on onboarding like the search page. The ASIN must match `^[A-Z0-9]{10}$`. The shared price chart ([detail-panel](web/detail-panel.md#chart): 1-2-5 price ticks, date ticks, the good-price band, token colours plus dashes, an `aria-label` summary) of `GET /products/{asin}/price-history`, a `<details>` data table as the accessible fallback, and the 30/90/365-day low/high/average table from `GET /products/{asin}/price-stats`. Listings from membership stores the user lacks are left out by the API; a note shows `hidden_listings` when it is above zero.
- `/[lang]/calculator`: `Calculator`, a cost form that calls the same `POST /roi`, so its numbers match the search page. Prep and shipping start from the costs saved on the search page; a draft kept across sign-in wins. Not gated on onboarding (it needs no memberships).
- `/[lang]/assistant`: `AssistantChat`, gated on onboarding like the search page ([agent.md](agent.md), [ADR 0013](../decisions/0013-assistant-sidecar-interface.md)). A message list (`aria-live="polite"`), a labelled textarea (Enter sends, Shift+Enter is a new line) and a send button; the whole conversation (last 20 turns) goes to `POST /assistant/chat` and the reply is shown as plain text. A `role="status"` line shows while the assistant thinks; API error codes (`agent_budget`, `agent_busy`, `invalid_messages`, `agent_failed`, `agent_timeout`, `agent_unavailable`) map to messages. Match proposals from the reply's `suggestions` become cards: **Confirmar** calls `POST /matches/decisions` with the suggestion id (CSRF via the client middleware), **Descartar** only hides the card and writes nothing. Nothing is stored in the browser; "Nueva conversación" clears it.
- `/[lang]/account`: `AccountPanel` (the phone Cuenta tab): email, plan, language and sign-out when signed in; sign-in and sign-up links when signed out. Reads the frame's session (`GET /me`).
- Frame ([app-shell](web/app-shell.md)): signed-in pages get a sidebar (≥768px) or top bar plus bottom tabs, with only the workspace scrolling; home, sign-in and sign-up get a minimal header. Session links come from one `GET /me` and show only once it answers. The frame holds `LanguageToggle` (English / Español; keeps the current path, sets the `lang` cookie and calls `PUT /me/locale` when signed in) and `SignOutButton` (`POST /auth/signout`). The current page gets `aria-current="page"`.
- Any 401 from the API sends the user to `/[lang]/signin`.

### i18n rules

- `src/proxy.ts` (Next 16's renamed middleware) redirects paths without a language segment to the `lang` cookie's language, else `es`. Sign-in sets the cookie from the account's saved locale.
- `<html lang>` follows the route. Money uses `Intl.NumberFormat` with `es-MX` or `en-US` and currency MXN (`src/i18n/format.ts`).
- All UI text lives in `src/i18n/messages/es.json` and `en.json`, loaded by `getDictionary(lang)` (`src/i18n/dictionaries.ts`). `es.json` defines the `Dictionary` type.
- ESLint `react/jsx-no-literals` (`noStrings`, props ignored, only `·` and `/` allowed) rejects text written in JSX.
- `scripts/check-messages.mjs` (part of `npm run lint`) fails when the two catalogs have different keys or any empty value. Add every new key to both files.

### API client

- `src/lib/api/client.ts`: `openapi-fetch` client, base URL `/backend` (proxied to `API_URL` by the `next.config.ts` rewrite), `credentials: "same-origin"`. A middleware fetches `GET /auth/csrf` and sets `X-CSRF-Token` on every non-GET request. `next build` writes the rewrite destination into `.next/routes-manifest.json`, so production uses the build-time `API_URL`; setting it only for `next start` has no effect.
- Client IPs: the `/backend` rewrite forwards the browser's headers verbatim (including any `X-Forwarded-For` or `X-Real-IP` it sends) and sets none, and the api sees the Next server as the peer. For per-IP limits on sign-up and sign-in, put a front proxy (nginx/Caddy) before Next that overwrites one header with the real client IP (for example nginx `proxy_set_header X-Real-IP $remote_addr;`) and set the api's `CLIENT_IP_HEADER` to that header ([api.md](api.md#rate-limits-module-ratelimit)). Otherwise all visitors share one anonymous bucket.
- `src/lib/api/schema.ts` and `openapi.json` are generated. Regenerate with `npm --prefix web run gen:api` and commit both; CI fails if they drift from the Rust code.

### End-to-end tests (`web/e2e`)

- `npm --prefix web run e2e` (Playwright, Chromium, one worker). `playwright.config.ts` starts the api (`cargo run --locked -p api`, with high rate limits and `AGENT_*` pointing at the sidecar), the assistant sidecar (`npm run start` in `services/agent` with `AGENT_PROVIDER=palsu`, `AGENT_ALLOW_FAKE_PROVIDER=true` and fixed e2e-only tokens; install it with `npm --prefix services/agent ci`) and `npm run start`, so run `npm --prefix web run build` first. Locally it reuses servers that are already running.
- Needs Postgres with migrations and the seed: `cargo run -p db --bin seed_e2e -- seed` (see [db.md](db.md)). `DATABASE_URL` defaults to the compose database.
- `journey.spec.ts` (1600×900, so the results table fits beside the detail panel), for both `es` and `en`: sign in → search → product page using only the keyboard (Tab/Enter), checking the hidden Sam's result, the default profit-first order, the match badge and profit; a not-onboarded account (`E2E_PENDING_EMAIL`, seeded with `onboarded=false`) sent from the search and product pages to onboarding; and the calculator matching the search ROI. Every page gets an axe scan (WCAG 2.1 A/AA) that must report zero violations.
- `assistant.spec.ts` (1600×900): with the sidecar's scripted fake model, a season-prep question about `B0E2ETEST1` must answer with the 365-day low read from `GET /products/{asin}/price-stats` (the same data the `get_price_history` tool returns); a tie-break request yields a proposal for the seeded Costco `needs_review` listing, which the user confirms, after which a new search shows it accepted. axe runs on `/es/assistant` and `/en/assistant`. `seed_e2e reset-assistant` runs before and after, so other specs and later runs see the `needs_review` fixture.
- `search.spec.ts` fakes `/backend/**` with `page.route` (no backend) and covers the search workspace: sorting, filters, selection, saved costs and recent searches, malformed storage, every state, retry after a network drop, a new search mid-poll, browser back and phone cards, plus the detail panel (inline at 1440, side sheet at 1024, bottom sheet at 390: decisions, panel states, Esc/×/backdrop, focus return), with axe; `search-helpers.spec.ts` and `chart-helpers.spec.ts` test the pure helpers in Node ([search](web/search.md#tests), [detail-panel](web/detail-panel.md#tests)).
- `shell.spec.ts` and `recovery.spec.ts` fake `/backend/**` with `page.route`, so they need no backend: the frame's session links, current page, skip link, Tab order, phone layout, language switch and the journey (including a failed sign-in retried) at 1440×900 and 390×844; recovery covers retries, `?next=` after sign-in and the sign-in ↔ sign-up switch keeping a safe `?next=`.
- Searches are completed by `seed_e2e complete` (no worker or Bright Data). `E2E_SEED_BIN` points at a prebuilt binary; otherwise the tests use `cargo run`. `E2E_EMAIL`, `E2E_PENDING_EMAIL` and `E2E_PASSWORD` override the seeded accounts.
- CI job `e2e` runs migrate, seed, build and the tests, and uploads the Playwright report on failure.

## Dependencies

- Backend: `api` over HTTP only.
- Font: Archivo via `next/font/google` (downloaded at build time).
- npm: `next`, `react`, `openapi-fetch`; dev: `openapi-typescript`, `eslint`, `typescript`, `@playwright/test` (1.64.1), `@axe-core/playwright` (4.14.0). All pinned in `package-lock.json`.

## Owned tables

None.
