# 0015. Web: a light/dark switch over the system scheme

- Status: Accepted
- Date: 2026-10-23
- Extends: [ADR 0014](0014-web-design-system-and-app-shell.md) (token set)

## Context

ADR 0014 picks dark only from `prefers-color-scheme`. Users asked for a button to switch between light and dark whatever the system says. Pages are prerendered (Cache Components), so the server cannot read a saved choice without making every page dynamic, and a choice applied after hydration would flash the other scheme (`node_modules/next/dist/docs/01-app/02-guides/preventing-flash-before-hydration.md`).

## Decision

- **Default stays the system scheme.** With nothing saved, `prefers-color-scheme` decides, as before.
- **A saved choice wins.** The switch stores `light` or `dark` in `localStorage` (`theme`) and sets `<html data-theme>`. An inline `<head>` script (`ThemeScript`) applies the saved value while the HTML is parsed, before the first paint; `<html>` has `suppressHydrationWarning`. There is no "back to system" control.
- **Tokens.** `tokens.css` keeps the light `:root` values and redefines the dark names in two identical blocks: `:root[data-theme="dark"]` and `@media (prefers-color-scheme: dark) { :root:not([data-theme="light"]) }`. `data-theme` also sets `color-scheme`, so native controls follow. `e2e/shell.spec.ts` checks that a picked scheme gives the same tokens as the system one.
- **The switch.** `ThemeToggle` sits beside the language switch in the sidebar footer, the phone top bar and the public header. Like the language switch it names where it goes ("Modo oscuro" while light). Both labels are rendered and CSS shows the one for the scheme on screen, so the label is right before hydration with no client state.

## Consequences

- A new dark value must be added to both dark blocks; the e2e test fails if they differ.
- The choice is per browser, not per account, and it needs JavaScript; without it the page follows the system scheme.
- A strict Content Security Policy would need a nonce for the inline script.
