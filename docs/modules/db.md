# db

## Purpose

Postgres access through sqlx, plus the schema migrations in `crates/db/migrations/`. Queries use runtime-checked `sqlx::query` so builds never need a live database.

## Public API

- `MIGRATOR`: embedded migrations.
- `connect(database_url)`: opens a connection pool.
- `migrate(&pool)`: applies pending migrations.
- Binary `migrate`: applies migrations to `DATABASE_URL` (`cargo run -p db --bin migrate`). Safe to run repeatedly.

## Dependencies

- Workspace: `domain`.
- External: `sqlx` (Postgres, rustls), `tokio`, `tracing`, `anyhow`.

## Owned tables

All tables. Currently:

- `app_meta(key, value)`: baseline key/value metadata (migration `0001_init.sql`).
- `_sqlx_migrations`: sqlx migration history (managed by sqlx).
