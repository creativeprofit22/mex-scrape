# web

## Purpose

Next.js (App Router, TypeScript) front end in `web/`. Spanish default copy. Talks to the backend only through the typed client generated from the `api` OpenAPI spec.

## Public API

- `/`: Spanish placeholder page with an API status indicator (calls `GET /health`).
- `src/lib/api/client.ts`: `openapi-fetch` client (base URL `/backend`, proxied to `API_URL` by the `next.config.ts` rewrite). `next build` writes the destination into `.next/routes-manifest.json`, so production uses the build-time `API_URL`; setting it only when running `next start` has no effect.
- `src/lib/api/schema.ts` and `openapi.json`: generated; regenerate with `npm --prefix web run gen:api` and commit. CI fails if they drift from the Rust code.

## Dependencies

- Backend: `api` over HTTP only.
- npm: `next`, `react`, `openapi-fetch`; dev: `openapi-typescript`, `eslint`, `typescript` (pinned in `package-lock.json`).

## Owned tables

None.
