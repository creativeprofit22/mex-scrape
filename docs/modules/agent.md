# agent (gg-agent sidecar)

## Purpose

`services/agent` is the AI assistant's engine: a small Node 22 service that runs [gg-agent](https://github.com/KenKaiii/gg-framework) (`@kenkaiiii/gg-agent`, MIT) headless. It answers sellers in Spanish (English on request) about their tracked products, helps prepare seasons such as Hot Sale and Buen Fin, computes ROI, and proposes tie-breaks for `needs_review` matches. It is pure orchestration: every tool is an HTTP call into the Rust api, which stays the system of record and enforces auth, tenancy and caps ([ADR 0004](../decisions/0004-gg-agent-sidecar-for-ai.md), [ADR 0013](../decisions/0013-assistant-sidecar-interface.md)).

## Boundary

- The browser never talks to the sidecar. The api's `POST /assistant/chat` authenticates the user (session, CSRF, plan rate limit), checks the daily token budget and calls `POST /v1/chat` on the sidecar through `http-guard` (`TransportPolicy::private_service`, [http-guard.md](http-guard.md)).
- Egress: only the Rust api (`AGENT_API_URL`) and the model provider host. The provider SDK gets an allowlisting `fetch` (`src/egress.ts`: HTTPS, default port, `api.anthropic.com` or `api.openai.com`, no redirects); the fake provider gets none.
- No database, Bright Data or SP-API access, and no credentials for them. The sidecar holds only the service token and the provider key.
- **Not a tenant boundary.** gg-agent keeps some process-wide caches and builds secret redaction from `process.env`, so it runs as its own process, and tenancy is enforced by the api: each chat carries a grant the api minted, and every tool route derives the tenant from that grant. Tool arguments are strict zod objects (extra keys rejected), so a model cannot pass a tenant or user id.
- The sidecar never sees the session cookie, and it can write only match *suggestions*; decisions need the user's click in the web app (`POST /matches/decisions`).

## HTTP contract

`POST /v1/chat` with `Authorization: Bearer <AGENT_SERVICE_TOKEN>` (SHA-256 then constant-time compare; else 401) and a JSON body (≤ 64 KiB, else 413):

```json
{"chat_id": "…", "tenant": "user-4", "locale": "es", "grant": "…", "token_budget": 48000,
 "messages": [{"role": "user", "content": "…"}]}
```

1–20 messages of 1–2000 characters, the last from the user (else 422). `tenant` is used only for the one-chat-per-tenant lock; tools never send it. Answers:

- 200 `{"reply", "stop": "done"|"max_turns"|"token_budget", "usage": {"input_tokens", "output_tokens"}}`
- 409 `agent_busy` (the tenant already has a chat running), 502 `agent_failed`, 504 `agent_timeout`.
- When the caller disconnects, the run is aborted and nothing is sent.

`GET /health` → `{"status":"ok"}`. Logs are JSON lines with fixed messages (`listening`, `chat_done` with stop/ms/token counts, `chat_busy`, `chat_aborted`, `chat_timeout`, `chat_failed`, `unhandled_rejection`). They never include message text, replies, tool data, provider errors, tokens or keys.

The api validates the reply again (length, control characters, stop value, usage) and attaches suggestions from its own database, so model output never becomes suggestion data ([api.md](api.md)).

## Tools

Defined in `src/tools.ts` (zod strict schemas). Each one calls the api with `Authorization: Bearer <AGENT_SERVICE_TOKEN>` and `X-Agent-Grant: <grant>` (`src/api-client.ts`, types generated from `web/openapi.json` into `src/api/schema.ts`, responses ≤ 256 KiB, no redirects):

| tool | arguments | api call |
|---|---|---|
| `search_offers` | `query?` (≤ 100), `match_status?` (`accepted`/`needs_review`), `limit` 1–20 | `GET /internal/agent/offers`: the seller's tracked products (results of their searches in the last 90 days), read-only, never starts a search |
| `get_price_history` | `asin`, `days` 1–365 | `GET /internal/agent/products/{asin}/price-history`: daily prices plus 30/90/365-day low/high/average per listing; the stats are sent first, and when the result would pass the tool-result cap the history keeps only the newest points (`history_trimmed: true`) |
| `compute_roi` | `items` 1–20 (centavos) | `POST /internal/agent/roi` |
| `resolve_match` | `retailer`, `retailer_sku`, `asin`, `decision` (`accept`/`reject`), `reason` ≤ 300 | `POST /internal/agent/match-suggestions`: stores a suggestion only for a `needs_review` match in the tenant's results (sequential execution) |

An api error reaches the model only as `api_error: <code>` or `api_unreachable`. After each model turn the sidecar posts the turn's tokens to `POST /internal/agent/usage`.

The system prompt (`src/prompt.ts`) is Spanish-first and carries a typical Mexican season calendar (Reyes, San Valentín, Día de las Madres, Hot Sale, Regreso a clases, El Buen Fin, Navidad). It tells the model to use only tool data, to treat store text as untrusted, to convert centavos to pesos, to suggest a new search when data is missing, and never to claim a match is resolved.

## Limits

| limit | variable | default |
|---|---|---|
| model turns per chat (`maxTurns`, no extensions) | `AGENT_MAX_TURNS` (1–20) | 6 |
| per-tool timeout (`timeoutMs`, also aborts its HTTP call) | `AGENT_TOOL_TIMEOUT_MS` | 10 000 |
| whole run | `AGENT_RUN_TIMEOUT_MS` (≤ 70 000) | 60 000 |
| output tokens per turn (`maxTokens`) | `AGENT_MAX_OUTPUT_TOKENS` | 1024 |
| characters of one tool result the model sees | `AGENT_MAX_TOOL_RESULT_CHARS` | 16 000 |
| chats in flight per tenant | — | 1 |

- One `AbortController` per run is passed to gg-agent as `signal`. It aborts when the HTTP response closes before it is sent (the api or browser went away: the api drops its request when the browser disconnects), on the run timeout, and on budget exhaustion. Each tool call uses `AbortSignal.any([toolSignal, AbortSignal.timeout(toolTimeout)])`.
- `webSearch`, compaction and tool-use clearing are off; no server tools.
- The api's own sidecar timeout is 75 s, and it reads at most 64 KiB of answer.

### Daily token budget

1. The api refuses a chat with 429 `agent_budget` once the tenant's tokens for the UTC day reach the plan cap (`AGENT_DAILY_TOKENS_FREE` 50 000, `AGENT_DAILY_TOKENS_PRO` 500 000). Otherwise it sends the remainder as `token_budget`.
2. After each turn the sidecar adds that turn's tokens locally and posts them to `/internal/agent/usage`, which adds them atomically and returns the authoritative `remaining_tokens`.
3. Before every model call (gg-agent `transformContext`) the sidecar waits for that report and stops the run with `stop: "token_budget"` when the local count reaches `token_budget` or the api reports 0. If the usage report fails, the run fails closed (502).
4. A final answer that crosses the budget is kept. Overshoot is at most one turn per tenant (one chat in flight).

## Model configuration

| variable | meaning |
|---|---|
| `AGENT_ADDR` | listen address, default `127.0.0.1:8090` |
| `AGENT_SERVICE_TOKEN` | shared with the api, ≥ 32 characters (required) |
| `AGENT_API_URL` | the Rust api base URL, `http(s)`, no credentials (required) |
| `AGENT_PROVIDER` | `anthropic`, `openai`, or `palsu` (the fake) |
| `AGENT_MODEL` | model id (required for real providers) |
| `AGENT_PROVIDER_API_KEY` | provider key (required for real providers) |
| `AGENT_ALLOW_FAKE_PROVIDER` | must be `true` for `palsu`; refused when `NODE_ENV=production` |

Config parsing (`src/config.ts`) fails closed. A missing or malformed variable stops the process, and the error names the variable, never the value. `GGAI_DUMP_REQUEST` (gg-ai's request dump to `/tmp`) is refused.

**Secrets.** The provider key is held only in a closure (`config.apiKey()`), so it is never serialized. It goes to gg-agent as `apiKey` and is never logged: provider errors are reduced to `agent_failed`, logs carry fixed messages, and a process-level `unhandledRejection` handler logs only `unhandled_rejection`. Tests check that the key and the service token do not appear in stdout, stderr, logs or responses, even when the provider error echoes the key. The api never holds the provider key.

## Headless operation

- Only `@kenkaiiii/gg-agent` and `@kenkaiiii/gg-ai` are used, both pinned to the exact same version (5.84.2) with `zod` 4.6.7 in `package-lock.json`. gg-core, ggcoder and the CLI/TUI are not dependencies.
- Inspection of the 5.84.2 dist bundles: neither package calls `os.homedir` or touches `~/.gg`. gg-ai imports `os` only for `arch`/`platform`/`release` (user agent). Its only file write is the `GGAI_DUMP_REQUEST` debug dump, which config refuses. gg-agent imports only gg-ai and zod.
- `test/headless.test.ts` starts the real entry point (`src/main.ts`) as a child process with `HOME`, `USERPROFILE`, `APPDATA`, `LOCALAPPDATA` and `XDG_CONFIG_HOME` pointing into an empty temp directory. It runs two chats and then checks that the directory is still empty, that the only `@kenkaiiii` packages loaded are gg-agent and gg-ai, that no CLI/TUI module (ink, react, commander, yargs, inquirer, ggcoder, gg-core) was loaded (traced with a `module.registerHooks` resolve hook), and that stdout is JSON lines without secrets.

## Running

- `npm --prefix services/agent ci`, then `npm --prefix services/agent start` (builds with `tsc` into `dist/` and runs `node dist/main.js`).
- The api needs `AGENT_URL` (a host name such as `http://localhost:8090` or `http://agent:8090`; IP literals are refused), `AGENT_SERVICE_TOKEN` and `AGENT_GRANT_KEY`. Without them the assistant answers 503 `agent_unavailable`.
- The fake model for local runs and e2e: `AGENT_PROVIDER=palsu AGENT_ALLOW_FAKE_PROVIDER=true`. The scripted responses (`src/fake-provider.ts`) answer an ASIN question with the 365-day low from `get_price_history`, and a tie-break request by calling `search_offers` then `resolve_match` on the first `needs_review` listing.

## Tests

`npm --prefix services/agent test` (vitest; gg-ai's `palsu` fake provider and an in-process fake api; no network):

- `test/run.test.ts`: each tool's HTTP call (method, path, query/body, bearer, grant); tool args cannot carry a tenant; api error codes; `maxTurns`; tool timeout aborts its HTTP call; client abort cancels the in-flight tool call; run timeout; token budget (local count, api-reported 0, final answer kept, usage report failure fails closed); a provider error echoing the key becomes `RunFailed` without the key.
- `test/server.test.ts`: 401 without or with a wrong service token, 422/413 bodies, a chat, 409 busy per tenant (other tenants unaffected), disconnect aborts the run and the api call, the key never appears in responses, stdout, stderr, console or logs, and the unhandled-rejection guard logs a fixed message.
- `test/config.test.ts`: defaults, fail-closed parsing, fake-provider opt-in, egress allowlist.
- `test/headless.test.ts`: see above. It also runs the season-prep and tie-break flows end to end with the scripted model.
- CI job `agent`: `npm ci`, `typecheck`, `test`, and `gen:api` must leave `src/api/schema.ts` unchanged. The web e2e (`web/e2e/assistant.spec.ts`) runs the sidecar with the fake model against the real api.

## Dependencies

- npm: `@kenkaiiii/gg-agent` 5.84.2, `@kenkaiiii/gg-ai` 5.84.2, `zod` 4.6.7; dev: `typescript` 5.9.3, `vitest` 5.0.5, `openapi-typescript` 7.13.0, `@types/node` 22.20.5. Node `>=22.12 <23` (`.nvmrc`: 22).
- Backend: the api's `/internal/agent/*` routes over HTTP only.

## Owned tables

None. Usage, suggestions and decisions live in the api's database (`agent_token_usage`, `match_suggestions`, `match_decisions`; [db.md](db.md)).
