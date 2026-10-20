//! Queries for `sessions` (migration `0008_users_sessions.sql`), used by
//! the API's session store.
//!
//! Records are opaque bytes serialized by `api`; only the id travels in
//! the cookie. Expired rows are never returned and are removed by
//! [`delete_expired`]. Timestamps cross as epoch milliseconds.

use sqlx::Row as _;
use sqlx::postgres::PgPool;

/// Inserts a new session; `false` (nothing written) if `id` already exists,
/// so the caller can pick another id.
///
/// # Errors
///
/// Database errors (including CHECK violations); redact before logging.
pub async fn insert_new(
    pool: &PgPool,
    id: &str,
    data: &[u8],
    expires_at_ms: i64,
) -> Result<bool, sqlx::Error> {
    let done = sqlx::query(
        "INSERT INTO sessions (id, data, expires_at) \
         VALUES ($1, $2, to_timestamp($3 / 1000.0)) ON CONFLICT (id) DO NOTHING",
    )
    .bind(id)
    .bind(data)
    .bind(expires_at_ms)
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Inserts or replaces a session.
///
/// # Errors
///
/// Database errors (including CHECK violations); redact before logging.
pub async fn save(
    pool: &PgPool,
    id: &str,
    data: &[u8],
    expires_at_ms: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO sessions (id, data, expires_at) \
         VALUES ($1, $2, to_timestamp($3 / 1000.0)) \
         ON CONFLICT (id) DO UPDATE SET data = EXCLUDED.data, expires_at = EXCLUDED.expires_at",
    )
    .bind(id)
    .bind(data)
    .bind(expires_at_ms)
    .execute(pool)
    .await?;
    Ok(())
}

/// The session's data, unless unknown or expired at `now_ms`.
///
/// # Errors
///
/// Database errors; redact before logging.
pub async fn load(pool: &PgPool, id: &str, now_ms: i64) -> Result<Option<Vec<u8>>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT data FROM sessions WHERE id = $1 AND expires_at > to_timestamp($2 / 1000.0)",
    )
    .bind(id)
    .bind(now_ms)
    .fetch_optional(pool)
    .await?;
    row.map(|r| r.try_get("data")).transpose()
}

/// Deletes one session (sign-out); unknown ids are fine.
///
/// # Errors
///
/// Database errors; redact before logging.
pub async fn delete(pool: &PgPool, id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM sessions WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Deletes sessions expired at `now_ms`; returns how many.
///
/// # Errors
///
/// Database errors; redact before logging.
pub async fn delete_expired(pool: &PgPool, now_ms: i64) -> Result<u64, sqlx::Error> {
    let done = sqlx::query("DELETE FROM sessions WHERE expires_at <= to_timestamp($1 / 1000.0)")
        .bind(now_ms)
        .execute(pool)
        .await?;
    Ok(done.rows_affected())
}
