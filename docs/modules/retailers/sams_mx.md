# Sam's Club MX adapter (`sams_mx`)

Status: **parser tested** against sanitized fixtures captured on 2026-10-19 through Web Unlocker (country MX).

| Host | Access mode | Membership required | Page kinds |
| --- | --- | --- | --- |
| `www.sams.com.mx` | Unlocker | **yes** | search, product |

## URLs

- Search: `/search?q=<q>&z=1`. robots.txt allows `/search?*z=1`.
- Product: `/ip/<slug>/<id>`.

## Parsing

- Uses the Walmart Glass parser (see [walmart_mx](walmart_mx.md)).
- Every offer has `membership_required = true`, set from the registry.

## Tests and fixtures

- Fixtures: `crates/retailers/tests/fixtures/sams_mx/{search,product}.html`.
- Tests: `search_fixture_parses_and_every_offer_needs_membership`, `product_fixture_parses_from_json_ld` (also asserts the membership flag and GTIN), `urls`.

## Browser API

Not used: the Unlocker HTML already contains the product data.
