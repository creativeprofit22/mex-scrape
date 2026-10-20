//! The queue consumer: `read → decode → handle → delete`, one message at a
//! time (ADR 0011).
//!
//! - A read hides the message for `JOB_VISIBILITY_SECS`; a worker that dies
//!   mid-job lets it reappear with a higher `read_ct`.
//! - Malformed messages and [`JobError::Permanent`] failures go to the
//!   dead-letter queue at once.
//! - [`JobError::Transient`] failures are retried with exponential backoff
//!   (`set_vt`: 30 s · 2^(attempt−1), at most 15 min) until `read_ct`
//!   reaches `JOB_MAX_ATTEMPTS`, then dead-lettered.
//! - [`JobError::Refused`] finishes the job (a search becomes `refused`)
//!   and deletes the message: a refusal is never retried.
//!
//! Dead letters carry `{error_code, attempts, queue, message}`.

use std::time::Duration;

use brightdata::Transport;
use brightdata::runs::RunStore;
use db::DbError;
use db::queue::{self, DeadLetterError, Queue};
use serde_json::{Value, json};
use tokio::sync::watch;
use tracing::Instrument as _;

use super::{JobError, JobMessage, Jobs};
use crate::clock::JobClock;

/// First retry delay.
pub const BASE_BACKOFF_SECS: i32 = 30;
/// Longest retry delay.
pub const MAX_BACKOFF_SECS: i32 = 900;

/// What to do with a handled message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Finished (done or refused): delete it.
    Delete,
    /// Make it visible again after `delay_secs`.
    Retry {
        /// Backoff delay.
        delay_secs: i32,
    },
    /// Move it to the dead-letter queue.
    DeadLetter {
        /// Fixed failure code.
        code: &'static str,
    },
}

/// The retry delay after the `read_ct`-th attempt failed.
#[must_use]
pub fn backoff_secs(read_ct: i32) -> i32 {
    let exp = u32::try_from(read_ct.saturating_sub(1).clamp(0, 10)).unwrap_or(0);
    BASE_BACKOFF_SECS
        .saturating_mul(2_i32.saturating_pow(exp))
        .min(MAX_BACKOFF_SECS)
}

/// Decides a handled message's fate from the job result and its attempt
/// count.
#[must_use]
pub fn decide(outcome: Result<(), JobError>, read_ct: i32, max_attempts: i32) -> Decision {
    match outcome {
        Ok(()) | Err(JobError::Refused(_)) => Decision::Delete,
        Err(JobError::Permanent(code)) => Decision::DeadLetter { code },
        Err(JobError::Transient(code)) if read_ct >= max_attempts => Decision::DeadLetter { code },
        Err(JobError::Transient(_)) => Decision::Retry {
            delay_secs: backoff_secs(read_ct),
        },
    }
}

/// Consumer settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConsumerConfig {
    /// Visibility timeout of a read, in seconds.
    pub visibility_secs: i32,
    /// Reads before a failing message is dead-lettered.
    pub max_attempts: i32,
    /// Sleep when both queues are empty.
    pub poll_idle: Duration,
}

/// One processed message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Processed {
    /// The pgmq message id.
    pub msg_id: i64,
    /// What was done with it.
    pub decision: Decision,
}

fn dead_letter_body(code: &str, read_ct: i32, queue: Queue, original: &str) -> String {
    let message: Value = serde_json::from_str(original).unwrap_or(Value::Null);
    json!({
        "error_code": code,
        "attempts": read_ct,
        "queue": queue.as_str(),
        "message": message,
    })
    .to_string()
}

/// Reads and processes at most one message from `queue`. `None` when the
/// queue had nothing visible.
///
/// # Errors
///
/// Queue errors (the message then reappears after its visibility
/// timeout); redact before logging.
pub async fn process_one<T: Transport, S: RunStore, C: JobClock>(
    jobs: &Jobs<'_, T, S, C>,
    queue: Queue,
    config: ConsumerConfig,
) -> Result<Option<Processed>, DbError> {
    let pool = jobs.ctx.pool;
    let Some(msg) = queue::read(pool, queue, config.visibility_secs, 1)
        .await?
        .into_iter()
        .next()
    else {
        return Ok(None);
    };
    let decision = match JobMessage::parse(&msg.message) {
        Err(_) => Decision::DeadLetter {
            code: "invalid_message",
        },
        // Read too often without finishing (e.g. the worker kept dying
        // mid-job): give up before running it again.
        Ok(job) if msg.read_ct > config.max_attempts => {
            let error = JobError::Transient("attempts_exhausted");
            jobs.give_up(&job, error).await;
            Decision::DeadLetter { code: error.code() }
        }
        Ok(job) => {
            let span = tracing::info_span!(
                "job",
                kind = job.kind(),
                msg_id = msg.msg_id,
                attempt = msg.read_ct,
                tenant = %job.tenant(),
            );
            let outcome = jobs.handle(queue, msg.msg_id, &job).instrument(span).await;
            let decision = decide(outcome, msg.read_ct, config.max_attempts);
            if let Err(error) = outcome
                && matches!(decision, Decision::Delete | Decision::DeadLetter { .. })
            {
                jobs.give_up(&job, error).await;
            }
            match outcome {
                Ok(()) => tracing::info!(
                    kind = job.kind(),
                    msg_id = msg.msg_id,
                    tenant = %job.tenant(),
                    "job finished"
                ),
                Err(error) => tracing::warn!(
                    kind = job.kind(),
                    msg_id = msg.msg_id,
                    tenant = %job.tenant(),
                    attempt = msg.read_ct,
                    error = %error,
                    "job did not finish"
                ),
            }
            decision
        }
    };
    match decision {
        Decision::Delete => {
            queue::delete(pool, queue, msg.msg_id).await?;
        }
        Decision::Retry { delay_secs } => {
            queue::set_vt(pool, queue, msg.msg_id, delay_secs).await?;
        }
        Decision::DeadLetter { code } => {
            let body = dead_letter_body(code, msg.read_ct, queue, &msg.message);
            match queue::dead_letter(pool, queue, msg.msg_id, &body).await {
                Ok(dead_id) => tracing::warn!(
                    queue = queue.as_str(),
                    msg_id = msg.msg_id,
                    dead_letter_id = dead_id,
                    error_code = code,
                    "job dead-lettered"
                ),
                Err(DeadLetterError::Database(error)) => return Err(error),
                // Work queues always have one; nothing to move otherwise.
                Err(DeadLetterError::NoDeadLetterQueue) => {}
            }
        }
    }
    Ok(Some(Processed {
        msg_id: msg.msg_id,
        decision,
    }))
}

/// Consumes `search_jobs` then `refresh_jobs`, one message at a time,
/// until `shutdown` becomes true. A job in progress is finished first.
pub async fn run<T: Transport, S: RunStore, C: JobClock>(
    jobs: &Jobs<'_, T, S, C>,
    config: ConsumerConfig,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        let mut worked = false;
        for queue in [Queue::SearchJobs, Queue::RefreshJobs] {
            if *shutdown.borrow() {
                return;
            }
            match process_one(jobs, queue, config).await {
                Ok(Some(_)) => worked = true,
                Ok(None) => {}
                Err(error) => {
                    let message = jobs.ctx.redactor.sanitize(&error.to_string());
                    tracing::warn!(queue = queue.as_str(), error = %message, "queue read failed");
                }
            }
        }
        if !worked {
            tokio::select! {
                () = tokio::time::sleep(config.poll_idle) => {}
                changed = shutdown.changed() => {
                    if changed.is_err() {
                        return;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_up_to_fifteen_minutes() {
        let delays: Vec<_> = (1..=7).map(backoff_secs).collect();
        assert_eq!(delays, [30, 60, 120, 240, 480, 900, 900]);
        assert_eq!(backoff_secs(0), 30);
        assert_eq!(backoff_secs(i32::MAX), 900);
    }

    #[test]
    fn decisions_follow_the_error_class_and_attempts() {
        assert_eq!(decide(Ok(()), 1, 5), Decision::Delete);
        assert_eq!(
            decide(Err(JobError::Refused("kill_switch")), 1, 5),
            Decision::Delete
        );
        assert_eq!(
            decide(Err(JobError::Permanent("unknown_search")), 1, 5),
            Decision::DeadLetter {
                code: "unknown_search"
            }
        );
        assert_eq!(
            decide(Err(JobError::Transient("storage")), 2, 5),
            Decision::Retry { delay_secs: 60 }
        );
        assert_eq!(
            decide(Err(JobError::Transient("storage")), 5, 5),
            Decision::DeadLetter { code: "storage" }
        );
    }

    #[test]
    fn dead_letters_keep_the_original_message() {
        let body = dead_letter_body("storage", 5, Queue::SearchJobs, r#"{"kind":"search"}"#);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["error_code"], "storage");
        assert_eq!(v["attempts"], 5);
        assert_eq!(v["queue"], "search_jobs");
        assert_eq!(v["message"]["kind"], "search");
        let v: Value =
            serde_json::from_str(&dead_letter_body("x", 1, Queue::RefreshJobs, "not json"))
                .unwrap();
        assert_eq!(v["message"], Value::Null);
    }
}
