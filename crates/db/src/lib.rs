//! Postgres access for mex-scrape: connection pool and sqlx migrations.
//!
//! Owns every table in the database. Queries use runtime-checked
//! `sqlx::query` calls so builds never need a live database.

use sqlx::migrate::Migrator;
use sqlx::postgres::{PgPool, PgPoolOptions};

/// Embedded migrations from `crates/db/migrations`.
pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// Opens a Postgres connection pool for `database_url`.
///
/// # Errors
/// Returns an error if the database cannot be reached.
pub async fn connect(database_url: &str) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(5)
        .connect(database_url)
        .await
}

/// Applies all pending migrations to `pool`.
///
/// # Errors
/// Returns an error if a migration fails or the applied history diverges.
pub async fn migrate(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    MIGRATOR.run(pool).await
}
