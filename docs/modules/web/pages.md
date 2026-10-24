# web: pages

The pages outside the search workspace: calculator, product page, assistant, sign-in and sign-up, onboarding and home. They all use the frames from [app-shell](app-shell.md) and the tokens and components from [design-system](design-system.md). The visual reasons and the product decisions live in `web/.gg/style-pack.md` §1a. No page has an API of its own: each one uses the endpoints listed in [web.md](../web.md#public-api).

## Shared pieces

| Piece | Where | Used by |
| --- | --- | --- |
| `TextField` | `src/components/form/` | auth forms, calculator |
| `LoadError` | `src/components/feedback/` | product page, onboarding, assistant, search, account |
| `MatchIdentity`, `MatchDecisionButtons`, `MatchOutcome`, `MatchMeter` | `src/components/match/` | the detail panel and the assistant's suggestion cards ([detail-panel](detail-panel.md#shared-match-pieces)) |
| `.card` | `src/styles/components/card.css` | sign-in, sign-up, onboarding |

- **`TextField`**: a label, the bordered `.field` and an `<input>`, plus an optional hint and inline error. All ids come from one `useId()` (`-hint`, `-error`), so `aria-describedby` always names the hint and the current error (`web/.gg/sources.md#field-ids`). `aria-invalid="true"` is set only while there is an error, and the field's border turns `--loss` from that attribute alone (`sources#field-invalid`). The error line (`.field-error`) is not an alert, because the form's own `role="alert"` names the failed action.
- **`.card`**: a centred form card on `--sheet` with a 1px `--rule` border and `--radius`, `max-width: 28rem`, and no shadow. `.card__switch` holds the link to the other form.

## Calculator (`/[lang]/calculator`, `src/components/calculator/`)

- `Calculator` holds the state and the submit, and `CalculatorResult` renders the result. The pure helpers live in `src/lib/calculator/draft.ts`: `saveDraft`, `readDraft`, `clearDraft`, `parseDraft` (the sessionStorage draft, key `mex-scrape:calculator-draft`) and `parseInputs` (returns the `/roi` item or the list of invalid fields).
- **Layout** (`calculator.css`): `.calculator` is an inline-size container. From 44rem of its own width the inputs sit beside a 20rem result column, `position: sticky; top: var(--space-4)` inside `.workspace` (the only scroller, and no ancestor clips it). Below 44rem the two stack. It is a container query because the workspace width depends on the sidebar.
- **Fields** are grouped in two fieldsets: "Precios" (sale and purchase price, fees) and "Costos" (prep, shipping, monthly fixed costs, units per month). Prep and shipping start from the costs saved on the search page (`mex-scrape:costs`). A draft kept across sign-in wins over them.
- **Result**: the profit is a display numeral (`--text-kpi`, condensed, weight 800) drawn by `Signed`, so it has the gain/loss colour plus its sign or ↓ glyph, never colour alone. ROI is signed too. Margin, total cost and fixed cost per unit sit in a `.breakdown` list. `data-testid="calc-profit"` and `calc-roi` are kept.
- **States**:

| State | What shows |
| --- | --- |
| empty | the result panel's hint "Escribe los precios y pulsa Calcular para ver la ganancia y el ROI." |
| pending | the button reads "Calculando…" with `aria-busy`; the result panel gets `aria-busy` |
| invalid input | each bad field gets `aria-invalid` and an inline message; a form `role="alert"` says "No se pudo calcular. Revisa los campos marcados." |
| API error | `invalid_amount` and `overflow`: form alert naming the action; `zero_units`: that form alert plus `aria-invalid`, an inline message and focus on "Unidades vendidas al mes"; other failures: "No se pudo calcular. {reason}" |
| success | profit, ROI and the breakdown |
| 401 | the draft is saved, then sign-in with `?next=` back to the calculator |

Not gated on onboarding: it needs no memberships.

## Product page (`/[lang]/products/[asin]`, `src/components/product/`)

- `PriceHistory` orchestrates the page. It is gated on onboarding like search, and the ASIN must match `^[A-Z0-9]{10}$`.
- `ProductHeader` has:
  - a "Volver a buscar" link to `/[lang]/search`, where the kept-mounted search still shows its results;
  - when this browser has a recent search, a "Repetir «{query}»" link (`rerunHref`, `?q=`);
  - the h1 with the Amazon product name (`title` from the history reply, ADR 0017), clamped to three lines with the full text kept in the DOM; while loading, or when no name is stored, it falls back to "Historial de precios de {asin}" (`productTitle` in `src/lib/product/name.ts`);
  - a `.product-header__meta` `<dl>` with the ASIN in tabular figures and, when known, the brand (`data-testid="product-brand"`).
- The chart is the shared `PriceChart` plus the `PriceTable` disclosure, 365 days ([detail-panel](detail-panel.md#chart)).
- `PriceStatsTable` shows one `.stats-table` per listing (30/90/365-day low, high and average, plus observations). Figures are tabular and right-aligned, with row headers on the left. It keeps `data-testid="price-stats"`.
- **States**:

| State | What shows |
| --- | --- |
| loading | a static chart skeleton (`aria-hidden`, no shimmer) and a `role="status"` line, `aria-busy` on the section |
| empty | "Aún no hay historial para este producto." (`data-testid="product-empty"`): a known ASIN (200) with no listings; the header keeps its link back |
| hidden listings | "{count} productos de tiendas ocultos…" when `hidden_listings > 0` (`data-testid="hidden-listings"`) |
| not found (404 `product_not_found`, or 400 `invalid_asin`) | its own page, replacing header and chart (`data-testid="product-not-found"`): h1 "No encontramos este producto", a `role="alert"` line naming the ASIN and telling the user to search by name, and a primary "Buscar un producto" link to `/[lang]/search`; no Reintentar and never the empty copy (`isProductNotFound` in `src/lib/product/name.ts`). The route's `^[A-Z0-9]{10}$` check normally stops a malformed ASIN first |
| error | `LoadError` with Reintentar, which reloads both requests |
| 401 | sign-in |

## Assistant (`/[lang]/assistant`, `src/components/assistant/`)

- `AssistantChat` holds the state and the requests; its logic is unchanged ([ADR 0013](../../decisions/0013-assistant-sidecar-interface.md), [web.md](../web.md#public-api)). `ChatLog`, `ChatComposer` and `SuggestionCard` render the parts.
- **Layout** (`chat.css`): `.assistant` fills the workspace height, and the visible assistant's `.workspace__content` drops its bottom padding. The intro and disclaimer come first, then the log (`.chat`: the `<ol class="chat-log">`, the `role="status"` thinking line and the alert), then the composer.
- The composer is `position: sticky; bottom: 0` on `--paper` with a top `--rule`. The shell is `100dvh`, and the phone tabs sit in their own grid row with the safe area, so the composer never hides behind them. From 768px (no tabs) it pads `env(safe-area-inset-bottom)` itself.
- The textarea stays 16px: Enter sends and Shift+Enter adds a new line. "Nueva conversación" clears everything.
- After each message, reply or status change, the workspace scrolls by just enough to show the newest entry above the composer (`behavior: "auto"`, no smooth motion).
- **Suggestion cards** render inside the log, right after the reply that produced them, in a `<section>` headed "Coincidencias propuestas" (`data-testid="suggestion"` per card). Each one uses `MatchIdentity` (store and title), the proposal tag and reason, `MatchDecisionButtons` and `MatchOutcome`. **Confirmar** sends `POST /matches/decisions`. **Descartar** only marks the card dismissed and **sends no request**. A failed Confirmar shows inline (`role="alert"`) and keeps both buttons, so pressing Confirmar again retries. Once a card is decided, focus moves to its outcome line.
- **States**: an empty log hint, thinking (`role="status"`, `aria-busy` on the log), API errors mapped per code (`agent_budget`, `agent_busy`, …), saving and saved per card, and a 401 that goes to sign-in.

## Sign-in and sign-up (`/[lang]/signin`, `/[lang]/signup`, `src/components/auth/AuthForm.tsx`)

- These are centred `.card`s in the public frame, with an h1 and `TextField` email and password. On sign-up, the password hint is linked with `aria-describedby`.
- **API codes map to a field**: `invalid_email` and `email_taken` mark the email, and `weak_password` marks the password, each with `aria-invalid` and an inline message, plus a form alert. `invalid_credentials` is a form alert only, so it never says which field was wrong. Any other code goes to the form alert. Editing a marked field clears its message.
- The button shows its pending label while the request runs. Afterwards the user goes to onboarding while `onboarded` is false, otherwise to the validated `?next=` or search. The link to the other form keeps the validated `?next=`.

## Onboarding (`/[lang]/onboarding`, `src/components/auth/OnboardingForm.tsx`)

- Onboarding keeps the **app frame** (style-pack §1a) and shows as a centred `.card` in the workspace.
- The Costco and Sam's Club rows are full-width `label.check-row` elements: at least 56px tall, on `--sheet` with a `--rule-strong` border, keeping the **native checkbox**. A checked row turns `--tint` with the accent text colour and a heavier weight, and the checkbox itself shows the state, so it never relies on colour alone.
- **States**: loading (`aria-busy` card with a `role="status"` line), a load error (`LoadError` with Reintentar), saving, and a save error (form alert). Saving re-reads the frame session (`useSession().refresh()`), so home's redirect sees `onboarded: true`, then the user goes to the validated `?next=` or search.

## Home (`/[lang]`, `src/app/[lang]/(public)/page.tsx`)

- **Signed out**: the h1 (product name), a one-line intro, and an `<ol class="home-steps">` with three steps: Busca un producto → compara con Amazon → calcula tu ganancia. Each step has its number in the display face, a title and one line. Below them: **Crear cuenta** (primary) and **Iniciar sesión**. There is no API status and no hero.
- **Signed in**: `SignedInRedirect` (`src/components/home/`) waits for the frame's session. Once it is `signedIn`, it calls `router.replace` to `/[lang]/search`, or to onboarding when `onboarded` is false, so Back never returns to home. While loading, signed out or unknown, the home content stays, so a signed-out visitor sees no flash.

## Tests

- `e2e/pages.spec.ts` fakes `/backend/**` with `page.route` (no backend) and runs at 1440×900 and 390×844 with axe on each page:
  - **calculator**: columns at 1440, stacked at 390; the result stays put while the inputs scroll; gain vs loss tone; `aria-invalid` with a described error; the prefill from the saved costs.
  - **product**: the back link, the rerun link only with a recent search, stats and the hidden note; the Amazon name as the h1 and the brand in /es and /en; a known ASIN with empty `listings` shows the empty state under the ASIN heading; a mocked 404 `product_not_found` and a 400 `invalid_asin` show the not-found state (heading, ASIN, search link, no Reintentar, no empty copy); Reintentar recovers.
  - **assistant**: the composer stays in view after six messages; the cards sit in the log; Descartar sends 0 `/matches/decisions` requests and Confirmar sends 1.
  - **auth**: `weak_password` marks the password, and its `aria-describedby` names the hint and the error; `invalid_credentials` marks no field.
  - **onboarding**: rows of at least 44px on touch, with native checkboxes.
  - **home**: three steps and no service status when signed out; signed in → search (Back skips home); not onboarded → onboarding; after saving onboarding, the wordmark leads home and on to search, not back to onboarding.
- `e2e/product-helpers.spec.ts` (Node only): `productTitle` fallbacks and `isProductNotFound`.
- Still covering these pages against the seeded backend: `recovery.spec.ts` (calculator draft through sign-in, `?next=`), `journey.spec.ts` (product page with the seeded Amazon name and brand, calculator ROI) and `assistant.spec.ts`.
