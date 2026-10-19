# 0007. Bright Data Browser API over a guarded WebSocket

- Status: Accepted (amended by [0008](0008-browser-api-settle-blocking-category-listings.md))
- Date: 2026-10-19
- Extends: [0002](0002-brightdata-web-unlocker-fetch-layer.md), [0006](0006-brightdata-agnostic-client-durable-runs.md)

## Context

Web Unlocker returns the server HTML of a page. Some store pages build their product list in the browser. The Unlocker HTML of a Sears MX category listing has no product JSON-LD, page JSON or product tiles, and the Sanborns search page comes back as a 252-byte stub. Those pages need a real browser. Bright Data's Browser API is a remote Chrome reached over the Chrome DevTools Protocol (CDP) on `wss://brd.superproxy.io:9222`. That is a new outbound channel: a WebSocket on a non-default port, authenticated with zone credentials. Until now every outbound call went through `http_guard::GuardedClient` (https only, default port).

## Decision

- `http-guard` gains `GuardedWebSocket`, the only WebSocket client in the workspace (`tokio-tungstenite` with `handshake` only, TLS via `tokio-rustls` with the platform verifier and aws-lc-rs). Its rules match the HTTP client's: `wss` only, exact allowlisted host, **one explicitly allowed port per connector** (9222 for `brd.superproxy.io` only), no userinfo in URLs, credentials only in a sensitive `Authorization: Basic` header, public-only DNS answers, a connect timeout, a per-message byte cap and redacted, URL-free errors. `scripts/check_docs.py` forbids `tungstenite`/`tokio-tungstenite` in any other crate.
- `brightdata` looks up credentials the way Bright Data's MCP server does (`browser_tools.js`, `calculate_cdp_endpoint`): `GET /status` for the customer id and `GET /zone/passwords?zone=…` for the password, through the existing guarded API client. The user is `brd-customer-<c>-zone-<z>-country-<cc>`.
- One fetch is a minimal CDP exchange: `Target.createTarget` → `Target.attachToTarget {flatten}` → `Page.enable` → `Page.navigate` → wait for `Page.loadEventFired` → `Runtime.evaluate(outerHTML)` → `Target.closeTarget`, within 120 s. The HTML is capped at 8 MiB. A `brob` navigation error is `BlockedNeedsKyc` (robots.txt).
- `Transport::browse` sits beside `unlock` (the default implementation fails "not configured"). `Runner::start_unlocker_run` takes `PageAccess { Unlocker, Browser }`, so gates, caps, the robots.txt pre-check and access events apply to both unchanged.
- The Browser API zone is the optional secret `BRIGHTDATA_BROWSER_ZONE`. Without it, Browser fetches fail closed.
- An adapter uses the Browser API only when its Unlocker HTML is proven to lack product data (no JSON-LD product, no page JSON with a price, no CSS price). Its doc under `docs/modules/retailers/` records the evidence.

## Consequences

- One more dependency family (`tokio-tungstenite`, `tokio-rustls`, `rustls-platform-verifier`, `base64`, `futures-util`), confined to `http-guard`.
- Browser sessions are slower and cost more than Unlocker requests, so they are rate limited separately (`RateKey::Browser`, 1 s) and used only where proven necessary.
- Rendered pages carry no HTTP status; `UnlockedPage.status` is 200 for a completed load.
- The account needs a Browser API zone before any adapter can use this path.
