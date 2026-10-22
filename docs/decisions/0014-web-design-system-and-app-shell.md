# 0014. Web design system: app-shell direction, tokens, Archivo, platform-first components, modular CSS

- Status: Accepted
- Date: 2026-10-22

## Context

The web app (`web/`) started as a single 324-line `globals.css` with a generic blue accent, `system-ui` text and no structure for adding components. The settled visual direction (`web/.gg/style-pack.md` §1, §1a, §4) is a desktop **app**, not a website: warm paper, ink type, hairline rules, heavy condensed numerals and one rosa-mexicano accent. Sellers compare dense price tables, so the look must stay calm and precise, and profit/loss colours must never be confused with the accent.

Constraints:

- Next.js 16 with plain CSS. Turbopack supports `@import` in global CSS, and global CSS is imported once from the root layout (`node_modules/next/dist/docs/01-app/03-api-reference/08-turbopack.md`, `11-css.md`).
- Every page must pass axe (WCAG 2.1 A/AA) in the e2e suite, in Spanish and English.
- No new runtime dependency for UI.

## Decision

- **App-shell direction.** The target layout is an app shell: sidebar on desktop, bottom tabs on phones, a toolbar, and only the workspace scrolls. Row details open in an inspector (side panel from 1200px, a sheet below). This ADR sets the foundation; the shell itself lands in a later phase.
- **Token set.** All colours live in `web/src/styles/tokens.css`, light first with a `prefers-color-scheme: dark` block that redefines the same names: surfaces `--paper --sheet --tint`, text and lines `--ink --muted --rule --rule-strong`, accent `--accent --accent-edge --accent-text --on-accent`, meaning `--gain --loss --warn`. Hover and press states are `color-mix` of existing tokens, never new hex. Type sizes, radius, the 44px control height, a 4px spacing scale and the press timing are tokens too. The accent is for actions and selection only; gain/loss/warn always come with a sign, glyph or label. Every text pair is ≥ 4.5:1 and every control border and focus ring is ≥ 3:1, light and dark. The only hex outside the token file is the price-history chart palette, which the charts phase replaces.
- **One family.** Archivo, loaded through `next/font/google` with the `wdth` axis and exposed as `--font-archivo`. Titles and numerals use the condensed width (`font-stretch: 62.5%`, weight 800); labels are 12px uppercase tracked; table and figure numbers are tabular.
- **Platform-first components.** No UI kit, Tailwind or CSS Modules. Components are native elements (`button`, `input`, `fieldset`, `details`, `table`) styled by class. Field errors are styled from `[aria-invalid="true"]`, not a separate class. The primary button is a push-button (2px bottom edge, 1px press travel, 60ms), adapted from a library exemplar after a conformance check. Hover applies only on fine pointers; reduced motion removes the press travel.
- **Modular CSS.** `web/src/app/globals.css` is only the cascade entry: it declares `@layer tokens, base, layout, components, utilities;` and imports `src/styles/tokens.css`, `base.css`, `layout.css` and one file per component family under `src/styles/components/`, each with `@import … layer(<name>)`. No rule is written outside a layer, so the layer order, not specificity, decides between files.

## Consequences

- A new component gets its own stylesheet plus one `@import` line; a new colour is a token change, checked for contrast in both schemes. See [design-system.md](../modules/web/design-system.md).
- Unlayered CSS would beat every layered rule, so third-party CSS must be imported into a layer too. The next/font `@font-face` is the only unlayered output.
- `next build` downloads Archivo from Google Fonts, so a build without network fails.
- `:has()` is required for the invalid-field border (supported by every current evergreen browser). Without it the field still shows the error text, just without the red border.
