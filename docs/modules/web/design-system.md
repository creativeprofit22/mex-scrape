# web: design system

Tokens, type roles, components and CSS file ownership for `web/` ([ADR 0014](../../decisions/0014-web-design-system-and-app-shell.md)). The visual direction and its reasons live in `web/.gg/style-pack.md`; external sources are listed in `web/.gg/sources.md` and cited at the rule as `/* see sources#id */`.

## Cascade

`src/app/globals.css` is the only CSS imported by the app (from `src/app/[lang]/layout.tsx`). It holds no rules, only the layer order and the imports:

```css
@layer tokens, base, layout, components, utilities;
@import "../styles/tokens.css" layer(tokens);
/* … one line per file below … */
```

Later layers win whatever the specificity, so a component rule always beats a base rule. Never write CSS outside a layer: unlayered rules beat all layers.

| File | Layer | Owns |
| --- | --- | --- |
| `src/styles/tokens.css` | `tokens` | every custom property, light and dark; the only file with colour values |
| `src/styles/base.css` | `base` | reset, `body`, type roles, links, the focus ring, `.skip-link`, forced-colours focus |
| `src/styles/layout.css` | `layout` | `.stack`, `.actions`, lone-link targets |
| `src/styles/components/button.css` | `components` | primary `button` / `.button`, `button.secondary`, `.link-button`, `.plain-button` (opts out of all three for self-drawn controls), disabled, reduced motion |
| `src/styles/components/field.css` | `components` | `.field` wrapper (red border from `[aria-invalid]`), `.text-field`, `.field-error`, `input`, `textarea`, `fieldset`, `legend`, `.check-group`, `.check-row` (large rows around a native checkbox) |
| `src/styles/components/shell.css` | `components` | `.app-shell`, `.sidebar`, `.workspace`, `.top-bar`, `.bottom-tabs`, `.nav-list`, `.tab-list`, `.wordmark`, `.account-menu*`, `.theme-toggle*`, `.top-bar__end`, `.public-frame*`, `.account-facts` ([app-shell](app-shell.md)) |
| `src/styles/components/tag.css` | `components` | `.tag`, `.tag-gain`, `.tag-warn`, `.tag-loss` |
| `src/styles/components/feedback.css` | `components` | `.error`, `.hint`, `.load-error` |
| `src/styles/components/card.css` | `components` | `.card` (centred form card), `.card__switch` ([pages](pages.md#shared-pieces)) |
| `src/styles/components/table.css` | `components` | `.table-wrap`, `table`, `caption`, `th`/`td` |
| `src/styles/components/chart.css` | `components` | `.chart`, `.chart__*` (grid, axis, ticks, band, series), `.series-0`…`.series-5`, `.legend`, `details`/`summary` ([detail-panel](detail-panel.md#chart)) |
| `src/styles/components/search.css` | `components` | the search workspace: `.search-toolbar`, `.costs-pill`, `.costs-editor`, `.search-status`, `.store-outcomes`, `.kpi-strip`, `.kpi-tile`, `.filter-bar`, `.chip`, `.results` (size container `results`), `.results-table`, `.sort-button`, `.row-select`, `.result-cards`, `.result-card`, `.skeleton*`, `.recent-searches*`, `.gain`, `.loss` ([search](search.md)) |
| `src/styles/components/detail.css` | `components` | `.results-layout`, `.detail-aside`, `.detail-sheet` (side and bottom sheet, `::backdrop`), `.detail*`, `.breakdown*` (also used by the calculator) ([detail-panel](detail-panel.md)) |
| `src/styles/components/match.css` | `components` | `.match__identity`, `.match__store`, `.match__title`, `.match__actions`, `.match__outcome`, `.match-meter*`: the match pieces shared by the detail panel and the assistant ([detail-panel](detail-panel.md#shared-match-pieces)) |
| `src/styles/components/chat.css` | `components` | the assistant: `.assistant`, `.chat`, `.chat-log`, `.chat-user`, `.chat-bot`, `.chat-who`, `.chat-text`, `.chat-status`, `.chat-cards*`, `.suggestion-card*`, the sticky `.chat-composer*` ([pages](pages.md)) |
| `src/styles/components/calculator.css` | `components` | `.calculator` (size container), `.calculator__layout`, the sticky `.calculator__result`, `.calculator__numeral` ([pages](pages.md)) |
| `src/styles/components/product.css` | `components` | `.product-header*`, `.back-link`, `.stats-table`, `.product__loading`, `.product__skeleton*`, `.product__empty` |
| `src/styles/components/home.css` | `components` | `.home`, `.home__intro`, `.home-steps*` |

The `utilities` layer is declared but empty; put single-purpose override classes there.

## Tokens

Light values are listed; `tokens.css` redefines the same names for dark in two identical blocks: `:root[data-theme="dark"]` (picked with the switch) and `@media (prefers-color-scheme: dark) { :root:not([data-theme="light"]) }` (the system default). Add a dark value to both; `e2e/shell.spec.ts` fails if they differ ([ADR 0015](../../decisions/0015-web-light-dark-switch.md)).

| Token | Light | Use |
| --- | --- | --- |
| `--paper` | `#F5F1EB` | page background |
| `--sheet` | `#FBF9F5` | fields, tables, panels, secondary buttons |
| `--tint` | `#F8E7EE` | selected row, active item |
| `--ink` | `#1E1D1C` | body text |
| `--muted` | `#5F5B55` | secondary text, default tag |
| `--rule` | ink at 12% | hairlines between rows and sections |
| `--rule-strong` | `#8E877D` | control borders (≥ 3:1 on paper and sheet) |
| `--accent` | `#CF1361` | primary button fill; actions and selection only |
| `--accent-edge` | `#9E0E4A` | push-button bottom edge |
| `--accent-text` | `#B5104F` | links, focus ring, accent text on paper |
| `--on-accent` | `#FFFFFF` | text on the accent fill |
| `--gain` / `--loss` / `--warn` | `#186F48` / `#A8432B` / `#8A5A00` | profit, loss/errors, caution; always with a sign, glyph or label |
| `--accent-hover`, `--accent-press`, `--sheet-hover`, `--sheet-press` | `color-mix` | derived states; never add hex for a state |
| `--font-sans` | Archivo, `system-ui` | the only family |
| `--text-label` … `--text-kpi` | 12 / 14 / 15 / 16 / 18 / 28 / 44px | label, table, UI, input, section, title, KPI |
| `--radius`, `--radius-tag` | 6px, 4px | controls and panels, tags |
| `--control`, `--row` | 44px, 56px | control height, table row height |
| `--space-1` … `--space-8` | 4 … 40px | spacing scale |
| `--sidebar-width`, `--tabbar-height` | 232px, `--control` | app-shell sidebar column, phone tab height |
| `--press-duration`, `--press-ease` | 60ms, `ease-out` | press feedback |

Contrast was checked for every pair, light and dark: text ≥ 4.5:1, control borders and focus ≥ 3:1. Re-check both schemes when changing a colour.

The price chart has no colours of its own: its series map to existing tokens (`.series-N` in `chart.css`) and differ by dash too ([detail-panel](detail-panel.md#colours-and-access)).

## Type roles

| Role | Selector | Spec |
| --- | --- | --- |
| Display | `h1`, `.t-display` | 28px, weight 800, `font-stretch: 62.5%`, `word-spacing: 0.1em` |
| Section | `h2` | 18px, weight 600 |
| Body | `body` | 15px / 1.5, normal width |
| Label | `.t-label`, `legend` | 12px, weight 600, uppercase, 0.08em tracking |
| Numeric | `.t-num`, `td`, `dd` | `font-variant-numeric: tabular-nums` |

Archivo comes from `next/font/google` in `src/app/[lang]/layout.tsx` (`axes: ["wdth"]`, `variable: "--font-archivo"`, class on `<html>`). Condensed width is for titles and numerals only.

## Components

- **Primary button** (`button`, `.button`): accent fill, `--on-accent` text, a 2px `--accent-edge` bottom edge; `:active` moves it 1px down onto a 1px edge in `--press-duration`. Hover only under `(hover: hover) and (pointer: fine)`. `prefers-reduced-motion: reduce` drops the travel and keeps the colour change. Min height `--control`.
- **Secondary** (`button.secondary`): sheet fill, `--rule-strong` border, ink text.
- **Link button** (`.link-button`): a button that looks like a link (sign out); 44px on coarse pointers.
- **Field**: prefer `TextField` (`src/components/form/`): label, `.field`, input, hint and inline error, with every id from one `useId()` so `aria-describedby` names the hint and the current error ([pages](pages.md#shared-pieces)). Hand-written fields wrap the input or textarea in `<span className="field">`; the wrapper draws the border and `[aria-invalid="true"]` inside it turns the border `--loss`. Inputs are 16px (no iOS zoom) and 44px tall. Checkboxes stay native, inside a `label.check-row` (at least 56px; tint plus weight when checked).
- **Tag** (`.tag`, `.tag-gain`, `.tag-warn`, `.tag-loss`): outlined, no fill, label type; muted by default.
- **Plain button** (`.plain-button`): no fill, border or push edge; for controls that draw themselves (filter chips, sort headers, row titles, the costs pill). Keeps the focus ring.
- **Feedback**: `.error` (`--loss`, hidden when empty), `.hint` (muted), `.field-error` (a field's inline error), `.load-error` (the `LoadError` panel in `src/components/feedback/`: sheet, hairline, error text plus retry).
- **Card** (`.card`): the centred form card for sign-in, sign-up and onboarding; sheet, hairline, `--radius`, no shadow.
- **Focus ring**: `:focus-visible` is a 2px `--accent-text` outline with a 2px offset (field controls offset 3px to clear the wrapper border); `Highlight` in forced colours.
- **Targets**: frame links, lone links (`.actions > a`, a link alone in a paragraph) and the account page's language toggle and sign-out are `--control` (44px) tall at every pointer; the link button elsewhere gets 44px under `(pointer: coarse)`.

Keep the class names the e2e tests select on: `.chat`, `.chat-bot`, `.chat-text`, `details`, `tbody tr`, `.results-table`, `.row-select`, `.result-card`.

## Add a component stylesheet

1. Create `web/src/styles/components/<name>.css`. Use tokens only: no hex, no new font, no shadow other than the push-button edge.
2. Add one line to `web/src/app/globals.css`, after the existing component imports: `@import "../styles/components/<name>.css" layer(components);`.
3. Gate hover behind `@media (hover: hover) and (pointer: fine)`, give touch targets 44px under `(pointer: coarse)`, and honour `prefers-reduced-motion`.
4. Cite any external recipe as `/* see sources#id */` and add the row to `web/.gg/sources.md`.
5. Add the file to the table above.
6. Run `npm --prefix web run build` and the polish verification floor (`web/.gg/commands/polish.md`).

A new token goes in `tokens.css` with both a light and a dark value, checked for contrast in each.
