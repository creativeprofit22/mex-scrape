//! The search job: fans a stored search out to every retailer adapter in
//! one Web Unlocker run (per access mode), then matches the first offers of
//! each retailer to Amazon and prices them (offers, fees, ROI), writing
//! `search_retailers` and `search_results` for the API.
//!
//! Stores without keyword search (ADR 0008) get a `category_only` outcome
//! instead of silently returning nothing; scheduled category browsing
//! covers them. Accepted matches are tracked for scheduled refreshes.

use std::collections::{BTreeMap, BTreeSet};

use app::{AmazonSource, AmazonSourceError, Fulfillment};
use brightdata::runs::{PageAccess, RunScope, RunStore};
use brightdata::{TargetUrl, Transport};
use db::searches::{self, RetailerOutcomeRow, SearchResultRow};
use domain::{
    MatchStatus, Offer, PriceMxn, ProductMatch, RetailerSearchOutcome, RoiInput, SearchQuery,
    SearchStatus, TenantId, UnixMillis, calculate_roi,
};
use retailers::{AccessMode, AdapterError, PageKind, ParseKind};

use super::{JobContext, JobError, pages};
use crate::clock::JobClock;
use crate::metrics::RetailerMetrics;
use crate::offer_matching::{MatchOutcome, OfferMatchError, match_and_store};

/// Run scope job name for searches.
pub const SEARCH_JOB: &str = "search";
/// Longest stored result title (`search_results.title`).
const MAX_TITLE_CHARS: usize = 500;
/// Longest stored result URL (`search_results.url`).
const MAX_URL_CHARS: usize = 2048;

/// How the Amazon side of a search went.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AmazonHealth {
    /// A call was refused by the spending guard (kill switch, daily cap).
    pub refused: bool,
    /// A call failed for another reason.
    pub failed: bool,
}

/// One retailer's outcome from its offers and metrics.
#[must_use]
pub fn retailer_outcome(offers: usize, m: &RetailerMetrics) -> RetailerSearchOutcome {
    if offers > 0 {
        RetailerSearchOutcome::Ok
    } else if m.blocked_needs_kyc > 0 {
        RetailerSearchOutcome::BlockedNeedsKyc
    } else if m.skipped_robots > 0 {
        RetailerSearchOutcome::SkippedRobots
    } else if m.parse_ok + m.parse_failed > 0 {
        RetailerSearchOutcome::NoOffers
    } else {
        RetailerSearchOutcome::Failed
    }
}

/// The terminal status and reason code of a finished search.
#[must_use]
pub fn final_status(
    outcomes: &[RetailerSearchOutcome],
    amazon: AmazonHealth,
) -> (SearchStatus, &'static str) {
    let fetched = outcomes.iter().any(|o| {
        matches!(
            o,
            RetailerSearchOutcome::Ok | RetailerSearchOutcome::NoOffers
        )
    });
    let failed = outcomes.iter().any(|o| o.is_failure());
    if failed && !fetched {
        (SearchStatus::Failed, "all_retailers_failed")
    } else if amazon.refused {
        (SearchStatus::Partial, "amazon_refused")
    } else if failed {
        (SearchStatus::Partial, "retailers_failed")
    } else if amazon.failed {
        (SearchStatus::Partial, "amazon_unavailable")
    } else {
        (SearchStatus::Done, "")
    }
}

fn page_access(mode: AccessMode) -> PageAccess {
    match mode {
        AccessMode::Unlocker => PageAccess::Unlocker,
        AccessMode::Browser => PageAccess::Browser,
    }
}

/// Amazon price, fees and ROI of an offer's match. Missing parts stay
/// `None`; a refusal or failure is noted in `health`.
async fn price(
    amazon: &dyn AmazonSource,
    m: &ProductMatch,
    offer: &Offer,
    prep_fee: PriceMxn,
    shipping: PriceMxn,
    health: &mut AmazonHealth,
) -> (
    Option<PriceMxn>,
    Option<PriceMxn>,
    Option<domain::RoiResult>,
) {
    let mut note = |e: AmazonSourceError| {
        if e == AmazonSourceError::Refused {
            health.refused = true;
        } else {
            health.failed = true;
        }
    };
    let sell = match amazon.offers(&m.asin).await {
        Ok(summary) => summary.buy_box_mxn.or(summary.lowest_new_mxn),
        Err(e) => {
            note(e);
            None
        }
    };
    let Some(sell) = sell else {
        return (None, None, None);
    };
    let fees = match amazon.fees(&m.asin, sell, Fulfillment::Fba).await {
        Ok(fees) => fees.total_mxn,
        Err(e) => {
            note(e);
            return (Some(sell), None, None);
        }
    };
    let zero = PriceMxn::from_centavos(0).ok();
    let roi = zero.and_then(|zero| {
        calculate_roi(&RoiInput {
            sell_price: sell,
            buy_price: offer.price_mxn,
            amazon_fees: fees,
            prep_fee,
            shipping,
            monthly_fixed: zero,
            units_per_month: 1,
        })
        .ok()
    });
    (Some(sell), Some(fees), roi)
}

fn result_row(
    retailer: &str,
    offer: &Offer,
    found: Option<&ProductMatch>,
    priced: (
        Option<PriceMxn>,
        Option<PriceMxn>,
        Option<domain::RoiResult>,
    ),
) -> SearchResultRow {
    let (sell, fees, roi) = priced;
    SearchResultRow {
        retailer: retailer.to_owned(),
        retailer_sku: offer.retailer_sku.as_str().to_owned(),
        title: offer.title.chars().take(MAX_TITLE_CHARS).collect(),
        url: offer.url.clone(),
        price_mxn: offer.price_mxn.centavos(),
        asin: found.map(|m| m.asin.as_str().to_owned()),
        match_status: found.map(|m| m.status.as_str().to_owned()),
        confidence_bp: found.map(|m| i32::from(m.confidence.bp())),
        amazon_price_mxn: sell.map(PriceMxn::centavos),
        fees_mxn: fees.map(PriceMxn::centavos),
        profit_centavos: roi.map(|r| r.profit_centavos),
        roi_bp: roi.and_then(|r| r.roi_bp),
        margin_bp: roi.and_then(|r| r.margin_bp),
        availability: offer.availability.as_str().to_owned(),
        membership_required: offer.membership_required,
    }
}

/// Runs search `search_id` owned by `tenant`.
///
/// # Errors
///
/// [`JobError`]: unknown or foreign searches are permanent; storage is
/// transient; gate refusals are refusals (the consumer then finishes the
/// search `refused`).
pub async fn run<T: Transport, S: RunStore, C: JobClock>(
    ctx: &JobContext<'_, T, S, C>,
    msg_id: i64,
    search_id: i64,
    tenant: &TenantId,
) -> Result<(), JobError> {
    let pool = ctx.pool;
    let row = searches::load_search(pool, search_id)
        .await
        .map_err(|e| ctx.storage(&e))?
        .ok_or(JobError::Permanent("unknown_search"))?;
    if row.tenant_id != tenant.as_str() {
        return Err(JobError::Permanent("tenant_mismatch"));
    }
    if SearchStatus::parse(&row.status).is_some_and(SearchStatus::is_terminal) {
        return Ok(());
    }
    let query = SearchQuery::parse(&row.query).map_err(|_| JobError::Permanent("invalid_query"))?;
    searches::mark_running(pool, search_id, ctx.now_ms())
        .await
        .map_err(|e| ctx.storage(&e))?;

    // Build every retailer's search URL; stores without keyword search are
    // category-only.
    let mut targets: BTreeMap<AccessMode, Vec<TargetUrl>> = BTreeMap::new();
    let mut searched: BTreeSet<&'static str> = BTreeSet::new();
    let mut outcomes: BTreeMap<&'static str, (RetailerSearchOutcome, usize)> = BTreeMap::new();
    for adapter in retailers::adapters() {
        let info = adapter.info();
        match adapter.search_url(query.as_str()) {
            Ok(url) => {
                targets
                    .entry(info.access_for(PageKind::Search))
                    .or_default()
                    .push(url);
                searched.insert(info.id);
            }
            Err(AdapterError::SearchUnavailable) => {
                outcomes.insert(info.id, (RetailerSearchOutcome::CategoryOnly, 0));
            }
            Err(_) => {
                outcomes.insert(info.id, (RetailerSearchOutcome::Failed, 0));
            }
        }
    }

    let scope = RunScope::new(tenant.as_str(), SEARCH_JOB)
        .map_err(|_| JobError::Permanent("invalid_scope"))?;
    let mut collected = crate::sinks::Collected::default();
    for (mode, urls) in &targets {
        let part = pages::fetch(ctx, &scope, urls, page_access(*mode), ParseKind::Search).await?;
        collected.offers.extend(part.offers);
        collected.metrics.merge(&part.metrics);
    }

    let mut by_retailer: BTreeMap<&'static str, Vec<Offer>> = BTreeMap::new();
    for (retailer, offer) in collected.offers {
        by_retailer.entry(retailer).or_default().push(offer);
    }
    for id in &searched {
        let count = by_retailer.get(id).map_or(0, Vec::len);
        let outcome = retailer_outcome(count, &collected.metrics.get(id));
        outcomes.insert(id, (outcome, count));
    }
    for (retailer, (outcome, offers)) in &outcomes {
        let row = RetailerOutcomeRow {
            retailer: (*retailer).to_owned(),
            outcome: outcome.as_str().to_owned(),
            offers: i32::try_from(*offers).unwrap_or(i32::MAX),
        };
        searches::upsert_retailer_outcome(pool, search_id, &row)
            .await
            .map_err(|e| ctx.storage(&e))?;
    }

    let amazon = ctx.amazon_for(tenant);
    let mut health = AmazonHealth::default();
    let now = UnixMillis(ctx.now_ms());
    for (retailer, offers) in &by_retailer {
        let mut seen = BTreeSet::new();
        let picked = offers
            .iter()
            .filter(|o| o.url.chars().count() <= MAX_URL_CHARS)
            .filter(|o| seen.insert(o.retailer_sku.as_str().to_owned()))
            .take(ctx.config.search_max_offers_per_retailer);
        for offer in picked {
            let (found, priced) = if health.refused {
                // No more Amazon calls once refused; still keep the price.
                ctx.prices
                    .record(offer)
                    .await
                    .map_err(|_| JobError::Transient("storage"))?;
                (None, (None, None, None))
            } else {
                match match_and_store(
                    &amazon,
                    ctx.matches,
                    ctx.prices,
                    offer,
                    ctx.match_config,
                    now,
                )
                .await
                {
                    Ok(MatchOutcome::Stored(m)) => {
                        let priced =
                            if matches!(m.status, MatchStatus::Accepted | MatchStatus::NeedsReview)
                            {
                                price(
                                    &amazon,
                                    &m,
                                    offer,
                                    ctx.config.prep_fee,
                                    ctx.config.shipping,
                                    &mut health,
                                )
                                .await
                            } else {
                                (None, None, None)
                            };
                        if m.status == MatchStatus::Accepted {
                            db::tracking::track(pool, retailer, offer.retailer_sku.as_str(), now.0)
                                .await
                                .map_err(|e| ctx.storage(&e))?;
                        }
                        (Some(m), priced)
                    }
                    Ok(MatchOutcome::KeptRejected | MatchOutcome::NoCandidate) => {
                        (None, (None, None, None))
                    }
                    Err(OfferMatchError::Amazon(AmazonSourceError::Refused)) => {
                        health.refused = true;
                        (None, (None, None, None))
                    }
                    Err(OfferMatchError::Amazon(_)) => {
                        health.failed = true;
                        (None, (None, None, None))
                    }
                    Err(OfferMatchError::Storage) => return Err(JobError::Transient("storage")),
                }
            };
            searches::upsert_result(
                pool,
                search_id,
                &result_row(retailer, offer, found.as_ref(), priced),
            )
            .await
            .map_err(|e| ctx.storage(&e))?;
        }
    }

    let all: Vec<_> = outcomes.values().map(|(o, _)| *o).collect();
    let (status, reason) = final_status(&all, health);
    searches::finish(pool, search_id, status.as_str(), reason, ctx.now_ms())
        .await
        .map_err(|e| ctx.storage(&e))?;
    collected
        .metrics
        .emit(SEARCH_JOB, msg_id, tenant.as_str(), ctx.config.cost_rates);
    tracing::info!(
        search_id,
        status = status.as_str(),
        reason,
        "search finished"
    );
    Ok(())
}

/// Finishes a search the consumer gave up on: `refused` for a refusal,
/// otherwise `failed`, with the error code as reason. A search already
/// terminal is left alone.
pub async fn give_up<T: Transport, S: RunStore, C: JobClock>(
    ctx: &JobContext<'_, T, S, C>,
    search_id: i64,
    error: JobError,
) {
    let status = match error {
        JobError::Refused(_) => SearchStatus::Refused,
        JobError::Transient(_) | JobError::Permanent(_) => SearchStatus::Failed,
    };
    if let Err(e) = searches::finish(
        ctx.pool,
        search_id,
        status.as_str(),
        error.code(),
        ctx.now_ms(),
    )
    .await
    {
        let message = ctx.redactor.sanitize(&e.to_string());
        tracing::warn!(search_id, error = %message, "finishing a search failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use RetailerSearchOutcome as O;

    #[test]
    fn retailer_outcome_prefers_offers_then_blocks() {
        let m = |blocked, skipped, parsed| RetailerMetrics {
            blocked_needs_kyc: blocked,
            skipped_robots: skipped,
            parse_failed: parsed,
            ..RetailerMetrics::default()
        };
        assert_eq!(retailer_outcome(2, &m(1, 0, 0)), O::Ok);
        assert_eq!(retailer_outcome(0, &m(1, 1, 0)), O::BlockedNeedsKyc);
        assert_eq!(retailer_outcome(0, &m(0, 1, 0)), O::SkippedRobots);
        assert_eq!(retailer_outcome(0, &m(0, 0, 1)), O::NoOffers);
        assert_eq!(retailer_outcome(0, &m(0, 0, 0)), O::Failed);
    }

    #[test]
    fn final_status_is_partial_or_failed_only_for_real_failures() {
        let fine = AmazonHealth::default();
        assert_eq!(
            final_status(&[O::Ok, O::CategoryOnly, O::NoOffers], fine),
            (SearchStatus::Done, "")
        );
        assert_eq!(
            final_status(&[O::Ok, O::Failed], fine),
            (SearchStatus::Partial, "retailers_failed")
        );
        assert_eq!(
            final_status(&[O::BlockedNeedsKyc, O::CategoryOnly], fine),
            (SearchStatus::Failed, "all_retailers_failed")
        );
        let refused = AmazonHealth {
            refused: true,
            failed: false,
        };
        assert_eq!(
            final_status(&[O::Ok], refused),
            (SearchStatus::Partial, "amazon_refused")
        );
        let failed = AmazonHealth {
            refused: false,
            failed: true,
        };
        assert_eq!(
            final_status(&[O::Ok], failed),
            (SearchStatus::Partial, "amazon_unavailable")
        );
    }
}
