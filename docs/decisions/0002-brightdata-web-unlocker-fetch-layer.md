# 0002. Bright Data Web Unlocker as the store fetch layer

- Status: Accepted
- Date: 2026-10-18

## Context

Mexican retailer sites (Walmart MX, Liverpool, Chedraui, Costco MX and others) use bot protection, CAPTCHAs and geo checks. Running our own proxy pool and browser fleet is costly and fragile.

## Decision

Fetch retailer pages through the Bright Data Web Unlocker API, wrapped in the `brightdata` crate. Store adapters in `retailers` receive fetched content and only parse it; they never open connections themselves. All outbound calls go through `http-guard` (timeouts, size limits, host allow-list).

## Consequences

- Per-request cost; the worker must cache and schedule fetches carefully.
- A single place to change the fetch provider later.
- Bright Data credentials are secrets supplied by environment, never committed.
