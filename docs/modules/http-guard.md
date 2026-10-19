# http-guard

## Purpose

The one outbound HTTP client and the server-secret types for the whole backend. Ported from linkgo `net/transport.rs` to async reqwest.

Every `GuardedClient` enforces:

- **HTTPS only.** `http://` and other schemes are refused before any I/O.
- **Per-client host allowlist.** Exact DNS names only; IP literals, other hosts, userinfo (`user:pw@`) and non-default ports are refused before any I/O.
- **No redirects.** 3xx responses are returned as-is. reqwest strips only a fixed set of auth headers on cross-host redirects, so `x-api-key` style headers and replayed 307/308 bodies would otherwise follow.
- **No proxy.** Environment proxies are ignored; a proxy would resolve names itself and bypass the DNS guard.
- **Timeouts.** Connect 10 s and total 120 s by default (`connect_timeout`, `total_timeout`).
- **Public-only DNS.** The resolver answers only for allowlisted names and drops loopback, private, link-local (including `169.254.169.254`), CGNAT, reserved and IPv6-embedded private addresses. If nothing public remains, the request fails (DNS-rebinding guard).
- **Bounded bodies.** Bodies can only be read through `bytes`/`text`/`json(max_bytes)`. A declared `Content-Length` over the cap is refused before reading, and streaming stops once the cap is passed.
- **Safe errors.** `GuardError` never wraps a raw `reqwest::Error`. Messages drop the URL (query strings), include the source chain, have every configured secret (the client's redactor plus every secret sent on that request) replaced with `[redacted]`, and are then truncated to 500 characters (redacting first, so truncation never leaves half a secret).

Secrets (defined in the [`secrets`](secrets.md) crate and re-exported here):

- `Secret` has no `Debug`, `Display`, `Serialize`, `Default` or sqlx `Encode`, so it cannot be logged, returned in an API response or bound into a DB row (enforced by `compile_fail` doctests).
- Secrets are loaded from the environment (or a secret store via `Secret::new`) at start-up. An absent variable is `None`; an empty, whitespace-only, control-character or non-UTF-8 value is an error that names the variable, never the value.
- `Redactor` removes known secret values from any string.

## Public API

- `GuardedClient::builder(HostAllowlist) -> GuardedClientBuilder`, with `.policy()`, `.connect_timeout()`, `.total_timeout()`, `.redactor()` and `.build()`.
- `GuardedClient::request(Method, &Url)`, `get`, `post`: validate the URL and return a `GuardedRequest`. `sanitize(&str)` redacts and truncates provider error text. `redactor()`.
- `GuardedRequest`:
  - `header(name, value)`: plain headers; refuses `Host`.
  - `secret_header(name, &Secret)`, `bearer(&Secret)`: values are marked sensitive and added to the request's redactor, so they are redacted from its errors even if the client's redactor lacks them.
  - `json(&T)`, `body(bytes)`, `send().await`.
- `GuardedResponse`: `status()`, `headers()`, the bounded readers `bytes(max)`, `text(max)` and `json::<T>(max)`, `error_text(max)` (bounded, redacted, truncated provider error body) and `sanitize(&str)`.
- `HostAllowlist::new([...])`, `contains`, `iter` (sorted).
- `TransportPolicy::PUBLIC_HTTPS`. `TransportPolicy::LOOPBACK_TEST` (test-only: allows plain http and any port, accepts loopback IP literals in addition to allowlisted names, and disables the public-only DNS resolver, so allowlisted names may resolve to private addresses) exists only with the `test-support` feature.
- `GuardError`, with `is_timeout()` and `is_transient()` for retry decisions.
- `Secret` (`new`, `from_lookup`, `from_env`, `expose_secret`), `SecretError`, `Redactor` (`new`, `with`, `redact`, `sanitize`), `truncate_error_message`.
- `is_non_public_ip`, `is_non_public_ipv4`, `is_non_public_ipv6`.
- Constants: `DEFAULT_MAX_RESPONSE_BYTES` (8 MiB), `SMALL_RESPONSE_BYTES` (256 KiB, for tokens and error bodies), `MAX_ERROR_MESSAGE_CHARS` (500), `DEFAULT_CONNECT_TIMEOUT`, `DEFAULT_TOTAL_TIMEOUT`.
- `GuardedWebSocket` ([ADR 0007](../decisions/0007-brightdata-browser-api.md)), the only WebSocket client, used for the Bright Data Browser API. `GuardedWebSocket::builder(HostAllowlist, port)` takes `.policy()`, `.connect_timeout()` (DNS + TCP + TLS + handshake), `.max_message_bytes()` (default 8 MiB) and `.redactor()`. `.connect(&Url, basic_auth: Option<(&Secret, &Secret)>)` allows `wss` only, an allowlisted host, the one configured port and no userinfo. It sends credentials only as a sensitive `Authorization: Basic` header (both parts are redacted from errors), uses no proxy and accepts only public DNS answers. The socket provides `send_text`, `recv_text` (text frames only; larger messages fail with `BodyTooLarge`) and `close`. Errors are `GuardError`s that never contain the URL. With `test-support`, `test_support::serve(script)` starts a loopback fake WebSocket server (`FakePeer::recv`/`send`, `headers()`, `finished()`).
- Re-exports `Method`, `StatusCode`, `Url` and `header`, so callers never need reqwest themselves. Build query strings with `Url::query_pairs_mut`.

### How other crates must use it

- Never depend on `reqwest`, `tungstenite`/`tokio-tungstenite` (or any other HTTP or WebSocket client) or build a client yourself. `scripts/check_docs.py` fails CI if a crate other than `http-guard` depends on one.
- Build one `GuardedClient` per provider, with only that provider's hosts:
  - Bright Data: `api.brightdata.com`; Browser API WebSocket: `brd.superproxy.io`, port 9222 only.
  - SP-API: `sellingpartnerapi-na.amazon.com`, plus `api.amazon.com` for LWA tokens.
- Give the client a `Redactor` over every secret it sends.
- Send secrets only through `secret_header` or `bearer`, never in URLs.
- Read every body with an explicit cap: `SMALL_RESPONSE_BYTES` for tokens and error bodies, `DEFAULT_MAX_RESPONSE_BYTES` or smaller otherwise.
- Turn non-2xx bodies into messages with `GuardedResponse::error_text(SMALL_RESPONSE_BYTES)`; any other provider text goes through `GuardedResponse::sanitize` or `GuardedClient::sanitize` before it reaches an error, log line or API response. Never format a raw `text()` body into an error.
- Enable `test-support` only from `[dev-dependencies]`, to point tests at loopback fake servers.

### Runtime requirement

TLS uses rustls with the platform verifier, so production images need the OS CA bundle (e.g. the `ca-certificates` package on Debian/Ubuntu).

## Dependencies

- Workspace: `secrets` (re-exported).
- External: `reqwest` (rustls, http2; no default features), `tokio` (`net`), `serde`, `serde_json`, `thiserror`; for WebSockets `tokio-tungstenite` (`handshake` only), `tokio-rustls` (aws-lc-rs, TLS 1.2+1.3), `rustls-platform-verifier`, `futures-util` (`sink`) and `base64`.

## Owned tables

None.
