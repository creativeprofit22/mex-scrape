# db

## Purpose

Postgres access through sqlx, plus the schema migrations in `crates/db/migrations/`. Queries use runtime-checked `sqlx::query` so builds never need a live database.

## Public API

- `MIGRATOR`: embedded migrations.
- `DatabaseUrl`: the connection URL held as a `Secret` (no `Debug`/`Display`/`Serialize`). `from_env()`/`from_lookup()` read `DATABASE_URL` (`DATABASE_URL_ENV`): absent is `None`; empty, non-UTF-8 or malformed (not a `postgres://`/`postgresql://` URL that sqlx can parse) is an error at load time naming the variable, never the value. `new(Secret)` applies the same URL check. `redactor()` removes the URL and its password (raw and percent-decoded, userinfo or `password=` query) from error text; apply it before logging any database error.
- `connect(&DatabaseUrl)`: opens a connection pool.
- `migrate(&pool)`: applies pending migrations.
- Binary `migrate`: applies migrations to `DATABASE_URL` (`cargo run -p db --bin migrate`). Safe to run repeatedly. Errors are logged through the URL's redactor.

## Dependencies

- Workspace: `domain`, `secrets` (`Secret`, `Redactor` for `DATABASE_URL`).
- External: `sqlx` (Postgres, rustls), `tokio`, `tracing`, `anyhow`.

## Owned tables

All tables. Currently:

- `app_meta(key, value)`: baseline key/value metadata (migration `0001_init.sql`).
- `_sqlx_migrations`: sqlx migration history (managed by sqlx).
