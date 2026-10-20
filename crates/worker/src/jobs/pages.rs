//! Fetching retailer pages for a job: one gated Bright Data run, its
//! pages parsed by an [`OfferCollector`], and its recorded access outcomes
//! folded into the job metrics per retailer.

use brightdata::runs::{AccessOutcome, PageAccess, RunScope, RunStore};
use brightdata::{CountryCode, TargetUrl, Transport};
use domain::UnixMillis;
use retailers::ParseKind;

use super::{JobContext, JobError, run_error};
use crate::clock::JobClock;
use crate::sinks::{Collected, OfferCollector};

fn access_outcome(stored: &str) -> Option<AccessOutcome> {
    [
        AccessOutcome::Ok,
        AccessOutcome::BlockedNeedsKyc,
        AccessOutcome::SkippedRobots,
        AccessOutcome::Failed,
    ]
    .into_iter()
    .find(|o| o.as_str() == stored)
}

/// Fetches `urls` as one run of `scope` (all gates and caps apply), parses
/// every page as `kind` and returns the offers and metrics.
///
/// # Errors
///
/// The run error classified by [`run_error`].
pub async fn fetch<T: Transport, S: RunStore, C: JobClock>(
    ctx: &JobContext<'_, T, S, C>,
    scope: &RunScope,
    urls: &[TargetUrl],
    access: PageAccess,
    kind: ParseKind,
) -> Result<Collected, JobError> {
    let sink = OfferCollector::new(kind, access, UnixMillis(ctx.now_ms()), ctx.health);
    let outcome = ctx
        .runner
        .start_unlocker_run(scope, urls, Some(CountryCode::MX), access, &sink)
        .await
        .map_err(|error| {
            let message = ctx.redactor.sanitize(&error.to_string());
            tracing::info!(error = %message, "Bright Data run not started");
            run_error(&error)
        })?;
    let mut collected = sink.into_collected();
    match db::brightdata_runs::access_outcomes_for_run(ctx.pool, outcome.run_id.0).await {
        Ok(rows) => {
            for (host, stored) in rows {
                if let (Some(info), Some(o)) =
                    (retailers::retailer_for_host(&host), access_outcome(&stored))
                {
                    collected.metrics.access(info.id, o);
                }
            }
        }
        // Metrics never fail a job.
        Err(error) => {
            let message = ctx.redactor.sanitize(&error.to_string());
            tracing::warn!(error = %message, "reading run access outcomes failed");
        }
    }
    tracing::info!(
        run_id = outcome.run_id.0,
        status = outcome.status.as_str(),
        pages = outcome.row_count,
        failed = outcome.failed_count,
        offers = collected.offers.len(),
        "Bright Data run finished"
    );
    Ok(collected)
}

#[cfg(test)]
mod tests {
    use super::access_outcome;
    use brightdata::runs::AccessOutcome;

    #[test]
    fn stored_outcomes_parse_back() {
        for o in [
            AccessOutcome::Ok,
            AccessOutcome::BlockedNeedsKyc,
            AccessOutcome::SkippedRobots,
            AccessOutcome::Failed,
        ] {
            assert_eq!(access_outcome(o.as_str()), Some(o));
        }
        assert_eq!(access_outcome("other"), None);
    }
}
