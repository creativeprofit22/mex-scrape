# Bright Data access matrix

Checked on 2026-10-19 (UTC) by `cargo run -p worker --bin access_check`.
Mode: robots.txt only; the policy column is **live check pending** (run with `--live` and Bright Data credentials).

Manual annotations dated 2026-10-19: the Sanborns search cell, the Browser API note and the robots.txt note below the table were edited by hand after the Browser API work; the next `access_check` run overwrites them.

Meaning: *blocked by robots.txt* pages need KYC approval before Bright Data serves them (`ub_bad_endpoint_robots`); *blocked by policy* is a `policy_` refusal; *error* is any other failure (including a sample URL that no longer exists). No page bodies are stored.

| Retailer | Search | Category | Product |
| --- | --- | --- | --- |
| Walmart MX (`www.walmart.com.mx`) | blocked by robots.txt | live check pending | live check pending |
| Liverpool (`www.liverpool.com.mx`) | live check pending | live check pending | live check pending |
| Chedraui (`www.chedraui.com.mx`) | live check pending | live check pending | live check pending |
| Bodega Aurrera (`www.bodegaaurrera.com.mx`) | blocked by robots.txt | live check pending | live check pending |
| Costco MX (`www.costco.com.mx`) | live check pending | live check pending | live check pending |
| Sam's Club MX (`www.sams.com.mx`) | blocked by robots.txt | live check pending | live check pending |
| Sears MX (`www.sears.com.mx`) | blocked by robots.txt | live check pending | live check pending |
| El Palacio de Hierro (`www.elpalaciodehierro.com`) | live check pending | live check pending | live check pending |
| HEB MX (`www.heb.com.mx`) | live check pending | live check pending | live check pending |
| Sanborns (`www.sanborns.com.mx`) | blocked by robots.txt (Bright Data refused, 2026-10-19) | live check pending | live check pending |
| Waldo's (`waldos.com.mx, www.waldos.com.mx`) | blocked by robots.txt | live check pending | live check pending |
| MercadoLibre MX (`listado.mercadolibre.com.mx, articulo.mercadolibre.com.mx, www.mercadolibre.com.mx`) | live check pending | live check pending | live check pending |

## Notes

- Sears MX and Sanborns category pages are fetched with the Browser API ([ADR 0008](decisions/0008-browser-api-settle-blocking-category-listings.md)); their samples are now `/cat/...?id=4714` (Sears MX) and `/cat/...?id=100609` (Sanborns). Those category cells reflect Web Unlocker until `access_check` checks the Browser channel (separate task).
- The earlier live run (2026-10-19, first three stores only) found Walmart MX search blocked by robots.txt and every other Walmart MX, Liverpool and Chedraui sample page allowed through Web Unlocker.
- A robots.txt that answers 4xx counts as "no restrictions" locally (Sanborns and El Palacio de Hierro reject non-browser clients). Their own robots.txt disallows `/resultados/*` (Sanborns) and `/*?` (El Palacio de Hierro), so Bright Data may refuse those search pages; adapters for them parse category pages instead. For Sanborns the refusal is confirmed: a Browser API capture on 2026-10-19 returned `blocked_needs_kyc (BrowserRobots)` for `/resultados/*` ([sanborns](modules/retailers/sanborns.md#urls)).
- Fixture captures (`capture_fixtures`) record the live result per store in `docs/modules/retailers/<store>.md`.
