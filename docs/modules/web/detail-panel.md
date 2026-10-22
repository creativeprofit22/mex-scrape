# web: detail panel and price chart

Picking a result on the search page opens its details without leaving the list: the store offer, the Amazon match with Confirmar / Descartar, the profit breakdown and a 90-day price chart. The chart is a shared module that the product page (`/[lang]/products/[asin]`) also uses. Styles: `src/styles/components/detail.css` and `chart.css`; the visual reasons live in `web/.gg/style-pack.md` §1a, §3 and §10.1.

No API changes: `GET /products/{asin}/price-history?days=90`, `POST /matches/decisions`, and the profit already fetched by the search page from `POST /roi`.

## Shells (`src/components/detail/`)

`DetailPanel` picks one shell from `matchMedia("(min-width: 75rem)")`, read through `useSyncExternalStore`. Only one shell renders at a time, so ids and headings are never duplicated. On the server and during hydration the query is unknown, and no panel renders.

| Width | Shell | Behaviour |
| --- | --- | --- |
| ≥1200px | `<aside aria-labelledby>` (`.detail-aside`) | Inline in the `.results-layout` grid: results plus a 25rem column, sticky, scrolls on its own. The column is reserved whenever results show, so picking a row never reflows the list. With nothing picked it says how to fill it. |
| 768–1199px | `DetailSheet`, a native modal `<dialog>` | Side sheet: right edge, full height (`100dvh`). |
| <768px | the same `<dialog>` | Bottom sheet: `max-height: 85dvh`, safe-area padding, `overscroll-behavior: contain`. |

`DetailSheet` follows the dialog-sync pattern (`web/.gg/sources.md#dialog-sync`):

- The dialog renders straight into `<body>` (a portal). While it is open, every other child of `<body>` gets `inert`, so the covered page is out of the accessibility tree and layout scans, not only blocked by the modal. `inert` is lifted before focus returns to the row, and when the sheet unmounts (crossing 1200px).
- An effect calls `showModal()` or `close()` from the `open` prop.
- The dialog's `close` event reports back through `onClose`. Esc, the × and a backdrop click (a click whose target is the dialog itself) all end in that event.
- Nothing waits on `animationend` or `transitionend`. The sheet opens without animation (style-pack §2.3).
- The body renders only while open, so a reopen starts fresh.

Closing clears `selectedKey` and moves focus back to the row's visible title button (`data-row-key` on `ResultTitle`, table or card). Resizing across 1200px while open keeps the pick and swaps the shell.

## Content (`DetailContent`)

`DetailContent` is keyed by the row, so a new pick starts with fresh request state. From top to bottom it shows:

1. The store name and the title (display face, the panel's heading). The × button is labelled "Cerrar el detalle".
2. The store price and a store link (`target="_blank" rel="noopener noreferrer"`). Its accessible name says it opens a new tab.
3. **Coincidencia en Amazon** (match only), with three parts:
   - `MatchMeter`: a native `<meter min=0 max=10000>` for `confidence_bp`, named "Confianza de la coincidencia: 82 %". The visible label and value are `aria-hidden` so they aren't read twice.
   - **Confirmar coincidencia** and **Descartar coincidencia** while the match needs review. Once it is accepted, a "Coincidencia confirmada" tag shows instead.
   - After a decision is saved, focus moves to the outcome line (`role="status"`), so keyboard users keep their place.
4. `ProfitBreakdown`: a `<dl>` of amounts:
   - Amazon sale (`amazon_price_centavos`), purchase (`price_centavos`) and Amazon fees (`fees_centavos`, an amount, not a percentage).
   - The saved prep and shipping costs (`mex-scrape:costs`).
   - The row's `/roi` profit, using the gain/loss sign glyphs.
5. **Precio más bajo por día, últimos 90 días**: `PriceChart` plus the `PriceTable` disclosure (`useDetailHistory`), and a link to the full product page.

`useDetailHistory(asin)` fetches 90 days of history:

- A reply for an ASIN that is no longer picked is dropped, so switching rows shows "loading" at once.
- A 401 goes to sign-in.
- `retry()` fetches again.
- A network drop arrives as the client's 503 `network` reply, so it lands in the error state.

### Decisions (ADR 0013)

- **Confirmar** sends `POST /matches/decisions` with `{asin, retailer, retailer_sku, decision: "accept"}` through the CSRF client. After a saved accept the panel calls `retry()` on its history: the price history counts only accepted matches, so the listing joins the chart, which shows "loading" and then the new series without closing the panel. A reject needs no reload, since the chart section is replaced by the rejected outcome.
- **Descartar** sends the same request with `decision: "reject"`. The user chose this on 2026-10-22 (style-pack §1a): a dismissal sticks across searches, and the listing stops counting in that product's price history and stats.
- While a decision is saving, both buttons are disabled and the pressed one shows "Guardando…" with `aria-busy`.
- A 401 goes to sign-in.
- Any other failure shows inline (`role="alert"`, "No se guardó tu decisión. …"; `not_reviewable` has its own text) with **Reintentar**, which resends the same decision.

`SearchPanel` keeps `decisions: Map<rowKey, "accept" | "reject">` for the current search; a new search clears it. `applyDecisions` (`src/lib/search/rows.ts`) applies it after `buildRows`:

- An accept turns `match_status` into `accepted`, so the row badge updates behind the panel.
- A reject removes the match and its profit. The row shows "Sin coincidencia" and leaves the profit KPIs and the "Solo coincidencias" filter, as it will on the next search.

### States

| State | What shows |
| --- | --- |
| nothing picked (≥1200px) | the reserved column's hint, "Elige un resultado…" |
| loading | static skeleton block (`aria-hidden`) and "Cargando historial de precios…" in `role="status"`; no shimmer |
| chart empty | "Aún no hay precios guardados para este producto." |
| listings hidden by membership | above the chart or the empty message, the product page's hint "{count} productos de tiendas ocultos porque requieren una membresía que no tienes." (`hidden_listings > 0`, `data-testid="hidden-listings"`) |
| history error | "No se cargó el historial. …" in `role="alert"`, plus Reintentar |
| no Amazon match | store, title, price and link only, with "Sin coincidencia en Amazon…"; no meter, buttons or chart |
| decision error | inline alert plus Reintentar; the buttons stay |
| rejected | "Coincidencia descartada…" (focusable `role="status"`) |

## Chart

### Pure helpers (`src/lib/chart/`)

No React and no DOM, covered by `e2e/chart-helpers.spec.ts` (Node only).

- `scale.ts`
  - `niceStep(range, maxTicks)`: the smallest 1-2-5 × 10ⁿ step that splits the range into at most `maxTicks` intervals (`sources#nice-step`).
  - `niceTicks(min, max, maxTicks, { minStep })`: round ticks covering `[min, max]` (first ≤ min, last ≥ max), at most `maxTicks` of them, without float noise. A flat range or a single point is padded so the value sits between two ticks. Non-finite input gives `[]`. The chart passes `minStep: 100`, so labels are whole pesos.
  - `linearScale(domain, range)`: value → pixel. A zero-width domain maps to the middle.
- `dates.ts`: `dateTicks(fromMs, toMs, maxTicks)` gives UTC-midnight ticks. It picks the finest step that fits from 1, 2, 7 or 14 days (weeks start on Monday), then the 1st of every 1, 2, 3, 6 or 12 months. That is about weekly for 90 days and monthly for 365.
- `band.ts`: `goodPriceBand(points)`, the **good-price band**. It runs from the lowest daily low up to the 25th percentile of the daily lows in the window (linear interpolation between ranks, whole centavos). It is `null` with fewer than 3 points or when every low is the same price, so no band is drawn.

### Components (`src/components/chart/`)

- `PriceChart`: a hand-rolled SVG (no chart library) drawn at the container's measured width (`ResizeObserver`), so labels stay at their CSS size.
  - Y axis: ticks from `niceTicks`, one per ~40px of plot height, with a hairline grid. The left gutter is sized to the longest tick label.
  - X axis: date ticks from `dateTicks`, one per ~72px.
  - Band: the good-price band is drawn behind the lines, and its range is written under the chart ("Buen precio: de … a …").
  - Series: one line per listing (a dot when a listing has a single day). The legend shows each listing's colour and dash.
  - The `role="img"` `aria-label` summarises the date range, min and max.
- `PriceTable`: the `<details>` data table with every listing's daily low and high, the accessible fallback.

`PriceHistory` (product page, `days` 365) and `DetailContent` (panel, 90 days, 200px tall) both use these.

### Colours and access

- Series use the classes `.series-0` to `.series-5`, which map to existing tokens: `--gain`, `--accent-text`, `--warn`, `--loss`, `--muted`, `--ink`. No hex values are used, and every one is at least 3:1 on `--sheet` in light and dark.
- Pink (`--accent-text`) only ever marks a second line, never profit.
- Lines also differ by dash (solid, `6 3`, `2 3`, `8 3 2 3`, `4 4`, `1 2`), so colour is never the only cue.
- Grid lines are `--rule`, the axis `--rule-strong`, tick labels `--muted` with tabular figures, and the band `--gain` at 12 %.
- Forced colours: lines and labels use `CanvasText`, the grid `GrayText`, and the band becomes a dashed `Highlight` outline. The dashes still tell the series apart.

## Layout around the panel

The results container (`.results`, `container: results / inline-size`) shows the table only when the container is at least 53rem wide, the table's measured minimum. Below that it shows cards. With the 400px panel beside it, the table therefore needs about 1550px of viewport; at 1200–1549px the list shows cards next to the panel. The user chose this over dropping columns on 2026-10-22. Specs that read table rows use a 1600×900 viewport.

## Tests

- `e2e/chart-helpers.spec.ts` (Node only):
  - 1-2-5 steps and the tick cap; ticks covering min and max.
  - Flat series, single point, large peso values, empty and invalid ranges, no float noise.
  - Date ticks at 90 and 365 days.
  - Band edge cases.
- `e2e/search-helpers.spec.ts`: `applyDecisions` (accept, reject, no verdict, no match).
- `e2e/search.spec.ts`, against a mocked `/backend/**`:
  - At 1440: match, meter, profit and chart; Confirmar updating the badge and reloading the chart; Descartar removing the match and profit; loading, history error with retry, empty history and no match; a decision error with retry.
  - At 1024 (side sheet) and 390 (bottom sheet): modal semantics; Esc, × and the backdrop closing it; focus returning to the row; Confirmar from the sheet.
  - Results switching to cards beside the panel.
  - Crossing 1200px while open.
  - axe at 0 violations throughout.
- `e2e/journey.spec.ts` and `assistant.spec.ts` run against the seeded backend at 1600×900.
