//! Queries for `tracked_products` (migrations `0006_jobs.sql`,
//! `0007_tracked_attempts.sql`): listings
//! whose price the scheduler refreshes.

use sqlx::Row as _;
use sqlx::postgres::PgPool;

/// Starts tracking a listing. `false` if it was already tracked.
///
/// # Errors
///
/// Database errors (including CHECK violations); redact before logging.
pub async fn track(
    pool: &PgPool,
    retailer: &str,
    retailer_sku: &str,
    now_ms: i64,
) -> Result<bool, sqlx::Error> {
    let done = sqlx::query(
        "INSERT INTO tracked_products (retailer, retailer_sku, tracked_at) \
         VALUES ($1, $2, to_timestamp($3 / 1000.0)) ON CONFLICT DO NOTHING",
    )
    .bind(retailer)
    .bind(retailer_sku)
    .bind(now_ms)
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Up to `limit` tracked listings never tried or last tried (attempted, else
/// refreshed) before `older_than_ms`, as `(retailer, retailer_sku)`:
/// never-tried first, then oldest, ties by retailer then SKU. Listings whose
/// refresh keeps failing thus rotate behind the others.
///
/// # Errors
///
/// Database errors; redact before logging.
pub async fn due(
    pool: &PgPool,
    older_than_ms: i64,
    limit: i64,
) -> Result<Vec<(String, String)>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT retailer, retailer_sku FROM tracked_products \
         WHERE COALESCE(last_attempted_at, last_refreshed_at) IS NULL \
            OR COALESCE(last_attempted_at, last_refreshed_at) < to_timestamp($1 / 1000.0) \
         ORDER BY COALESCE(last_attempted_at, last_refreshed_at) NULLS FIRST, \
                  retailer COLLATE \"C\", retailer_sku COLLATE \"C\" \
         LIMIT $2",
    )
    .bind(older_than_ms)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    rows.iter()
        .map(|r| Ok((r.try_get("retailer")?, r.try_get("retailer_sku")?)))
        .collect()
}

/// Records that a listing was refreshed. `false` if it is not tracked.
///
/// # Errors
///
/// Database errors; redact before logging.
pub async fn mark_refreshed(
    pool: &PgPool,
    retailer: &str,
    retailer_sku: &str,
    now_ms: i64,
) -> Result<bool, sqlx::Error> {
    let done = sqlx::query(
        "UPDATE tracked_products SET last_refreshed_at = to_timestamp($3 / 1000.0) \
         WHERE retailer = $1 AND retailer_sku = $2",
    )
    .bind(retailer)
    .bind(retailer_sku)
    .bind(now_ms)
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Records that a refresh of `retailer_skus` was attempted, whether or not
/// it succeeded. Returns how many tracked listings were updated.
///
/// # Errors
///
/// Database errors; redact before logging.
pub async fn mark_attempted(
    pool: &PgPool,
    retailer: &str,
    retailer_skus: &[&str],
    now_ms: i64,
) -> Result<u64, sqlx::Error> {
    let done = sqlx::query(
        "UPDATE tracked_products SET last_attempted_at = to_timestamp($3 / 1000.0) \
         WHERE retailer = $1 AND retailer_sku = ANY($2)",
    )
    .bind(retailer)
    .bind(retailer_skus)
    .bind(now_ms)
    .execute(pool)
    .await?;
    Ok(done.rows_affected())
}
