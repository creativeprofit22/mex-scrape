# Bright Data KYC dossier (DRAFT)

Status: **draft, not submitted.** Submission needs a company email and is deferred to a later phase. Structure follows linkgo's Bright Data connector review.

Background: Web Unlocker works without KYC ("immediate access"), but endpoints disallowed by the target's robots.txt are blocked (HTTP 400, `X-Brd-Error-Code: ub_bad_endpoint_robots`), some targets return 403 `policy_*` errors, and the Browser API throws a `brob` error at `page.goto`. KYC approval lifts these. See <https://docs.brightdata.com/products/web-unlocker/kyc-and-access>. Current reach per retailer: [brightdata-access.md](../brightdata-access.md).

## 1. Business use case

mex-scrape is a Spanish-first web app for Mexican Amazon retail-arbitrage sellers. It compares **public product prices** on Mexican retailer sites with Amazon.com.mx prices so a seller can see which products are worth buying. It reads only pages any visitor can open without an account.

## 2. API access

- Products: Web Unlocker (retailer pages) and Datasets v3 (Amazon.com.mx fallback). Browser API is not used.
- Calls come only from the `brightdata` crate over `http-guard`, to fixed Bright Data hosts.
- Credentials (`BRIGHTDATA_API_KEY`, `BRIGHTDATA_UNLOCKER_ZONE`) live in server environment variables, are never logged or committed, and are redacted from errors.
- Refusals (`ub_bad_endpoint_robots`, `policy_*`) are recorded as `blocked_needs_kyc` and never retried. The worker also reads each retailer's robots.txt and skips disallowed paths without a request.

## 3. Permitted use

- Public product, category and search pages only: price, title, availability, image URL, product identifiers.
- No login-protected pages, no accounts, no checkout, no personal data, no CAPTCHA farming of authenticated areas.
- Low volume and polite: one request per URL, spaced at least 500 ms per zone.

## 4. Data collected and retention

- Collected: public product facts listed above, plus per-host access counters (`brightdata_access_events`: host and outcome only, no URL path, no body).
- Page bodies are parsed and discarded; raw HTML is not stored. **Retention period for parsed prices: TBD by owner** (proposal: price history kept 24 months).
- No end-user personal data is sent to Bright Data.

## 5. Target domain list

Generated from the retailer registry (`crates/retailers/src/registry.rs`):

| Retailer | Host |
| --- | --- |
| Walmart MX | `www.walmart.com.mx` |
| Liverpool | `www.liverpool.com.mx` |
| Chedraui | `www.chedraui.com.mx` |
| Bodega Aurrera | `www.bodegaaurrera.com.mx` |
| Costco MX | `www.costco.com.mx` |
| Sam's Club MX | `www.sams.com.mx` |
| Sears MX | `www.sears.com.mx` |
| El Palacio de Hierro | `www.elpalaciodehierro.com` |
| HEB MX | `www.heb.com.mx` |
| Sanborns | `www.sanborns.com.mx` |
| Waldo's | `waldos.com.mx`, `www.waldos.com.mx` |
| MercadoLibre MX | `listado.mercadolibre.com.mx`, `articulo.mercadolibre.com.mx`, `www.mercadolibre.com.mx` |

The Bright Data Browser API (`brd.superproxy.io:9222`) is used only for Sears MX category listings and Sanborns search, whose pages need JavaScript ([ADR 0007](../decisions/0007-brightdata-browser-api.md)).

Amazon.com.mx is read through Amazon SP-API first; Bright Data Datasets is the fallback only.

## 6. Caps and cost

Enforced before any request, per tenant (fail closed): global kill switch, connector switch, API key present, **20 runs per UTC day**, **50 items per run**, **500 items per UTC day**, one active run per scope. Each Unlocker page and dataset record is billed by Bright Data, so spend is bounded by 500 billable items per tenant per day. The manual `access_check --live` command is capped at 36 requests, and `capture_fixtures` at 2 pages per run.

## 7. Missing items (deferred)

| Item | Status |
| --- | --- |
| Company email for the Bright Data account | **DEFERRED** (owner) |
| Legal entity name and address | **DEFERRED** (owner) |
| Retention period confirmation | **DEFERRED** (owner) |
| Live access matrix | Done 2026-10-19: only Walmart MX search is blocked (robots.txt); see [brightdata-access.md](../brightdata-access.md) |
| KYC form submission | **DEFERRED** to a later phase |

## 8. Owner sign-off

| Role | Name | Date | Decision |
| --- | --- | --- | --- |
| Owner | | | |
| Technical review | | | |
