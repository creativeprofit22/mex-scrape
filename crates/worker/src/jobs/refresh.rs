//! The `product_refresh` job: fetches the product pages of a batch of
//! tracked SKUs of one retailer in one gated run (tenant `system`), records
//! each parsed offer in `price_observations` and marks the listing
//! refreshed; every SKU of the batch is marked attempted.

use brightdata::Transport;
use brightdata::runs::{PageAccess, RunScope, RunStore};
use domain::{RetailerSku, TenantId};
use retailers::{AccessMode, PageKind, ParseKind};

use super::{JobContext, JobError, pages};
use crate::clock::JobClock;

/// Run scope job name for refreshes.
pub const REFRESH_JOB: &str = "refresh";

/// Refreshes `skus` at `retailer`, then marks every SKU of the batch
/// attempted (whether or not an offer parsed) so listings that keep failing
/// rotate behind the others in `db::tracking::due`.
///
/// # Errors
///
/// [`JobError`]: storage is transient, gate refusals are refusals.
pub async fn run<T: Transport, S: RunStore, C: JobClock>(
    ctx: &JobContext<'_, T, S, C>,
    msg_id: i64,
    retailer: &str,
    skus: &[RetailerSku],
) -> Result<(), JobError> {
    let outcome = refresh(ctx, msg_id, retailer, skus).await;
    let attempted: Vec<&str> = skus.iter().map(RetailerSku::as_str).collect();
    if let Err(e) = db::tracking::mark_attempted(ctx.pool, retailer, &attempted, ctx.now_ms()).await
    {
        // Logged only: failing the job here would retry it and re-spend the
        // paid run; the batch just comes due again sooner.
        let _logged = ctx.storage(&e);
    }
    outcome
}

async fn refresh<T: Transport, S: RunStore, C: JobClock>(
    ctx: &JobContext<'_, T, S, C>,
    msg_id: i64,
    retailer: &str,
    skus: &[RetailerSku],
) -> Result<(), JobError> {
    let adapter =
        retailers::adapter_for(retailer).ok_or(JobError::Permanent("unknown_retailer"))?;
    let info = adapter.info();
    let urls: Vec<_> = skus
        .iter()
        .filter_map(|sku| match adapter.product_url(sku) {
            Ok(url) => Some(url),
            Err(_) => {
                tracing::warn!(
                    retailer,
                    sku = sku.as_str(),
                    "tracked SKU has an invalid shape"
                );
                None
            }
        })
        .collect();
    if urls.is_empty() {
        return Err(JobError::Permanent("no_valid_skus"));
    }
    let access = match info.access_for(PageKind::Product) {
        AccessMode::Unlocker => PageAccess::Unlocker,
        AccessMode::Browser => PageAccess::Browser,
    };
    let tenant = TenantId::system();
    let scope = RunScope::new(tenant.as_str(), REFRESH_JOB)
        .map_err(|_| JobError::Permanent("invalid_scope"))?;
    let collected = pages::fetch(ctx, &scope, &urls, access, ParseKind::Product).await?;
    let mut recorded = 0_usize;
    for (id, offer) in &collected.offers {
        ctx.prices
            .record(offer)
            .await
            .map_err(|_| JobError::Transient("storage"))?;
        db::tracking::mark_refreshed(ctx.pool, id, offer.retailer_sku.as_str(), ctx.now_ms())
            .await
            .map_err(|e| ctx.storage(&e))?;
        recorded += 1;
    }
    collected
        .metrics
        .emit(REFRESH_JOB, msg_id, tenant.as_str(), ctx.config.cost_rates);
    tracing::info!(
        retailer,
        skus = skus.len(),
        recorded,
        "product refresh finished"
    );
    Ok(())
}
