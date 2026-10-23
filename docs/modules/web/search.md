# web: search workspace

The search page (`/[lang]/search`, `src/components/search/SearchPanel.tsx`) is the app's main workspace: a toolbar with remembered costs, a status line with per-store progress, summary tiles, filter chips, a sortable results table (cards when it does not fit), the [detail panel](detail-panel.md) for the selected result and recent searches kept in the browser. Styles live in `src/styles/components/search.css`; the visual reasons live in `web/.gg/style-pack.md` §1a, §5 and §10.1.

No API changes: `POST /searches`, then `GET /searches/{id}` every 1.5 s for up to 180 s, then `POST /roi` in batches of up to 100 over the matched results. The detail panel adds `GET /products/{asin}/price-history` and `POST /matches/decisions`.

## SearchPanel (orchestrator)

`SearchPanel` holds only the flow and the state; every part renders in its own component.

- **Gate**: `useRequireOnboarded` (401 → sign-in, `onboarded=false` → onboarding; a loading line until `/me` answers).
- **Polling**: each search gets a run number. A newer search (or a retry) bumps it, so an older poll loop stops at its next answer and never overwrites the newer one.
- **ROI**: one batch effect keyed on the *finished* results. The results only change identity when a search finishes, so sorting and filtering never re-run `/roi`. Prep and shipping come from the saved costs; changing them recalculates.
- **`?q=` rerun**: `?q=<query>` (from a recent-search link) starts that search once, then `router.replace` goes back to the bare `/[lang]/search`. Browser back therefore never reruns a search, which would cost a POST and scraping. The page wraps the panel in `<Suspense>` because `useSearchParams` needs it on a prerendered page.
- **Sort, filters, selection, decisions**: page state only, never stored. They reset on reload; sort and filters stay the same across new searches, selection and this search's match decisions are cleared by a new search.
- **Decisions**: `decisions: Map<rowKey, "accept" | "reject">`, filled when the panel saves a verdict and applied by `applyDecisions` after `buildRows` ([detail panel](detail-panel.md#decisions-adr-0013)). A reject drops the row's match and profit, so it also leaves the KPIs and the "Solo coincidencias" filter.

## Selection contract

The selected result is **state lifted into `SearchPanel`**, not a URL parameter (decided 2026-10-22):

- `selectedKey: string | null`, the `resultKey(result)` of the row: `"<retailer>/<retailer_sku>"`.
- `onSelect(key)`, passed to `ResultsTable` and `ResultCards`.
- A new search clears it. Re-sorting or filtering keeps it, because it is keyed by result, not position. A filtered-out row stays selected and shows again when the filter is turned off.

`DetailPanel` renders as a sibling of the results inside `.results-layout` and reads the row with that key. Closing it sets `selectedKey` to `null` and returns focus to the row's title button (`data-row-key`).

## Components (`src/components/search/`)

| Component | What it renders |
| --- | --- |
| `SearchToolbar` | `role="search"` form: the 16px search input (`.field--toolbar`, ring on the wrapper, `data-testid="search-query"`), `CostsPill`, **Buscar**. `/` focuses the input unless focus is already in a field or contenteditable, or a modifier is held (`aria-keyshortcuts="/"`). |
| `CostsPill` | Disclosure button (`aria-expanded`, `aria-controls`) labelled with the current costs ("Costos: prep $15.00 · envío $0.00"). Its panel is a small form with Prep and Envío, validated with `pesosToCentavos(v, true)`; **Aplicar** saves. Esc or Aplicar closes it and returns focus to the pill. |
| `SearchStatus` | `role="status"` line (`data-testid="search-status"`, `data-state` = state family, `data-view` = view state), per-store outcome chips, and the next action for the state. Errors also go to a `role="alert"`. |
| `KpiStrip` / `KpiTile` | Best profit (display numeral, gain/loss colour, sign, ↓ on a loss), offers, stores with offers, hidden by membership. Static skeleton tiles while queued or running. |
| `FilterChips` | "Solo coincidencias", "Ganancia > 0", "Disponibles" as `<button aria-pressed>` with a check glyph when on, and the count ("3 de 5 ofertas") in `role="status"` (`data-testid="result-count"`). |
| `SortHeader` | The `<th aria-sort>` with a `<button>` inside for price, profit and ROI (argos pattern, `web/.gg/sources.md#sort-header`). |
| `ResultsTable` | Results container ≥53rem. Sticky header, right-aligned tabular numbers, the title is a `button.row-select` (`aria-pressed`) whose `::after` stretches over the row; the store and history links sit above it. |
| `ResultCards` | Results container <53rem (phones, 768–1099px beside the sidebar, and 1200–1549px beside the panel). The same rows as cards with labelled `dl` values; no sideways page scroll. Both views sit in one `.results` size container and a container query shows one, so a hidden view is out of the accessibility tree. |
| `ResultParts`, `Signed` | Shared cells: title (`data-row-key` for focus return), store line, match tags, profit and ROI through `formatSigned` / `signClass` (`data-testid` `result-profit`, `result-roi`). |
| `SearchSkeleton` | Static placeholder rows from `--sheet-press` (no shimmer; style-pack §2.3 has no ambient motion). `aria-hidden`; the status line carries the state. |
| `RecentSearches` | Links to `?q=<query>`, newest first. Used in the sidebar slot and, below 768px only, under the toolbar when idle. Renders nothing when empty. |

Tags are outlined (`.tag`): membership (muted), out of stock (`.tag-loss`), needs review (`.tag-warn`). Profit and ROI use `.gain` / `.loss` with the sign glyphs from `search.sign`, never pink.

## Helpers (`src/lib/search/`)

Pure (no React, type-only imports from `@/lib/api/client`), covered by `e2e/search-helpers.spec.ts`:

- `rows.ts`: `resultKey`, `buildRows(results, roiMap)`, `applyDecisions(rows, decisions)`, `rowProfit`, `rowRoi`.
- `sort.ts`: `DEFAULT_SORT` (profit, highest first), `sortRows` (rows without a value go last in both directions, ties keep the API order), `nextSort`, `ariaSort`.
- `filter.ts`: `filterRows`. `matched` = any Amazon match (accepted or needs review), `profitable` = profit > 0, `inStock` = `availability === "in_stock"`.
- `kpis.ts`: `deriveKpis`. Always the whole search, whatever the filters.
- `status.ts`: `viewState`, `stateFamily`, `FINISHED_STATUSES`, `RETRY_STATES`, `STORE_STATES`.
- `format.ts`: `formatSigned`, `signClass`.
- `storage.ts`: parsing, serialising and the guarded `localStorage` wrappers (below).

`useSearchStore.ts` (client only): `useCosts()` and `useRecentSearches()` through `useSyncExternalStore`. The server snapshot is empty, so hydration always matches.

## Browser storage

| Key | Shape | Validation |
| --- | --- | --- |
| `mex-scrape:costs` | `{"v":1,"prep":<centavos>,"shipping":<centavos>}` | integers ≥ 0; anything else → no saved costs |
| `mex-scrape:recent-searches` | `{"v":1,"items":[...]}` | trimmed, 1–100 characters, deduplicated ignoring case, newest first, at most 8; malformed → empty list |

Reads and writes are wrapped in try/catch (private mode, quota). A write dispatches the same-tab `mex-scrape:storage` event; other tabs update through the `storage` event. Malformed or foreign values are ignored, never thrown.

The calculator starts from the saved costs for prep and shipping when there is no sign-in draft; a draft kept across sign-in still wins.

## States

Every state has copy (`search.status`) and a next action (`search.next`, buttons):

| View state | Shown when | Next action |
| --- | --- | --- |
| `idle` | nothing searched | type a product; recent searches on phones |
| `queued` | POST sent, or status `queued` | skeletons |
| `running` | `running` with no retailers yet | "Consultando tiendas…", skeletons |
| `running-stores` | `running` with retailers | "Tiendas consultadas (n) · comparando con Amazon…" plus store chips |
| `done` | finished with results | sort, filter, select (opens the detail panel) |
| `partial` | some stores failed | results plus the failed stores |
| `failed` | `failed` (`all_retailers_failed`, …) | Reintentar |
| `refused` | `refused` (gate codes; unknown codes get a generic message) | try other words (no retry) |
| `timeout` | still running after 180 s | Reintentar |
| `network` | a request never reached the API | Reintentar |
| `error` | any other request failure | Reintentar |
| `empty` | done, no offers | fewer or more general words |
| `all-hidden` | done, every offer hidden by membership | Revisar mis membresías |

A failed `/roi` keeps the results and shows an alert with **Volver a calcular**. Reintentar reruns the same query with the same costs (both kept in state and storage).

### Gap: no per-store progress while fetching

The worker (`crates/worker/src/jobs/search.rs`) fetches every store’s pages first, then writes **all** `search_retailers` rows at once, then matches with Amazon. So `SearchResponse.retailers` is `[]` for the whole fetch phase and complete once it ends. The UI shows generic progress (“Consultando tiendas…”) until then, then the per-store chips while Amazon matching runs. Per-store progress during fetching needs the worker to write each store’s outcome as it finishes (no API change; the response already carries `retailers`).

## Tests

- `e2e/search-helpers.spec.ts` (node only, no browser): sort order with nulls last and stable ties, filters, KPIs, view states and families, storage parsing (malformed JSON, wrong shapes, negative and fractional centavos, long queries), `addRecent` dedupe and cap.
- `e2e/search.spec.ts` (mocked `/backend/**` with `page.route`, no backend; 1600×900 so the table fits beside the panel): default profit order and header sorting with `aria-sort`, filter chips and the announced count, selection surviving re-sort and filter, costs validated, saved, sent to `/roi` and prefilling the calculator, recent searches in the sidebar rerunning once (and `?q` removed), malformed storage ignored, `/`, every state, the timeout (`page.clock`), a network drop mid-poll then Reintentar keeping query and costs, a new search mid-poll replacing the old one, browser back from a product without searching again (these three at 1440×900 and 390×844), phone cards without sideways scroll, and axe at each main state. The detail-panel tests are listed in [detail-panel](detail-panel.md#tests).
- `e2e/journey.spec.ts` (seeded backend) also checks the default profit order.
