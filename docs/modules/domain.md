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

## Dependencies

- Workspace: none (enforced by `scripts/check_docs.py`).
- External: none.

## Owned tables

None. Persistence lives in `db`.
