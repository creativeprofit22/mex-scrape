# Amazon.com.mx via Bright Data: live check

Test: `crates/amazon/tests/live_brightdata_mx.rs` (ignored by default). See [amazon.md](../modules/amazon.md#live-check).

```bash
BRIGHTDATA_API_KEY=... BRIGHTDATA_UNLOCKER_ZONE=...   cargo test -p amazon --test live_brightdata_mx -- --ignored --nocapture
```

Question: does the "Amazon products global dataset" `gd_lwhideng15g8jg63s7` return Amazon.com.mx records priced in Mexican pesos?

Hard cap: one `discover_by_keywords` trigger on `https://www.amazon.com.mx` (keywords `audifonos bluetooth`) with `limit_per_input = 1` (at most one billed record), free progress polls and one download; retries off.

## Results

| date | result | trigger | polls | records | domain | currency | has ASIN / UPC / price | notes |
|---|---|---|---|---|---|---|---|---|
| 2026-10-19 | passed | ok | 4 (ready after ~40 s) | 1 | `https://www.amazon.com.mx/` | `MXD` | yes / **no** / yes | amazon.com.mx is supported. Bright Data labels pesos `MXD`, not `MXN`; the fallback now accepts `MXD` on amazon.com.mx records only (regression test `mx_records_labelled_mxd_count_as_pesos`). The record had no `upc`, so barcode lookups through the fallback may return nothing. |

## SP-API sandbox

| date | result | notes |
|---|---|---|
| 2026-10-19 | not run | No `SPAPI_*` credentials exist yet; the owner must finish [the SP-API registration](../modules/amazon.md#owner-setup-sp-api-registration). |

Add a row after each run. Record only statuses, counts, domains and currencies; never the key, zone or record contents.
