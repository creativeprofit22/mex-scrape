# domain

## Purpose

Core domain types and pure business logic (products, prices, stores). No I/O; every other crate may use it.

## Public API

- `PRODUCT_NAME`: product name constant.
- `Offer`: one normalized retailer offer (`retailer`, `title`, `brand`, `price_mxn`, `list_price_mxn`, `availability`, `gtin`, `retailer_sku`, `url`, `membership_required`, `fetched_at`). `Offer::validate()` collapses whitespace in title/brand, drops a list price that is not above the price, and requires a non-empty title and an absolute `https://` URL.
- Value types (all validated on construction, errors as `OfferError`):
  - `RetailerId`: lowercase `[a-z0-9_]`, 1–40 chars (e.g. `walmart_mx`).
  - `PriceMxn`: integer centavos, never negative, no floats. `parse_mxn("$1,299.00")` reads displayed prices (comma thousands, dot decimals, `$`/`MXN` marks); `from_decimal_str("1299.5")` reads JSON prices (a third decimal rounds half up).
  - `Gtin`: GTIN-8/12/13/14 with GS1 check digit.
  - `RetailerSku`: trimmed, 1–100 chars, no control characters.
  - `Availability { InStock, OutOfStock, PreOrder, Unknown }` with `from_schema_org`.
  - `UnixMillis(i64)`: fetch time, same unit as `brightdata`'s `Clock::now_ms`.
- `Asin`: Amazon product id, exactly 10 ASCII letters/digits (trimmed, upper-cased); errors as `AsinError`.
- ROI (`roi`): `calculate_roi(&RoiInput) -> Result<RoiResult, RoiError>`. See [ROI formula](#roi-formula).
- Price history stats (`history`): `PriceSample { price_mxn, availability, observed_at }`, `window_stats(&[PriceSample], now, days) -> PriceWindow { days, low, high, average: Option<PriceMxn>, observations }`, `STATS_WINDOWS = [30, 90, 365]`, `DAY_MS`. See [Price history](#price-history).
- Match types (`matching`, used by the `matching` crate and the `app::MatchRepo` port): `MatchMethod { Gtin, Title }` and `MatchStatus { Accepted, NeedsReview, Rejected }` (both with `as_str`/`parse`: `gtin`, `title`, `accepted`, `needs_review`, `rejected`), `Confidence` (basis points `0..=10000`; `from_bp` validates, `saturating` clamps, `MAX`, `bp()`; error `ConfidenceError`), `ProductMatch { retailer, retailer_sku, asin, method, confidence, status, matched_at }`. Match rules and thresholds: [matching.md](matching.md).

## ROI formula

Pure, deterministic, integer only: money in centavos, ratios in basis points (1 bp = 0.01%), intermediate math in `i128`.

Inputs (`RoiInput`, all `PriceMxn` except the last): `sell_price` (Amazon price), `buy_price`, `amazon_fees`, `prep_fee` (prep center), `shipping`, `monthly_fixed` (monthly fixed costs), `units_per_month: u32`.

```
fixed_cost_per_unit = round(monthly_fixed / units_per_month)
total_cost          = buy_price + prep_fee + shipping + fixed_cost_per_unit   (cash invested)
profit_centavos     = sell_price − amazon_fees − total_cost                    (may be negative)
roi_bp              = round(profit × 10000 / total_cost)    None when total_cost = 0
margin_bp           = round(profit × 10000 / sell_price)    None when sell_price = 0
```

- `round` is half away from zero, to the centavo or basis point (1 000.00/3 → 333.33; 2 000.00/3 → 666.67; a loss of −0.005 % rounds to −1 bp).
- Errors: `RoiError::ZeroUnits` when `monthly_fixed > 0` and `units_per_month = 0` (with no fixed costs, units are ignored); `RoiError::Overflow` when a result does not fit in 64 bits.
- Worked example: sell 599.00, buy 300.00, fees 120.50, prep 15.00, shipping 20.00, fixed 1 500.00/month over 300 units → fixed/unit 5.00, total cost 340.00, profit 138.50, ROI 4074 bp (40.74%), margin 2312 bp (23.12%).

## Searches and tenants

- `SearchQuery::parse(raw)`: trims and collapses whitespace; 1–`MAX_SEARCH_QUERY_CHARS` (100) characters, else `SearchQueryError::{Empty, TooLong}`. Same rule as the retailer adapters' own check.
- `TenantId::new`: 1–100 bytes of `[A-Za-z0-9_.:-]` (the `tenant_id` column rule; errors never echo the value). `TenantId::for_user(UserId)` is `user-<id>`, the tenant of every signed-in user's searches (ADR 0012); `TenantId::system()` is `system` (scheduled jobs). `TenantId::default_tenant()` (`default`) was the API tenant before accounts (ADR 0011).
- `SearchStatus { Queued, Running, Done, Partial, Failed, Refused }` (`is_terminal` for the last four) and `RetailerSearchOutcome { Ok, CategoryOnly, BlockedNeedsKyc, SkippedRobots, Failed, NoOffers }` (`is_failure` for blocked, skipped and failed), both with `as_str`/`parse` matching the `searches`/`search_retailers` CHECKs.

## Accounts

ADR 0012; used by `app::account` and `db::users`.

- `UserId`: positive `i64` (`new` returns `None` otherwise).
- `Email::parse`: trimmed and lowercased; at most `MAX_EMAIL_BYTES` (254) bytes, exactly one `@` with a non-empty local part and a dotted domain, no whitespace or control characters. `EmailError` never echoes the value.
- `check_new_password`: `MIN_PASSWORD_CHARS` (8) to `MAX_PASSWORD_CHARS` (128) characters, else `PasswordPolicyError::{TooShort, TooLong}`.
- `Plan { Free, Pro }` and `Locale { Es, En }` (Spanish is the default), both with `as_str`/`parse` matching the `users` CHECKs.
- `Memberships { costco, sams }`: `allows(membership_required, &RetailerId)` passes every offer that needs no membership, passes `costco_mx` / `sams_mx` (`COSTCO_RETAILER_ID`, `SAMS_RETAILER_ID`) only with that membership, and hides any other membership store (fails closed). `is_membership_retailer(&RetailerId)` is true for exactly `costco_mx` and `sams_mx`; a `retailers` registry test keeps it equal to the registry's `membership_required` flags. Price stats and series use it to hide product-page listings.

## Price history

- `window_stats` covers `[now − days·DAY_MS, now]`, inclusive at both ends. `now` comes from the caller (an injected `app::Clock`), never the system time.
- Samples observed after `now` and samples whose availability is `OutOfStock` are ignored (sold-out prices are often stale). `InStock`, `PreOrder` and `Unknown` count.
- `low` / `high` are the minimum and maximum price; `average` is the arithmetic mean rounded half up to the centavo. All three are `None` when the window has no samples (`observations = 0`).
- Retention: observations are kept indefinitely. The table `price_observations` is append-only (a trigger refuses UPDATE, DELETE and TRUNCATE; see [db.md](db.md)), and the stats read at most the last 365 days. Pruning old rows needs a new migration and an ADR.

## Assistant

[ADR 0013](../decisions/0013-assistant-sidecar-interface.md); used by `app::assistant`, `api::assistant` and `db::assistant`.

- `ChatText::parse`: control characters removed (newlines and tabs kept), trimmed, 1–`MAX_CHAT_TEXT_CHARS` (2000) characters. `ChatMessage { role: ChatRole { User, Assistant }, text }`. `ChatHistory::parse(iter of (ChatRole, &str))`: 1–`MAX_CHAT_TURNS` (20) messages, the last from the user, at most `MAX_CHAT_TOTAL_CHARS` (16 000) characters in total; `ChatError` never echoes the text.
- `clean_agent_reply`: strips control characters (`strip_control_chars`, keeps newlines and tabs), trims and cuts to `MAX_AGENT_REPLY_CHARS` (8000); the sidecar's answer is untrusted.
- `MatchDecision { Accept, Reject }` with `as_str`/`parse` (`accept`/`reject`, matching the `match_suggestions`/`match_decisions` CHECKs).
- `SuggestionReason::parse`: control characters removed, whitespace collapsed, 1–`MAX_SUGGESTION_REASON_CHARS` (300) characters.
- `TokenCount` (non-negative `u64`, saturating), `TokenUsage { input, output }` with `total()`, and `AgentStop { Done, MaxTurns, TokenBudget }` (`done`, `max_turns`, `token_budget`).

## Dependencies

- Workspace: none (enforced by `scripts/check_docs.py`).
- External: none.

## Owned tables

None. Persistence lives in `db`.
