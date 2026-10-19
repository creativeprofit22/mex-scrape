# api

## Purpose

axum HTTP server for the web app. Publishes its OpenAPI spec with utoipa's `OpenApiRouter`; the spec is the contract the `web` client is generated from.

## Public API

- `GET /health` → `{"status":"ok"}` (liveness only, no database call).
- `GET /admin/brightdata/access` → `[{retailer, host, ok, blocked_needs_kyc, skipped_robots, failed}]`: Bright Data access outcomes per host (retailer id from the `retailers` registry, else the host). Read-only. Needs `Authorization: Bearer <ADMIN_API_TOKEN>` (constant-time compare). Fails closed: 503 when `ADMIN_API_TOKEN` is unset or shorter than 16 bytes, or `DATABASE_URL` is missing or unreachable; 401 for a missing or wrong token; 500 (no detail) if the counts cannot be read.
- `GET /api-docs/openapi.json`: the OpenAPI spec. Admin routes declare the `admin_bearer` HTTP bearer security scheme (`ADMIN_BEARER`); `/health` has none.
- Library: `router()` returns the axum router and spec (admin routes disabled); `router_with(AdminState)` enables them (`AdminState::new(token, Arc<dyn AccessSource>)`, `PgAccessSource` over Postgres); `openapi()` returns the spec.
- Binary `api`: serves on `API_ADDR` (default `127.0.0.1:8080`). `/health` never touches the database; `ADMIN_API_TOKEN` and `DATABASE_URL` enable the admin route.
- Binary `openapi <file>`: writes the spec as JSON (used by `npm --prefix web run gen:api`).

## Dependencies

- Workspace: `domain`, `db`, `matching`, `retailers` (host → retailer id for the admin counts).
- External: `axum`, `utoipa`, `utoipa-axum`, `serde`, `tokio`, `tracing`.

## Owned tables

None directly; data access goes through `db`.
