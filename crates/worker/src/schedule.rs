//! Cron schedules (UTC) that enqueue scheduled jobs on `refresh_jobs`
//! (ADR 0011):
//!
//! - `REFRESH_CRON`: `product_refresh` batches of tracked listings not
//!   refreshed for [`REFRESH_MIN_AGE_MS`], at most
//!   [`MAX_REFRESH_BATCHES_PER_TICK`] batches per tick. A tick is skipped
//!   while `refresh_jobs` still holds messages, so slow runs never pile up.
//! - `CATEGORY_BROWSE_CRON`: one `category_browse` per retailer with
//!   configured category listings.
//!
//! The loop reads time from an injected clock and only sleeps on tokio
//! time; the planning functions are pure.

use std::collections::BTreeMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use croner::Cron;
use db::PgPool;
use db::queue::{self, Queue};
use domain::RetailerSku;
use http_guard::Redactor;
use retailers::category::CategoryListing;
use tokio::sync::watch;

use crate::clock::JobClock;
use crate::jobs::{JobMessage, MAX_REFRESH_SKUS};

/// A listing is due once its last refresh is this old (1 h).
pub const REFRESH_MIN_AGE_MS: i64 = 3_600_000;
/// Most refresh batches enqueued by one tick.
pub const MAX_REFRESH_BATCHES_PER_TICK: usize = 4;
/// Longest sleep between clock checks, so a changed clock is noticed.
const MAX_SLEEP: Duration = Duration::from_secs(60);

/// The first tick of `cron` strictly after `now_ms` (epoch ms, UTC).
#[must_use]
pub fn next_tick(cron: &Cron, now_ms: i64) -> Option<i64> {
    let now: DateTime<Utc> = DateTime::from_timestamp_millis(now_ms)?;
    cron.find_next_occurrence(&now, false)
        .ok()
        .map(|t| t.timestamp_millis())
}

/// Groups due `(retailer, sku)` listings into `product_refresh` messages
/// of at most `batch` SKUs per retailer, retailers sorted, at most
/// [`MAX_REFRESH_BATCHES_PER_TICK`] messages. Unknown retailers and
/// invalid SKUs are skipped.
#[must_use]
pub fn plan_refresh(due: &[(String, String)], batch: usize) -> Vec<JobMessage> {
    let batch = batch.clamp(1, MAX_REFRESH_SKUS);
    let mut by_retailer: BTreeMap<&'static str, Vec<RetailerSku>> = BTreeMap::new();
    for (retailer, sku) in due {
        let (Some(adapter), Ok(sku)) = (retailers::adapter_for(retailer), RetailerSku::new(sku))
        else {
            continue;
        };
        by_retailer.entry(adapter.info().id).or_default().push(sku);
    }
    by_retailer
        .into_iter()
        .flat_map(|(retailer, skus)| {
            skus.chunks(batch)
                .map(|chunk| JobMessage::ProductRefresh {
                    retailer,
                    skus: chunk.to_vec(),
                })
                .collect::<Vec<_>>()
        })
        .take(MAX_REFRESH_BATCHES_PER_TICK)
        .collect()
}

/// One `category_browse` per retailer with listings, sorted by retailer.
#[must_use]
pub fn plan_category(listings: &[CategoryListing]) -> Vec<JobMessage> {
    let mut retailers: Vec<&'static str> = listings.iter().map(|l| l.retailer).collect();
    retailers.sort_unstable();
    retailers.dedup();
    retailers
        .into_iter()
        .map(|retailer| JobMessage::CategoryBrowse { retailer })
        .collect()
}

async fn send_all(pool: &PgPool, messages: &[JobMessage]) -> Result<usize, db::DbError> {
    for m in messages {
        queue::send(pool, m.queue(), &m.to_json()).await?;
    }
    Ok(messages.len())
}

/// Enqueues the due refresh batches, unless `refresh_jobs` is not empty.
/// Returns how many messages were sent.
///
/// # Errors
///
/// Database errors; redact before logging.
pub async fn enqueue_refresh(
    pool: &PgPool,
    now_ms: i64,
    batch: usize,
) -> Result<usize, db::DbError> {
    if queue::length(pool, Queue::RefreshJobs).await? > 0 {
        tracing::info!("refresh tick skipped: refresh_jobs is not empty");
        return Ok(0);
    }
    let limit = i64::try_from(batch.saturating_mul(MAX_REFRESH_BATCHES_PER_TICK)).unwrap_or(200);
    let due = db::tracking::due(pool, now_ms - REFRESH_MIN_AGE_MS, limit).await?;
    send_all(pool, &plan_refresh(&due, batch)).await
}

/// Enqueues one category browse per configured retailer.
///
/// # Errors
///
/// Database errors; redact before logging.
pub async fn enqueue_category(
    pool: &PgPool,
    listings: &[CategoryListing],
) -> Result<usize, db::DbError> {
    send_all(pool, &plan_category(listings)).await
}

/// Scheduler settings.
pub struct Schedules<'a> {
    /// Product refresh schedule.
    pub refresh: &'a Cron,
    /// Category browse schedule.
    pub category: &'a Cron,
    /// SKUs per refresh batch.
    pub refresh_batch: usize,
    /// Validated category listings.
    pub listings: &'a [CategoryListing],
}

/// Runs both schedules until `shutdown` becomes true.
pub async fn run<C: JobClock>(
    pool: &PgPool,
    clock: &C,
    schedules: Schedules<'_>,
    redactor: &Redactor,
    mut shutdown: watch::Receiver<bool>,
) {
    let now = || brightdata::runs::Clock::now_ms(clock);
    let mut next_refresh = next_tick(schedules.refresh, now());
    let mut next_category = next_tick(schedules.category, now());
    loop {
        if *shutdown.borrow() {
            return;
        }
        let t = now();
        if next_refresh.is_some_and(|tick| tick <= t) {
            match enqueue_refresh(pool, t, schedules.refresh_batch).await {
                Ok(sent) => tracing::info!(schedule = "refresh", sent, "schedule fired"),
                Err(e) => {
                    let message = redactor.sanitize(&e.to_string());
                    tracing::warn!(schedule = "refresh", error = %message, "schedule failed");
                }
            }
            next_refresh = next_tick(schedules.refresh, t);
        }
        if next_category.is_some_and(|tick| tick <= t) {
            match enqueue_category(pool, schedules.listings).await {
                Ok(sent) => tracing::info!(schedule = "category_browse", sent, "schedule fired"),
                Err(e) => {
                    let message = redactor.sanitize(&e.to_string());
                    tracing::warn!(schedule = "category_browse", error = %message, "schedule failed");
                }
            }
            next_category = next_tick(schedules.category, t);
        }
        let wake = [next_refresh, next_category].into_iter().flatten().min();
        let sleep = wake.map_or(MAX_SLEEP, |w| {
            Duration::from_millis(u64::try_from(w - now()).unwrap_or(0)).min(MAX_SLEEP)
        });
        tokio::select! {
            () = tokio::time::sleep(sleep) => {}
            changed = shutdown.changed() => {
                if changed.is_err() {
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr as _;

    use super::*;

    // 2026-10-19T00:00:00Z
    const MIDNIGHT: i64 = 1_792_368_000_000;
    const HOUR: i64 = 3_600_000;

    #[test]
    fn next_tick_is_strictly_after_now_in_utc() {
        let every6 = Cron::from_str("0 */6 * * *").unwrap();
        assert_eq!(next_tick(&every6, MIDNIGHT), Some(MIDNIGHT + 6 * HOUR));
        assert_eq!(next_tick(&every6, MIDNIGHT - 1), Some(MIDNIGHT));
        let daily = Cron::from_str("30 3 * * *").unwrap();
        assert_eq!(
            next_tick(&daily, MIDNIGHT + 4 * HOUR),
            Some(MIDNIGHT + 24 * HOUR + 3 * HOUR + 30 * 60_000)
        );
    }

    #[test]
    fn refresh_batches_group_by_retailer_and_cap() {
        let due: Vec<(String, String)> = [
            ("walmart_mx", "1"),
            ("liverpool", "a"),
            ("walmart_mx", "2"),
            ("walmart_mx", "3"),
            ("nope", "x"),
        ]
        .iter()
        .map(|(r, s)| ((*r).to_owned(), (*s).to_owned()))
        .collect();
        let plan = plan_refresh(&due, 2);
        let shape: Vec<_> = plan
            .iter()
            .map(|m| match m {
                JobMessage::ProductRefresh { retailer, skus } => (
                    *retailer,
                    skus.iter()
                        .map(|s| s.as_str().to_owned())
                        .collect::<Vec<_>>(),
                ),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(
            shape,
            [
                ("liverpool", vec!["a".to_owned()]),
                ("walmart_mx", vec!["1".to_owned(), "2".to_owned()]),
                ("walmart_mx", vec!["3".to_owned()]),
            ]
        );
        let many: Vec<_> = (0..20)
            .map(|i| ("walmart_mx".to_owned(), i.to_string()))
            .collect();
        assert_eq!(plan_refresh(&many, 1).len(), MAX_REFRESH_BATCHES_PER_TICK);
    }

    #[test]
    fn one_category_browse_per_retailer() {
        let listings = retailers::category::parse_listings(include_str!(
            "../../../config/category_listings.json"
        ))
        .unwrap();
        assert_eq!(
            plan_category(&listings),
            [
                JobMessage::CategoryBrowse {
                    retailer: "sanborns"
                },
                JobMessage::CategoryBrowse {
                    retailer: "sears_mx"
                },
            ]
        );
    }
}
