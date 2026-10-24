//! The `category_browse` job: fetches the first `CATEGORY_MAX_PAGES_PER_RUN`
//! configured category listings of one category-only retailer (ADR 0008)
//! through the Browser API in one gated run (tenant `system`), then
//! matches every offer to Amazon (which records its price in
//! `price_observations`) and tracks accepted matches.

use app::AmazonSourceError;
use brightdata::runs::{PageAccess, RunScope, RunStore};
use brightdata::{TargetUrl, Transport};
use domain::{MatchStatus, TenantId, UnixMillis};
use retailers::ParseKind;
use retailers::category::CategoryListing;

use super::{JobContext, JobError, pages};
use crate::clock::JobClock;
use crate::offer_matching::{MatchOutcome, OfferMatchError, match_and_store};

/// Run scope job name for category browses.
pub const CATEGORY_JOB: &str = "category";

/// The listing URLs of `retailer` browsed by one run: the first
/// `max_pages`, in configuration order.
#[must_use]
pub fn pages_for(listings: &[CategoryListing], retailer: &str, max_pages: usize) -> Vec<TargetUrl> {
    listings
        .iter()
        .filter(|l| l.retailer == retailer)
        .take(max_pages)
        .map(|l| l.url.clone())
        .collect()
}

/// Browses `retailer`'s category listings.
///
/// # Errors
///
/// [`JobError`]: storage is transient, gate refusals are refusals.
pub async fn run<T: Transport, S: RunStore, C: JobClock>(
    ctx: &JobContext<'_, T, S, C>,
    msg_id: i64,
    retailer: &str,
) -> Result<(), JobError> {
    let urls = pages_for(ctx.listings, retailer, ctx.config.category_max_pages);
    if urls.is_empty() {
        tracing::info!(
            retailer,
            "no category listings configured; nothing to browse"
        );
        return Ok(());
    }
    let tenant = TenantId::system();
    let scope = RunScope::new(tenant.as_str(), CATEGORY_JOB)
        .map_err(|_| JobError::Permanent("invalid_scope"))?;
    let collected =
        pages::fetch(ctx, &scope, &urls, PageAccess::Browser, ParseKind::Search).await?;
    let amazon = ctx.amazon_for(&tenant);
    let now = UnixMillis(ctx.now_ms());
    let (mut matched, mut tracked) = (0_usize, 0_usize);
    let mut amazon_refused = false;
    for (id, offer) in &collected.offers {
        if amazon_refused {
            // Keep the price history even when Amazon is off limits.
            ctx.prices
                .record(offer)
                .await
                .map_err(|_| JobError::Transient("storage"))?;
            continue;
        }
        match match_and_store(
            &amazon,
            ctx.matches,
            ctx.products,
            ctx.prices,
            offer,
            ctx.match_config,
            now,
        )
        .await
        {
            Ok(MatchOutcome::Stored(m)) => {
                matched += 1;
                if m.status == MatchStatus::Accepted {
                    db::tracking::track(ctx.pool, id, offer.retailer_sku.as_str(), now.0)
                        .await
                        .map_err(|e| ctx.storage(&e))?;
                    tracked += 1;
                }
            }
            Ok(MatchOutcome::KeptRejected | MatchOutcome::NoCandidate) => {}
            Err(OfferMatchError::Amazon(AmazonSourceError::Refused)) => {
                tracing::info!(retailer, "Amazon calls refused; recording prices only");
                amazon_refused = true;
            }
            Err(OfferMatchError::Amazon(_)) => {}
            Err(OfferMatchError::Storage) => return Err(JobError::Transient("storage")),
        }
    }
    collected
        .metrics
        .emit(CATEGORY_JOB, msg_id, tenant.as_str(), ctx.config.cost_rates);
    tracing::info!(
        retailer,
        pages = urls.len(),
        offers = collected.offers.len(),
        matched,
        tracked,
        "category browse finished"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_are_capped_per_retailer_in_file_order() {
        let listings = retailers::category::parse_listings(include_str!(
            "../../../../config/category_listings.json"
        ))
        .unwrap();
        let all_sears: Vec<_> = listings
            .iter()
            .filter(|l| l.retailer == "sears_mx")
            .map(|l| l.url.clone())
            .collect();
        assert!(all_sears.len() >= 2);
        assert_eq!(pages_for(&listings, "sears_mx", 1), all_sears[..1]);
        assert_eq!(pages_for(&listings, "sears_mx", 100), all_sears);
        assert!(pages_for(&listings, "walmart_mx", 4).is_empty());
    }
}
