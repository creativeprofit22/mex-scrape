//! Applies pending sqlx migrations to the database in `DATABASE_URL`.

use anyhow::Context as _;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let url = std::env::var("DATABASE_URL").context("DATABASE_URL is not set")?;
    let pool = db::connect(&url)
        .await
        .context("failed to connect to Postgres")?;
    db::migrate(&pool).await.context("migration failed")?;
    tracing::info!(
        migrations = db::MIGRATOR.iter().count(),
        "database is up to date"
    );
    Ok(())
}
