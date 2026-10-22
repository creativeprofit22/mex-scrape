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
| `src/styles/layout.css` | `layout` | `main`, `.site-header`, `.stack`, `.actions`, coarse-pointer link targets |
| `src/styles/components/button.css` | `components` | primary `button` / `.button`, `button.secondary`, `.link-button`, disabled, reduced motion |
| `src/styles/components/field.css` | `components` | `.field` wrapper, `input`, `textarea`, `fieldset`, `legend`, `.check` |
| `src/styles/components/tag.css` | `components` | `.tag`, `.tag-gain`, `.tag-warn` |
| `src/styles/components/feedback.css` | `components` | `.error`, `.hint`, `.load-error` |
| `src/styles/components/table.css` | `components` | `.table-wrap`, `table`, `caption`, `th`/`td`, `.retailer`, `.facts`, `.outcomes` |
| `src/styles/components/chart.css` | `components` | `.chart`, `.legend`, `details`/`summary` |
| `src/styles/components/chat.css` | `components` | `.chat-log`, `.chat-user`, `.chat-bot`, `.chat-who`, `.chat-text`, `.suggestions`, `.suggestion` |

The `utilities` layer is declared but empty; put single-purpose override classes there.

## Tokens

Light values are listed; the dark block in `tokens.css` redefines the same names.

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
| `--press-duration`, `--press-ease` | 60ms, `ease-out` | press feedback |

Contrast was checked for every pair, light and dark: text ≥ 4.5:1, control borders and focus ≥ 3:1. Re-check both schemes when changing a colour.

The price-history chart (`src/components/PriceHistory.tsx`, `COLORS`) still has its own hex series palette; the charts phase replaces it with tokens.

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
- **Field**: wrap each text input or textarea in `<span className="field">` inside its `<label>`, so the label association stays implicit. The wrapper draws the border; `[aria-invalid="true"]` inside it turns the border `--loss`. Inputs are 16px (no iOS zoom) and 44px tall. Checkboxes use `label.check` and `accent-color`.
- **Tag** (`.tag`, `.tag-gain`, `.tag-warn`): outlined, no fill, label type; muted by default.
- **Feedback**: `.error` (`--loss`, hidden when empty), `.hint` (muted), `.load-error` (the `LoadError` panel: sheet, hairline, error text plus retry).
- **Focus ring**: `:focus-visible` is a 2px `--accent-text` outline with a 2px offset (field controls offset 3px to clear the wrapper border); `Highlight` in forced colours.
- **Touch**: under `(pointer: coarse)`, header links, lone links and the link button get a 44px minimum.

Keep the class names the e2e tests select on: `.chat`, `.chat-bot`, `.chat-text`, `details`, `tbody tr`.

## Add a component stylesheet

1. Create `web/src/styles/components/<name>.css`. Use tokens only: no hex, no new font, no shadow other than the push-button edge.
2. Add one line to `web/src/app/globals.css`, after the existing component imports: `@import "../styles/components/<name>.css" layer(components);`.
3. Gate hover behind `@media (hover: hover) and (pointer: fine)`, give touch targets 44px under `(pointer: coarse)`, and honour `prefers-reduced-motion`.
4. Cite any external recipe as `/* see sources#id */` and add the row to `web/.gg/sources.md`.
5. Add the file to the table above.
6. Run `npm --prefix web run build` and the polish verification floor (`web/.gg/commands/polish.md`).

A new token goes in `tokens.css` with both a light and a dark value, checked for contrast in each.
