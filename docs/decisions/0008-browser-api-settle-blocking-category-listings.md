# 0008. Browser API: settle, download blocking and category listings

- Status: Accepted
- Date: 2026-10-19
- Amends: [0007](0007-brightdata-browser-api.md)

## Context

[ADR 0007](0007-brightdata-browser-api.md) read a page's HTML as soon as `Page.loadEventFired` arrived, let the browser download everything, and routed Sears MX category listings and Sanborns search to the Browser API. The first live captures on 2026-10-19 showed three problems:

- **Cost.** The Browser API bills per byte (about $8/GB on pay-as-you-go). Images, fonts, video and stylesheets make up most of a store page's bytes, and the parsers need none of them.
- **Timing.** Claro Shop pages (Sears MX, Sanborns) fetch their product list with a request made after `load`. Reading the HTML at `load` can return an empty grid.
- **Evidence.**
  - The Sears MX listing that 0007 cited (`/categoria/4714/licuadoras-y-picadoras/`) loads a mismatched category path and lists 0 products even in a real browser. Only the full menu link (`/cat/<path>?id=<id>`) lists products.
  - Sanborns search (`/resultados/*`) is disallowed by the site's robots.txt, and Bright Data refuses it with `blocked_needs_kyc (BrowserRobots)` until KYC approval.

## Decision

- **Download blocking.** After `Page.enable`, the client sends `Network.enable`, then `Network.setBlockedURLs` with `BLOCKED_URL_PATTERNS`. That list covers image, font, media and stylesheet extensions plus common ad and analytics hosts, following Bright Data's bandwidth guidance. Scripts and data requests stay allowed, because they build the product list. Blocking is on by default (`BrightDataConfig::browser_block_downloads`). `capture_fixtures --browser --no-block` turns it off for diagnosis only.
- **Settle.** After `load`, the client checks the page every `poll_interval` (500 ms). It counts as finished after `quiet_polls` (3) consecutive checks with no request of its session in flight and an unchanged element count (`document.getElementsByTagName('*').length`). After `max_polls` (60, about 30 s) it reads the HTML as it is. A page that never goes quiet is not an error. The 120 s `browser_timeout` still caps the whole fetch. Configured in `BrowserSettle`.
- **Category listings, not search, for Claro Shop stores.** Sears MX and Sanborns list products through category pages linked from the site's own menu (`/cat/<path>?id=<id>`). Both have `browser_pages = [Category]`; search stays on Unlocker, so no browser time is spent on a page Bright Data refuses. The rendered product cards are read by `claro::TILES`.
- **Evidence, re-checked with the correct links on 2026-10-19.**
  - The Unlocker HTML of both category pages has the category in `__NEXT_DATA__` but no product data. These are fixtures `search_unlocker.html`, tested to return `NoOffers`.
  - The Browser API HTML has 100 product cards per page: Sears showed 479 results, Sanborns 499. These are fixtures `search.html`.
- **Fixture redaction** also scrubs JSON stored inside JSON strings (Claro Shop's `runtimeConfig.POWREVIEWS`), and the `algol*` keys (Claro Shop's Algolia app id and search key). Both carry public client keys that should not be committed.

## Consequences

- Each browser page costs mostly its HTML and scripts. A rendered Claro Shop listing is about 0.5–1 MB of HTML, but the billed bytes per page have not been measured yet.
- A browser fetch takes longer: at least about 2 s of settle checks after `load`, and up to about 30 s for a page that keeps changing. Captures took 15–60 s.
- Sears MX and Sanborns cannot be searched by keyword. The caller has to supply category links, because the `Retailer` trait builds only search and product URLs.
- Offers from rendered cards have no brand (the card label is the seller); the product page supplies it.
- A blocked pattern could break a page that needs a stylesheet or font to render. `--no-block` exists to check that.
