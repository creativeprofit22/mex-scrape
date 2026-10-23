# web: app shell

The frame around every page in `web/`: route groups, session-aware links, the current-page rule, breakpoints and scrolling ([ADR 0014](../../decisions/0014-web-design-system-and-app-shell.md)). Styles live in `src/styles/components/shell.css`; the reasons live in `web/.gg/style-pack.md` §1a, §7a.4 and §12.

## Route groups

Groups do not change URLs.

| Group | Layout | Frame | Pages |
| --- | --- | --- | --- |
| `src/app/[lang]/(public)/` | `PublicFrame` | header with wordmark, session links, theme and language; normal document scroll | `/[lang]` (home), `/[lang]/signin`, `/[lang]/signup` |
| `src/app/[lang]/(app)/` | `AppShell` | sidebar ≥768px; top bar and bottom tabs below | `search`, `products/[asin]`, `calculator`, `assistant`, `onboarding`, `account` |

The root layout `src/app/[lang]/layout.tsx` holds `<html>`/`<body>`, the font, the `viewport` export (`viewportFit: "cover"`, zoom left enabled), the skip link and `SessionProvider`. Each frame renders one `<main id="main" tabIndex={-1}>`, so the skip link (the first Tab stop, `href="#main"`) moves focus into the content.

## Components (`src/components/shell/`)

- `SessionProvider`: one `GET /me` for the document, shared by both frames. States: `loading`, `signedIn` (with `me`), `signedOut` (401) and `unknown` (any other failure, with the status and error code). `refresh()` re-reads it; `AuthForm` calls it after sign-in, `SignOutButton` after sign-out and the account page's retry calls it. Every page 401 goes through `useSignInRedirect(lang)` (`src/lib/useSignInRedirect.ts`), which calls `refresh()` before going to sign-in, so the frame never keeps a stale signed-in menu. `useRequireOnboarded` keeps its own `/me` call for the redirect and retry behaviour; its retry also calls `refresh()`, so a frame left `unknown` by a transient failure recovers with the page.
- `AccountMenu` (`sidebar` and `header` variants): signed out → sign-in and sign-up links, which carry `?next=` back to the current page (or keep the validated `?next=` already on an auth page). Signed in → email and plan (sidebar only), the account link and `SignOutButton`. `SignOutButton` leaves for sign-in only when `POST /auth/signout` answers 204 (or 401, already signed out); any other answer keeps the user on the page with a `role="alert"` message from `common.errors`, since the session is still alive. The button is disabled while the request runs. While `loading` or `unknown` it renders `AccountMenuPlaceholder`: an `aria-hidden` box the size of the loaded menu with **no links**, so the frame never offers the wrong links and nothing shifts.
- `PrimaryNav` / `ActiveNavLinks` / `NavLinkList`: a labelled `<nav>` whose current item comes from `usePathname()`. Its `<Suspense>` fallback (used while prerendering request-time pages) is the same links with nothing marked current.
- `FrameLanguage`: wraps `LanguageToggle` (keeps the path, sets the `lang` cookie, `PUT /me/locale` when signed in); its fallback is a plain link to the other language's home.
- `ThemeToggle`: a `.link-button` just before the language switch that flips light/dark and saves it in `localStorage` (`theme`, see `src/lib/theme.ts`). It names where it goes ("Modo oscuro" while light); CSS picks the label from `<html data-theme>` or the system scheme, so it is right before hydration. `ThemeScript` (in the root layout's `<head>`) applies a saved choice before paint ([ADR 0015](../../decisions/0015-web-light-dark-switch.md)).
- `Sidebar`, `TopBar`, `BottomTabs`, `Wordmark`, `AppShell`, `PublicFrame`. Between its nav and the account menu the sidebar holds `RecentSearches` (searches saved in this browser, each a `?q=` link that reruns it; nothing renders until one is saved). On phones the same list shows on the idle search page instead ([search](search.md)).

## Current page

`activeNavItem(pathname)` in `src/lib/nav.ts` maps the first segment after the locale: `search` and `products` → search; `calculator`, `assistant`, `account`, `signin` and `signup` → themselves; home and onboarding → none. The matching link gets `aria-current="page"` and stays a link; other items **omit** the attribute (never `"false"`). It shows as a tint, the accent text colour, a 3px accent bar (left in the sidebar, top on the tabs) and weight 600, never colour alone; in forced colours it gets a 2px underline. `NAV_ITEMS` (Buscar, Calculadora, Asistente) feed the sidebar; `TAB_ITEMS` add Cuenta for the phone tabs.

## Breakpoints and scrolling

- ≥768px (`48rem`): `.app-shell` is a `100dvh` grid: a `--sidebar-width` (232px) sidebar plus the workspace.
- <768px: one column; top bar (wordmark, theme and language), workspace, bottom tabs. Session links live on the Cuenta tab (`/[lang]/account`).
- Only `.workspace` scrolls (`overflow-y: auto; overscroll-behavior: contain`); the document never does. The public frame uses normal document scroll.
- Safe areas: the top bar pads `env(safe-area-inset-top)` and the sides, the bottom tabs pad `env(safe-area-inset-bottom)` and the sides. Surfaces are opaque; there is no motion.
- Frame links, the account page's language toggle and sign-out, and lone links are `--control` (44px) tall at every pointer.

## Account page

`/[lang]/account` (`AccountPanel`, reads the session context). Signed in: email, plan (`planLabel`, falls back to the raw id), language toggle and sign-out. Signed out: sign-in and sign-up links that come back to the account page. Loading shows a `role="status"` line; `unknown` shows `LoadError` with a retry that calls `refresh()`.

## Auth switch link

`AuthForm`'s "¿Ya tienes cuenta?" / "¿No tienes cuenta?" link goes to the other form with the same `?next=` (read after mount and validated by `safeNextPath`; an unsafe value is dropped). `signUpPath(lang, next)` sits beside `signInPath` in `src/lib/ui.ts`.

## Tests

- `e2e/shell.spec.ts` (mocked `/backend/**`, no backend): signed-out and signed-in links, no session links while `/me` is pending, skip link first and focusing `main`, Tab order equal to DOM order, one `aria-current` per nav, the language switch, the theme switch (overrides the system scheme both ways, survives a reload, same tokens as the system scheme), the phone layout (tabs visible, sidebar hidden, only the workspace scrolls), the Cuenta tab, the journey sign in → search → product → back, a failed sign-in retried, a session expiring mid-use (the frame signs out with the redirect) and a failed `/me` recovered by the page's retry (the frame recovers too), both at 1440×900 and 390×844, with axe at each step.
- `e2e/recovery.spec.ts`: the switch link keeps a safe `?next=` and drops an unsafe one.
- E2E locators filter on `{ visible: true }`: Next keeps visited pages mounted but hidden, so a nav can appear twice in the DOM.
