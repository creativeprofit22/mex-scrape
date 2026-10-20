# 0013. Assistant sidecar interface: grants, private-service transport, per-tenant match decisions, token budget

- Status: Accepted
- Date: 2026-10-20

## Context

ADR 0004 put AI work in a gg-agent sidecar whose output Rust treats as untrusted, and deferred the interface to a later ADR. The assistant needs to answer sellers about their own data, prepare seasons (Hot Sale, Buen Fin), compute ROI and help resolve `needs_review` matches.

Constraints:

- gg-agent (`@kenkaiiii/gg-agent`, Node) keeps process-wide caches and builds secret redaction from `process.env`, so one process cannot be trusted to separate tenants.
- `offer_matches` is global. User data is per tenant only through `searches`/`search_results` (ADR 0012).
- The user routes need a session and CSRF, which a sidecar cannot and should not hold.
- `http-guard` allowed only public HTTPS (no plain http, private addresses or custom ports), and `api` had no outbound client.

## Decision

- **Topology.** browser → api `POST /assistant/chat` (session, CSRF, plan rate limit) → sidecar `POST /v1/chat` (service bearer) → tools → api `/internal/agent/*`. The sidecar has no database, Bright Data or SP-API access. Its egress is the api plus the model provider host. It is stateless: the browser sends the whole conversation each time ([agent.md](../modules/agent.md)).
- **Grants.** For each chat the api mints `X-Agent-Grant`: HMAC-SHA256 under `AGENT_GRANT_KEY` (≥ 32 bytes, api-only) over `{v, user_id, tenant, chat_id, exp_ms}`, valid 120 s, base64url. Internal routes need the service bearer (`AGENT_SERVICE_TOKEN`, constant-time) **and** a valid grant. The tenant and memberships come from the grant plus a reload of the user, never from tool arguments. The sidecar never sees the session cookie.
- **Transport.** `http_guard::TransportPolicy::private_service(port)` is opt-in per client: plain `http` (or `https`) to allowlisted DNS names on exactly that port. IP literals, userinfo, other ports and hosts stay refused. Loopback and private answers are allowed, but link-local/metadata, multicast, broadcast and unspecified addresses are still dropped. No redirects, no proxy, bounded bodies and redaction stay as they were. `api` may now depend on `http-guard`.
- **Untrusted output.** The api validates the sidecar answer (≤ 64 KiB body, reply cleaned and ≤ 8000 characters, `stop` in its enum, usage ≥ 0). It attaches match suggestions read from its own database by chat id, never parsed from model output.
- **Tie-breaks propose; users decide.** The `resolve_match` tool only creates a row in `match_suggestions`, and only for a listing that is a `needs_review` match to that ASIN in the tenant's own results of the last 90 days. The user confirms in the web app (`POST /matches/decisions`, session + CSRF). That upserts `match_decisions (tenant, retailer, retailer_sku)` with `decided_by_user_id` and the confirming `suggestion_id`. The decisions overlay only that tenant's reads: search results (accept → `accepted`, reject → no match) and price stats/series (global accepted listings, minus the tenant's rejects, plus its accepts). The global `offer_matches` table is never written by the assistant.
- **Tracked products** are the listings in the user's own searches of the last 90 days. The assistant reads stored results only and never starts a paid search.
- **Token budget.** Per tenant and UTC day in `agent_token_usage`, capped by plan (`AGENT_DAILY_TOKENS_FREE`/`_PRO`). The api refuses at the cap (429 `agent_budget`) and sends the remainder. The sidecar reports each turn to `/internal/agent/usage` (atomic add, authoritative remainder) and stops before the next model call once the budget is spent. If the report fails, the run fails closed. The sidecar allows one chat in flight per tenant (409 `agent_busy`), so overshoot is at most one turn.
- **Limits in the sidecar.** `maxTurns` 6 (no extensions), tool timeout 10 s (also aborting the tool's HTTP call), run timeout 60 s, 1024 output tokens per turn, capped tool results, web search and compaction off, and one `AbortSignal` tied to the client disconnecting.
- **Testing.** The sidecar is tested with gg-ai's fake `palsu` provider and a fake api, including a headless check with an empty home directory. The web e2e runs a scripted fake model that is enabled only with `AGENT_ALLOW_FAKE_PROVIDER=true` and refused in production.

## Consequences

- Three new secrets: `AGENT_SERVICE_TOKEN` (api + sidecar), `AGENT_GRANT_KEY` (api only) and the provider key (sidecar only). Without the api ones the assistant answers 503.
- One more process (Node 22) to deploy and monitor, with its own CI job and lockfile. The sidecar's api types are generated from `web/openapi.json`, so API changes need `npm --prefix services/agent run gen:api` too.
- `SearchResults`, `PriceStats` and `PriceSeries` take the tenant and a `MatchDecisionRepo`, so two users can see the same listing matched differently.
- The `http-guard` rule "public HTTPS only" now has one documented, opt-in exception, used only by the api's sidecar client.
- A grant leaked from the sidecar lets its holder read that one user's tracked data and create suggestions for up to 120 s, and only together with the service token. It cannot decide matches.
