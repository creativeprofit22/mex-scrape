# Bright Data live smoke test

Test: `crates/brightdata/tests/live_smoke.rs` (ignored by default; at most 3 requests, retries off; at most one billed Web Unlocker request). See [brightdata.md](../modules/brightdata.md#live-smoke-test).

```bash
BRIGHTDATA_API_KEY=... BRIGHTDATA_UNLOCKER_ZONE=... \
  cargo test -p brightdata --test live_smoke -- --ignored --nocapture
```

Expected: the Unlocker fetch of `https://geo.brdtest.com/mygeo.json` (country `mx`) succeeds, and `progress` on an unknown snapshot id returns a terminal error that is not HTTP 401/403.

## Results

| date | result | Unlocker | Datasets progress | requests | notes |
|---|---|---|---|---|---|
| 2026-10-19 | not run | — | — | 0 | `BRIGHTDATA_API_KEY` and `BRIGHTDATA_UNLOCKER_ZONE` were not available in the implementation environment. |

Add a row after each run. Record only statuses, sizes, exit country and redacted error text; never the key, zone or page body.
