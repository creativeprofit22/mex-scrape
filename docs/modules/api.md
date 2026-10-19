# api

## Purpose

axum HTTP server for the web app. Publishes its OpenAPI spec with utoipa's `OpenApiRouter`; the spec is the contract the `web` client is generated from.

## Public API

- `GET /health` → `{"status":"ok"}` (liveness only, no database call).
- `GET /api-docs/openapi.json`: the OpenAPI spec.
- Library: `router()` returns the axum router and spec; `openapi()` returns the spec.
- Binary `api`: serves on `API_ADDR` (default `127.0.0.1:8080`).
- Binary `openapi <file>`: writes the spec as JSON (used by `npm --prefix web run gen:api`).

## Dependencies

- Workspace: `domain`, `db`, `matching`.
- External: `axum`, `utoipa`, `utoipa-axum`, `serde`, `tokio`, `tracing`.

## Owned tables

None directly; data access goes through `db`.
