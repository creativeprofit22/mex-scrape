# 0001. Rust backend and Next.js web app

- Status: Accepted
- Date: 2026-10-18

## Context

mex-scrape is a Spanish-first SaaS for Mexican Amazon retail-arbitrage sellers. It fetches and parses many retailer pages, matches products, and serves search, ROI and price history to a browser UI. The owner fixed the stack: Rust backend, Next.js web app, Postgres.

## Decision

- Backend: a Cargo workspace (`crates/`) with small crates split by responsibility (see [ARCHITECTURE.md](../ARCHITECTURE.md)). HTTP via axum; database via sqlx on Postgres.
- Front end: a Next.js App Router + TypeScript app in `web/`.
- Contract: the `api` crate publishes an OpenAPI spec with utoipa; `web/` uses a TypeScript client generated from it (`npm --prefix web run gen:api`). CI fails if the generated files drift.

## Consequences

- Two toolchains (cargo, npm) in one repo and in CI.
- Type safety across the boundary comes from the generated client, not shared code.
- Crate boundaries make dependency rules checkable (`scripts/check_docs.py`).
