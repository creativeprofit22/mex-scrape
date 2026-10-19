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

## Price history

- `window_stats` covers `[now − days·DAY_MS, now]`, inclusive at both ends. `now` comes from the caller (an injected `app::Clock`), never the system time.
- Samples observed after `now` and samples whose availability is `OutOfStock` are ignored (sold-out prices are often stale). `InStock`, `PreOrder` and `Unknown` count.
- `low` / `high` are the minimum and maximum price; `average` is the arithmetic mean rounded half up to the centavo. All three are `None` when the window has no samples (`observations = 0`).
- Retention: observations are kept indefinitely. The table `price_observations` is append-only (a trigger refuses UPDATE, DELETE and TRUNCATE; see [db.md](db.md)), and the stats read at most the last 365 days. Pruning old rows needs a new migration and an ADR.

## Dependencies

- Workspace: none (enforced by `scripts/check_docs.py`).
- External: none.

## Owned tables

None. Persistence lives in `db`.
