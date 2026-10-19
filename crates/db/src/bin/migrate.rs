//! Applies pending sqlx migrations to the database in `DATABASE_URL`.

use anyhow::Context as _;
use db::DatabaseUrl;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    // Errors here name the variable only, never the value.
    let url = DatabaseUrl::from_env()
        .context("loading DATABASE_URL")?
        .context("DATABASE_URL is not set")?;
    if let Err(error) = run(&url).await {
        let message = url.redactor().sanitize(&format!("{error:#}"));
        tracing::error!(error = %message, "migration failed");
        return Err(anyhow::anyhow!(message));
    }
    Ok(())
}

async fn run(url: &DatabaseUrl) -> anyhow::Result<()> {
    let pool = db::connect(url)
        .await
        .context("failed to connect to Postgres")?;
    db::migrate(&pool).await.context("migration failed")?;
    tracing::info!(
        migrations = db::MIGRATOR.iter().count(),
        "database is up to date"
    );
    Ok(())
}
