//! pgmq job queues (migrations `0005_pgmq.sql` and `0006_jobs.sql`,
//! ADR 0011), called through our own sqlx pool.
//!
//! Queue names come from the closed [`Queue`] enum, never from caller
//! strings. Messages cross the boundary as JSON text cast to `jsonb`, like
//! the rest of this crate.

use sqlx::Row as _;
use sqlx::postgres::{PgExecutor, PgPool};

/// A pgmq queue created by `0006_jobs.sql`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Queue {
    /// Search jobs enqueued by the API.
    SearchJobs,
    /// Dead letters of [`Queue::SearchJobs`].
    SearchJobsDlq,
    /// Scheduled `product_refresh` and `category_browse` jobs.
    RefreshJobs,
    /// Dead letters of [`Queue::RefreshJobs`].
    RefreshJobsDlq,
}

impl Queue {
    /// Every queue, work queues first.
    pub const ALL: [Self; 4] = [
        Self::SearchJobs,
        Self::RefreshJobs,
        Self::SearchJobsDlq,
        Self::RefreshJobsDlq,
    ];

    /// The pgmq queue name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SearchJobs => "search_jobs",
            Self::SearchJobsDlq => "search_jobs_dlq",
            Self::RefreshJobs => "refresh_jobs",
            Self::RefreshJobsDlq => "refresh_jobs_dlq",
        }
    }

    /// The dead-letter queue of a work queue; `None` for a dead-letter
    /// queue itself.
    #[must_use]
    pub const fn dead_letter(self) -> Option<Self> {
        match self {
            Self::SearchJobs => Some(Self::SearchJobsDlq),
            Self::RefreshJobs => Some(Self::RefreshJobsDlq),
            Self::SearchJobsDlq | Self::RefreshJobsDlq => None,
        }
    }
}

/// A message read from a queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueMessage {
    /// pgmq message id (unique per queue).
    pub msg_id: i64,
    /// How many times it has been read, this read included.
    pub read_ct: i32,
    /// The JSON body, as text.
    pub message: String,
}

/// Why a dead-letter move did not happen.
#[derive(Debug, thiserror::Error)]
pub enum DeadLetterError {
    /// The queue is itself a dead-letter queue.
    #[error("queue has no dead-letter queue")]
    NoDeadLetterQueue,
    /// Database error; redact before logging.
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// Enqueues `json` (a JSON object) on `queue` and returns its message id.
/// Takes any executor, so callers can enqueue inside their own
/// transaction.
///
/// # Errors
///
/// Database errors (including invalid JSON); redact before logging.
pub async fn send<'e>(
    executor: impl PgExecutor<'e>,
    queue: Queue,
    json: &str,
) -> Result<i64, sqlx::Error> {
    sqlx::query("SELECT pgmq.send($1::text, $2::jsonb) AS msg_id")
        .bind(queue.as_str())
        .bind(json)
        .fetch_one(executor)
        .await?
        .try_get("msg_id")
}

/// Reads up to `qty` visible messages and hides them for `vt_secs`
/// seconds. A message that is not deleted before then becomes visible
/// again with a higher `read_ct`.
///
/// # Errors
///
/// Database errors; redact before logging.
pub async fn read(
    pool: &PgPool,
    queue: Queue,
    vt_secs: i32,
    qty: i32,
) -> Result<Vec<QueueMessage>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT msg_id, read_ct, message::text AS message \
         FROM pgmq.read($1::text, $2::integer, $3::integer) ORDER BY msg_id",
    )
    .bind(queue.as_str())
    .bind(vt_secs)
    .bind(qty)
    .fetch_all(pool)
    .await?;
    rows.iter()
        .map(|row| {
            Ok(QueueMessage {
                msg_id: row.try_get("msg_id")?,
                read_ct: row.try_get("read_ct")?,
                message: row.try_get("message")?,
            })
        })
        .collect()
}

/// Deletes a message (the job is finished). `false` if it was already gone.
///
/// # Errors
///
/// Database errors; redact before logging.
pub async fn delete(pool: &PgPool, queue: Queue, msg_id: i64) -> Result<bool, sqlx::Error> {
    sqlx::query("SELECT pgmq.delete($1::text, $2::bigint) AS deleted")
        .bind(queue.as_str())
        .bind(msg_id)
        .fetch_one(pool)
        .await?
        .try_get("deleted")
}

/// Makes a message visible again `vt_secs` seconds from now (retry
/// backoff). `false` if the message is gone.
///
/// # Errors
///
/// Database errors; redact before logging.
pub async fn set_vt(
    pool: &PgPool,
    queue: Queue,
    msg_id: i64,
    vt_secs: i32,
) -> Result<bool, sqlx::Error> {
    let rows = sqlx::query("SELECT msg_id FROM pgmq.set_vt($1::text, $2::bigint, $3::integer)")
        .bind(queue.as_str())
        .bind(msg_id)
        .bind(vt_secs)
        .fetch_all(pool)
        .await?;
    Ok(!rows.is_empty())
}

/// Moves a message to `queue`'s dead-letter queue: sends `json` (the
/// original message wrapped with the failure code) there and deletes the
/// original, in one transaction. Returns the dead-letter message id.
///
/// # Errors
///
/// [`DeadLetterError`].
pub async fn dead_letter(
    pool: &PgPool,
    queue: Queue,
    msg_id: i64,
    json: &str,
) -> Result<i64, DeadLetterError> {
    let dlq = queue
        .dead_letter()
        .ok_or(DeadLetterError::NoDeadLetterQueue)?;
    let mut tx = pool.begin().await?;
    let dead_id = send(&mut *tx, dlq, json).await?;
    sqlx::query("SELECT pgmq.delete($1::text, $2::bigint)")
        .bind(queue.as_str())
        .bind(msg_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(dead_id)
}

/// Messages currently in `queue` (visible or hidden).
///
/// # Errors
///
/// Database errors; redact before logging.
pub async fn length(pool: &PgPool, queue: Queue) -> Result<i64, sqlx::Error> {
    sqlx::query("SELECT queue_length FROM pgmq.metrics($1::text)")
        .bind(queue.as_str())
        .fetch_one(pool)
        .await?
        .try_get("queue_length")
}

#[cfg(test)]
mod tests {
    use super::Queue;

    #[test]
    fn names_match_the_migration_and_dead_letters_pair_up() {
        let names: Vec<_> = Queue::ALL.iter().map(|q| q.as_str()).collect();
        assert_eq!(
            names,
            [
                "search_jobs",
                "refresh_jobs",
                "search_jobs_dlq",
                "refresh_jobs_dlq"
            ]
        );
        assert_eq!(Queue::SearchJobs.dead_letter(), Some(Queue::SearchJobsDlq));
        assert_eq!(
            Queue::RefreshJobs.dead_letter(),
            Some(Queue::RefreshJobsDlq)
        );
        assert_eq!(Queue::SearchJobsDlq.dead_letter(), None);
        assert_eq!(Queue::RefreshJobsDlq.dead_letter(), None);
    }
}
