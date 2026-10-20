//! Queries for `searches`, `search_retailers` and `search_results`
//! (migration `0006_jobs.sql`), plus the [`app::SearchRepo`] adapter.
//!
//! A search row and its `search_jobs` message are written in one
//! transaction, so a search is never stored without a job or the other way
//! round. Reads by the API are tenant-scoped. Timestamps cross the boundary
//! as epoch milliseconds and money as integer centavos.

use std::sync::Arc;

use app::{
    Clock, PortFuture, RepoError, ResultMatchView, RetailerOutcomeView, SearchId, SearchRepo,
    SearchResultView, SearchView,
};
use domain::{
    Asin, Availability, Confidence, MatchStatus, PriceMxn, RetailerId, RetailerSearchOutcome,
    RetailerSku, SearchQuery, SearchStatus, TenantId, UnixMillis,
};
use sqlx::Row as _;
use sqlx::postgres::{PgPool, PgRow};

use crate::queue::Queue;

/// A stored search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRow {
    /// Search id.
    pub id: i64,
    /// Owner.
    pub tenant_id: String,
    /// Cleaned query.
    pub query: String,
    /// `queued`, `running`, `done`, `partial`, `failed` or `refused`.
    pub status: String,
    /// Fixed reason code (empty when none).
    pub reason: String,
    /// Creation time (epoch ms).
    pub created_at_ms: i64,
    /// Terminal time (epoch ms).
    pub finished_at_ms: Option<i64>,
}

/// One retailer's outcome in a search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetailerOutcomeRow {
    /// Retailer id.
    pub retailer: String,
    /// Outcome name (see the table CHECK).
    pub outcome: String,
    /// Offers returned.
    pub offers: i32,
}

/// One search result row; the Amazon columns are all `None` when no match
/// was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResultRow {
    /// Retailer id.
    pub retailer: String,
    /// Retailer SKU.
    pub retailer_sku: String,
    /// Listing title.
    pub title: String,
    /// Listing URL.
    pub url: String,
    /// Retailer price in centavos.
    pub price_mxn: i64,
    /// Stock state name (`in_stock`, `out_of_stock`, `pre_order`, `unknown`).
    pub availability: String,
    /// Whether buying needs a paid store membership.
    pub membership_required: bool,
    /// Matched ASIN.
    pub asin: Option<String>,
    /// Match status name.
    pub match_status: Option<String>,
    /// Match confidence in basis points.
    pub confidence_bp: Option<i32>,
    /// Amazon selling price in centavos.
    pub amazon_price_mxn: Option<i64>,
    /// Amazon fees in centavos.
    pub fees_mxn: Option<i64>,
    /// Profit per unit in centavos.
    pub profit_centavos: Option<i64>,
    /// ROI in basis points.
    pub roi_bp: Option<i64>,
    /// Margin in basis points.
    pub margin_bp: Option<i64>,
}

/// A search with its outcomes (sorted by retailer) and results (sorted by
/// retailer then SKU).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchViewRows {
    /// The search.
    pub search: SearchRow,
    /// Per-retailer outcomes.
    pub retailers: Vec<RetailerOutcomeRow>,
    /// Results.
    pub results: Vec<SearchResultRow>,
}

/// Columns selected for a [`SearchRow`].
macro_rules! search_columns {
    () => {
        "id, tenant_id, query, status, reason, \
         (EXTRACT(EPOCH FROM created_at) * 1000)::BIGINT AS created_at_ms, \
         (EXTRACT(EPOCH FROM finished_at) * 1000)::BIGINT AS finished_at_ms"
    };
}

fn search_row(row: &PgRow) -> Result<SearchRow, sqlx::Error> {
    Ok(SearchRow {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        query: row.try_get("query")?,
        status: row.try_get("status")?,
        reason: row.try_get("reason")?,
        created_at_ms: row.try_get("created_at_ms")?,
        finished_at_ms: row.try_get("finished_at_ms")?,
    })
}

/// Stores a `queued` search and enqueues `{"kind":"search","search_id":..,
/// "tenant_id":..}` on `search_jobs`, in one transaction. Returns the id.
///
/// # Errors
///
/// Database errors (including CHECK violations); redact before logging.
pub async fn create_search(
    pool: &PgPool,
    tenant_id: &str,
    query: &str,
    now_ms: i64,
) -> Result<i64, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let id: i64 = sqlx::query(
        "INSERT INTO searches (tenant_id, query, created_at, updated_at) \
         VALUES ($1, $2, to_timestamp($3 / 1000.0), to_timestamp($3 / 1000.0)) RETURNING id",
    )
    .bind(tenant_id)
    .bind(query)
    .bind(now_ms)
    .fetch_one(&mut *tx)
    .await?
    .try_get("id")?;
    sqlx::query(
        "SELECT pgmq.send($1::text, jsonb_build_object(\
           'kind', 'search', 'search_id', $2::bigint, 'tenant_id', $3::text))",
    )
    .bind(Queue::SearchJobs.as_str())
    .bind(id)
    .bind(tenant_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(id)
}

/// The search `id`, whoever owns it (worker use; the API reads through
/// [`load_view`]).
///
/// # Errors
///
/// Database errors; redact before logging.
pub async fn load_search(pool: &PgPool, id: i64) -> Result<Option<SearchRow>, sqlx::Error> {
    let row = sqlx::query(concat!(
        "SELECT ",
        search_columns!(),
        " FROM searches WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await?;
    row.as_ref().map(search_row).transpose()
}

/// Moves a `queued` (or retried `running`) search to `running`. `false`
/// when it is unknown or already terminal.
///
/// # Errors
///
/// Database errors; redact before logging.
pub async fn mark_running(pool: &PgPool, id: i64, now_ms: i64) -> Result<bool, sqlx::Error> {
    let done = sqlx::query(
        "UPDATE searches SET status = 'running', updated_at = to_timestamp($2 / 1000.0) \
         WHERE id = $1 AND status IN ('queued', 'running')",
    )
    .bind(id)
    .bind(now_ms)
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Sets a terminal `status` (`done`, `partial`, `failed`, `refused`) and a
/// fixed `reason` code. `false` when the search is unknown or already
/// terminal (terminal states never change).
///
/// # Errors
///
/// Database errors (including CHECK violations); redact before logging.
pub async fn finish(
    pool: &PgPool,
    id: i64,
    status: &str,
    reason: &str,
    now_ms: i64,
) -> Result<bool, sqlx::Error> {
    let done = sqlx::query(
        "UPDATE searches SET status = $2, reason = $3, \
           updated_at = to_timestamp($4 / 1000.0), finished_at = to_timestamp($4 / 1000.0) \
         WHERE id = $1 AND status IN ('queued', 'running') \
           AND $2 IN ('done', 'partial', 'failed', 'refused')",
    )
    .bind(id)
    .bind(status)
    .bind(reason)
    .bind(now_ms)
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Inserts or replaces one retailer's outcome (retries overwrite).
///
/// # Errors
///
/// Database errors (including CHECK violations); redact before logging.
pub async fn upsert_retailer_outcome(
    pool: &PgPool,
    search_id: i64,
    outcome: &RetailerOutcomeRow,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO search_retailers (search_id, retailer, outcome, offers) \
         VALUES ($1, $2, $3, $4) \
         ON CONFLICT (search_id, retailer) DO UPDATE SET \
           outcome = EXCLUDED.outcome, offers = EXCLUDED.offers",
    )
    .bind(search_id)
    .bind(&outcome.retailer)
    .bind(&outcome.outcome)
    .bind(outcome.offers)
    .execute(pool)
    .await?;
    Ok(())
}

/// Inserts or replaces one result (retries overwrite).
///
/// # Errors
///
/// Database errors (including CHECK violations); redact before logging.
pub async fn upsert_result(
    pool: &PgPool,
    search_id: i64,
    r: &SearchResultRow,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO search_results (search_id, retailer, retailer_sku, title, url, price_mxn, \
           asin, match_status, confidence_bp, amazon_price_mxn, fees_mxn, profit_centavos, \
           roi_bp, margin_bp, availability, membership_required) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16) \
         ON CONFLICT (search_id, retailer, retailer_sku) DO UPDATE SET \
           title = EXCLUDED.title, url = EXCLUDED.url, price_mxn = EXCLUDED.price_mxn, \
           asin = EXCLUDED.asin, match_status = EXCLUDED.match_status, \
           confidence_bp = EXCLUDED.confidence_bp, amazon_price_mxn = EXCLUDED.amazon_price_mxn, \
           fees_mxn = EXCLUDED.fees_mxn, profit_centavos = EXCLUDED.profit_centavos, \
           roi_bp = EXCLUDED.roi_bp, margin_bp = EXCLUDED.margin_bp, \
           availability = EXCLUDED.availability, \
           membership_required = EXCLUDED.membership_required",
    )
    .bind(search_id)
    .bind(&r.retailer)
    .bind(&r.retailer_sku)
    .bind(&r.title)
    .bind(&r.url)
    .bind(r.price_mxn)
    .bind(&r.asin)
    .bind(&r.match_status)
    .bind(r.confidence_bp)
    .bind(r.amazon_price_mxn)
    .bind(r.fees_mxn)
    .bind(r.profit_centavos)
    .bind(r.roi_bp)
    .bind(r.margin_bp)
    .bind(&r.availability)
    .bind(r.membership_required)
    .execute(pool)
    .await?;
    Ok(())
}

/// The search `id` with its outcomes and results, only if `tenant_id`
/// owns it.
///
/// # Errors
///
/// Database errors; redact before logging.
pub async fn load_view(
    pool: &PgPool,
    tenant_id: &str,
    id: i64,
) -> Result<Option<SearchViewRows>, sqlx::Error> {
    let Some(row) = sqlx::query(concat!(
        "SELECT ",
        search_columns!(),
        " FROM searches WHERE id = $1 AND tenant_id = $2"
    ))
    .bind(id)
    .bind(tenant_id)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(None);
    };
    let search = search_row(&row)?;
    let retailers = sqlx::query(
        "SELECT retailer, outcome, offers FROM search_retailers WHERE search_id = $1 \
         ORDER BY retailer COLLATE \"C\"",
    )
    .bind(id)
    .fetch_all(pool)
    .await?
    .iter()
    .map(|r| {
        Ok(RetailerOutcomeRow {
            retailer: r.try_get("retailer")?,
            outcome: r.try_get("outcome")?,
            offers: r.try_get("offers")?,
        })
    })
    .collect::<Result<Vec<_>, sqlx::Error>>()?;
    let results = sqlx::query(
        "SELECT retailer, retailer_sku, title, url, price_mxn, asin, match_status, \
           confidence_bp, amazon_price_mxn, fees_mxn, profit_centavos, roi_bp, margin_bp, \
           availability, membership_required \
         FROM search_results WHERE search_id = $1 \
         ORDER BY retailer COLLATE \"C\", retailer_sku COLLATE \"C\"",
    )
    .bind(id)
    .fetch_all(pool)
    .await?
    .iter()
    .map(|r| {
        Ok(SearchResultRow {
            retailer: r.try_get("retailer")?,
            retailer_sku: r.try_get("retailer_sku")?,
            title: r.try_get("title")?,
            url: r.try_get("url")?,
            price_mxn: r.try_get("price_mxn")?,
            availability: r.try_get("availability")?,
            membership_required: r.try_get("membership_required")?,
            asin: r.try_get("asin")?,
            match_status: r.try_get("match_status")?,
            confidence_bp: r.try_get("confidence_bp")?,
            amazon_price_mxn: r.try_get("amazon_price_mxn")?,
            fees_mxn: r.try_get("fees_mxn")?,
            profit_centavos: r.try_get("profit_centavos")?,
            roi_bp: r.try_get("roi_bp")?,
            margin_bp: r.try_get("margin_bp")?,
        })
    })
    .collect::<Result<Vec<_>, sqlx::Error>>()?;
    Ok(Some(SearchViewRows {
        search,
        retailers,
        results,
    }))
}

fn price(centavos: Option<i64>) -> Option<Option<PriceMxn>> {
    match centavos {
        None => Some(None),
        Some(c) => PriceMxn::from_centavos(c).ok().map(Some),
    }
}

fn result_view(r: SearchResultRow) -> Option<SearchResultView> {
    let amazon = match (r.asin, r.match_status, r.confidence_bp) {
        (Some(asin), Some(status), Some(bp)) => Some(ResultMatchView {
            asin: Asin::new(&asin).ok()?,
            status: MatchStatus::parse(&status)?,
            confidence: Confidence::from_bp(u32::try_from(bp).ok()?).ok()?,
            amazon_price_mxn: price(r.amazon_price_mxn)?,
            fees_mxn: price(r.fees_mxn)?,
            profit_centavos: r.profit_centavos,
            roi_bp: r.roi_bp,
            margin_bp: r.margin_bp,
        }),
        (None, None, None) => None,
        _ => return None,
    };
    Some(SearchResultView {
        retailer: RetailerId::new(&r.retailer).ok()?,
        retailer_sku: RetailerSku::new(&r.retailer_sku).ok()?,
        title: r.title,
        url: r.url,
        price_mxn: PriceMxn::from_centavos(r.price_mxn).ok()?,
        availability: Availability::parse(&r.availability)?,
        membership_required: r.membership_required,
        amazon,
    })
}

/// Maps stored rows to the app view; `None` if any row breaks the schema's
/// invariants (callers fail closed).
#[must_use]
pub fn view_from_rows(rows: SearchViewRows) -> Option<SearchView> {
    let s = rows.search;
    let retailers = rows
        .retailers
        .into_iter()
        .map(|r| {
            Some(RetailerOutcomeView {
                retailer: RetailerId::new(&r.retailer).ok()?,
                outcome: RetailerSearchOutcome::parse(&r.outcome)?,
                offers: u32::try_from(r.offers).ok()?,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    let results = rows
        .results
        .into_iter()
        .map(result_view)
        .collect::<Option<Vec<_>>>()?;
    Some(SearchView {
        id: SearchId(s.id),
        query: s.query,
        status: SearchStatus::parse(&s.status)?,
        reason: s.reason,
        created_at: UnixMillis(s.created_at_ms),
        finished_at: s.finished_at_ms.map(UnixMillis),
        retailers,
        results,
        hidden_results: 0,
    })
}

/// [`SearchRepo`] over the search tables and the `search_jobs` queue.
/// Errors become [`RepoError::Unavailable`] without detail.
#[derive(Clone)]
pub struct PgSearches {
    pool: PgPool,
    clock: Arc<dyn Clock>,
}

impl PgSearches {
    /// Searches in `pool`, stamped with `clock`.
    #[must_use]
    pub fn new(pool: PgPool, clock: Arc<dyn Clock>) -> Self {
        Self { pool, clock }
    }
}

impl SearchRepo for PgSearches {
    fn create_and_enqueue<'a>(
        &'a self,
        tenant: &'a TenantId,
        query: &'a SearchQuery,
    ) -> PortFuture<'a, Result<SearchId, RepoError>> {
        Box::pin(async move {
            create_search(
                &self.pool,
                tenant.as_str(),
                query.as_str(),
                self.clock.now().0,
            )
            .await
            .map(SearchId)
            .map_err(|_| RepoError::Unavailable)
        })
    }

    fn load<'a>(
        &'a self,
        tenant: &'a TenantId,
        id: SearchId,
    ) -> PortFuture<'a, Result<Option<SearchView>, RepoError>> {
        Box::pin(async move {
            let rows = load_view(&self.pool, tenant.as_str(), id.0)
                .await
                .map_err(|_| RepoError::Unavailable)?;
            rows.map(|r| view_from_rows(r).ok_or(RepoError::Unavailable))
                .transpose()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn search() -> SearchRow {
        SearchRow {
            id: 7,
            tenant_id: "default".to_owned(),
            query: "taladro".to_owned(),
            status: "partial".to_owned(),
            reason: "amazon_refused".to_owned(),
            created_at_ms: 10,
            finished_at_ms: Some(20),
        }
    }

    fn result(asin: Option<&str>) -> SearchResultRow {
        SearchResultRow {
            retailer: "liverpool".to_owned(),
            retailer_sku: "1001".to_owned(),
            title: "Taladro".to_owned(),
            url: "https://www.liverpool.com.mx/tienda/pdp/1001".to_owned(),
            price_mxn: 100_000,
            availability: "in_stock".to_owned(),
            membership_required: false,
            asin: asin.map(str::to_owned),
            match_status: asin.map(|_| "accepted".to_owned()),
            confidence_bp: asin.map(|_| 9_500),
            amazon_price_mxn: asin.map(|_| 150_000),
            fees_mxn: asin.map(|_| 20_000),
            profit_centavos: asin.map(|_| 30_000),
            roi_bp: asin.map(|_| 3_000),
            margin_bp: asin.map(|_| 2_000),
        }
    }

    #[test]
    fn rows_map_to_the_app_view() {
        let view = view_from_rows(SearchViewRows {
            search: search(),
            retailers: vec![RetailerOutcomeRow {
                retailer: "sears_mx".to_owned(),
                outcome: "category_only".to_owned(),
                offers: 0,
            }],
            results: vec![result(Some("B0CHHSFMRL")), result(None)],
        })
        .unwrap();
        assert_eq!(view.id, SearchId(7));
        assert_eq!(view.status, SearchStatus::Partial);
        assert_eq!(view.finished_at, Some(UnixMillis(20)));
        assert_eq!(
            view.retailers[0].outcome,
            RetailerSearchOutcome::CategoryOnly
        );
        let m = view.results[0].amazon.as_ref().unwrap();
        assert_eq!(m.asin.as_str(), "B0CHHSFMRL");
        assert_eq!(m.roi_bp, Some(3_000));
        assert!(view.results[1].amazon.is_none());
        assert_eq!(view.results[0].availability, Availability::InStock);
        assert!(!view.results[0].membership_required);
        assert_eq!(view.hidden_results, 0);
    }

    #[test]
    fn rows_breaking_invariants_fail_closed() {
        let mut bad_status = search();
        bad_status.status = "weird".to_owned();
        let rows = |search, results| SearchViewRows {
            search,
            retailers: Vec::new(),
            results,
        };
        assert!(view_from_rows(rows(bad_status, Vec::new())).is_none());
        let mut half_match = result(Some("B0CHHSFMRL"));
        half_match.match_status = None;
        assert!(view_from_rows(rows(search(), vec![half_match])).is_none());
        let mut negative = result(None);
        negative.price_mxn = -1;
        assert!(view_from_rows(rows(search(), vec![negative])).is_none());
        let mut bad_stock = result(None);
        bad_stock.availability = "maybe".to_owned();
        assert!(view_from_rows(rows(search(), vec![bad_stock])).is_none());
    }
}
