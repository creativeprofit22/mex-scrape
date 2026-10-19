//! Queries for `price_observations` and `offer_matches` (migration
//! `0004_matching_price_history.sql`), plus the [`app::PriceHistoryRepo`]
//! and [`app::MatchRepo`] adapters over them.
//!
//! `price_observations` is append-only: a trigger refuses UPDATE, DELETE
//! and TRUNCATE. Timestamps cross the boundary as epoch milliseconds and
//! money as integer centavos.

use app::{MatchRepo, PortFuture, PriceHistoryRepo, PricePoint, RepoError, UpsertOutcome};
use domain::{
    Asin, Availability, Offer, PriceMxn, ProductMatch, RetailerId, RetailerSku, UnixMillis,
};
use sqlx::Row as _;
use sqlx::postgres::PgPool;

/// A price observation to insert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewObservation<'a> {
    /// Retailer id.
    pub retailer: &'a str,
    /// Retailer SKU.
    pub retailer_sku: &'a str,
    /// Selling price in centavos.
    pub price_mxn: i64,
    /// List price in centavos, when above the selling price.
    pub list_price_mxn: Option<i64>,
    /// `in_stock`, `out_of_stock`, `pre_order` or `unknown`.
    pub availability: &'a str,
    /// Observation time (epoch ms).
    pub observed_at_ms: i64,
}

/// A stored price observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservationRow {
    /// Selling price in centavos.
    pub price_mxn: i64,
    /// List price in centavos.
    pub list_price_mxn: Option<i64>,
    /// Stock state name.
    pub availability: String,
    /// Observation time (epoch ms).
    pub observed_at_ms: i64,
}

/// A match to insert or replace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatchRow<'a> {
    /// Retailer id.
    pub retailer: &'a str,
    /// Retailer SKU.
    pub retailer_sku: &'a str,
    /// Matched ASIN.
    pub asin: &'a str,
    /// `gtin` or `title`.
    pub method: &'a str,
    /// Confidence in basis points.
    pub confidence_bp: i32,
    /// `accepted`, `needs_review` or `rejected`.
    pub status: &'a str,
    /// Match time (epoch ms).
    pub matched_at_ms: i64,
}

/// Appends one observation.
///
/// # Errors
///
/// Database errors (including CHECK violations); redact before logging.
pub async fn insert_observation(pool: &PgPool, o: NewObservation<'_>) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO price_observations \
         (retailer, retailer_sku, price_mxn, list_price_mxn, availability, observed_at) \
         VALUES ($1, $2, $3, $4, $5, to_timestamp($6 / 1000.0))",
    )
    .bind(o.retailer)
    .bind(o.retailer_sku)
    .bind(o.price_mxn)
    .bind(o.list_price_mxn)
    .bind(o.availability)
    .bind(o.observed_at_ms)
    .execute(pool)
    .await?;
    Ok(())
}

/// Observations of one listing at or after `since_ms`, oldest first (ties
/// by insertion order).
///
/// # Errors
///
/// Database errors; redact before logging.
pub async fn observations_since(
    pool: &PgPool,
    retailer: &str,
    retailer_sku: &str,
    since_ms: i64,
) -> Result<Vec<ObservationRow>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT price_mxn, list_price_mxn, availability, \
         (EXTRACT(EPOCH FROM observed_at) * 1000)::BIGINT AS observed_at_ms \
         FROM price_observations \
         WHERE retailer = $1 AND retailer_sku = $2 AND observed_at >= to_timestamp($3 / 1000.0) \
         ORDER BY observed_at, id",
    )
    .bind(retailer)
    .bind(retailer_sku)
    .bind(since_ms)
    .fetch_all(pool)
    .await?;
    rows.iter()
        .map(|row| {
            Ok(ObservationRow {
                price_mxn: row.try_get("price_mxn")?,
                list_price_mxn: row.try_get("list_price_mxn")?,
                availability: row.try_get("availability")?,
                observed_at_ms: row.try_get("observed_at_ms")?,
            })
        })
        .collect()
}

/// Inserts or replaces the listing's match, except that a stored
/// `rejected` match is never overwritten. Returns the number of rows
/// written: 1, or 0 when a stored `rejected` match was kept.
///
/// # Errors
///
/// Database errors (including CHECK violations); redact before logging.
pub async fn upsert_match(pool: &PgPool, m: MatchRow<'_>) -> Result<u64, sqlx::Error> {
    let done = sqlx::query(
        "INSERT INTO offer_matches \
         (retailer, retailer_sku, asin, method, confidence_bp, status, matched_at) \
         VALUES ($1, $2, $3, $4, $5, $6, to_timestamp($7 / 1000.0)) \
         ON CONFLICT (retailer, retailer_sku) DO UPDATE SET \
           asin = EXCLUDED.asin, method = EXCLUDED.method, \
           confidence_bp = EXCLUDED.confidence_bp, status = EXCLUDED.status, \
           matched_at = EXCLUDED.matched_at \
         WHERE offer_matches.status <> 'rejected'",
    )
    .bind(m.retailer)
    .bind(m.retailer_sku)
    .bind(m.asin)
    .bind(m.method)
    .bind(m.confidence_bp)
    .bind(m.status)
    .bind(m.matched_at_ms)
    .execute(pool)
    .await?;
    Ok(done.rows_affected())
}

/// The stored status of a listing's match, if any.
///
/// # Errors
///
/// Database errors; redact before logging.
pub async fn match_status(
    pool: &PgPool,
    retailer: &str,
    retailer_sku: &str,
) -> Result<Option<(String, String)>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT asin, status FROM offer_matches WHERE retailer = $1 AND retailer_sku = $2",
    )
    .bind(retailer)
    .bind(retailer_sku)
    .fetch_optional(pool)
    .await?;
    row.map(|r| Ok((r.try_get("asin")?, r.try_get("status")?)))
        .transpose()
}

/// `(retailer, retailer_sku)` of every accepted match to `asin`, sorted by
/// retailer then SKU.
///
/// # Errors
///
/// Database errors; redact before logging.
pub async fn accepted_listings(
    pool: &PgPool,
    asin: &str,
) -> Result<Vec<(String, String)>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT retailer, retailer_sku FROM offer_matches \
         WHERE asin = $1 AND status = 'accepted' \
         ORDER BY retailer COLLATE \"C\", retailer_sku COLLATE \"C\"",
    )
    .bind(asin)
    .fetch_all(pool)
    .await?;
    rows.iter()
        .map(|r| Ok((r.try_get("retailer")?, r.try_get("retailer_sku")?)))
        .collect()
}

fn availability_from(name: &str) -> Option<Availability> {
    [
        Availability::InStock,
        Availability::OutOfStock,
        Availability::PreOrder,
        Availability::Unknown,
    ]
    .into_iter()
    .find(|a| a.as_str() == name)
}

fn point(row: ObservationRow) -> Option<PricePoint> {
    Some(PricePoint {
        price_mxn: PriceMxn::from_centavos(row.price_mxn).ok()?,
        list_price_mxn: match row.list_price_mxn {
            Some(c) => Some(PriceMxn::from_centavos(c).ok()?),
            None => None,
        },
        availability: availability_from(&row.availability)?,
        observed_at: UnixMillis(row.observed_at_ms),
    })
}

/// [`PriceHistoryRepo`] over `price_observations`. Errors become
/// [`RepoError::Unavailable`] without detail.
#[derive(Clone)]
pub struct PgPriceHistory(pub PgPool);

impl PriceHistoryRepo for PgPriceHistory {
    fn record<'a>(&'a self, offer: &'a Offer) -> PortFuture<'a, Result<(), RepoError>> {
        Box::pin(async move {
            insert_observation(
                &self.0,
                NewObservation {
                    retailer: offer.retailer.as_str(),
                    retailer_sku: offer.retailer_sku.as_str(),
                    price_mxn: offer.price_mxn.centavos(),
                    list_price_mxn: offer.list_price_mxn.map(PriceMxn::centavos),
                    availability: offer.availability.as_str(),
                    observed_at_ms: offer.fetched_at.0,
                },
            )
            .await
            .map_err(|_| RepoError::Unavailable)
        })
    }

    fn history<'a>(
        &'a self,
        retailer: &'a RetailerId,
        sku: &'a RetailerSku,
        since: UnixMillis,
    ) -> PortFuture<'a, Result<Vec<PricePoint>, RepoError>> {
        Box::pin(async move {
            let rows = observations_since(&self.0, retailer.as_str(), sku.as_str(), since.0)
                .await
                .map_err(|_| RepoError::Unavailable)?;
            // The table CHECKs make every row valid; a row that is not
            // means the schema drifted, so fail closed.
            rows.into_iter()
                .map(|r| point(r).ok_or(RepoError::Unavailable))
                .collect()
        })
    }
}

/// [`MatchRepo`] over `offer_matches`. Errors become
/// [`RepoError::Unavailable`] without detail.
#[derive(Clone)]
pub struct PgMatches(pub PgPool);

impl MatchRepo for PgMatches {
    fn upsert<'a>(
        &'a self,
        m: &'a ProductMatch,
    ) -> PortFuture<'a, Result<UpsertOutcome, RepoError>> {
        Box::pin(async move {
            let written = upsert_match(
                &self.0,
                MatchRow {
                    retailer: m.retailer.as_str(),
                    retailer_sku: m.retailer_sku.as_str(),
                    asin: m.asin.as_str(),
                    method: m.method.as_str(),
                    confidence_bp: i32::from(m.confidence.bp()),
                    status: m.status.as_str(),
                    matched_at_ms: m.matched_at.0,
                },
            )
            .await
            .map_err(|_| RepoError::Unavailable)?;
            // The upsert touches at most the one row keyed by the listing.
            match written {
                0 => Ok(UpsertOutcome::KeptRejected),
                1 => Ok(UpsertOutcome::Written),
                _ => Err(RepoError::Unavailable),
            }
        })
    }

    fn accepted_listings<'a>(
        &'a self,
        asin: &'a Asin,
    ) -> PortFuture<'a, Result<Vec<(RetailerId, RetailerSku)>, RepoError>> {
        Box::pin(async move {
            let rows = accepted_listings(&self.0, asin.as_str())
                .await
                .map_err(|_| RepoError::Unavailable)?;
            rows.into_iter()
                .map(|(r, s)| {
                    Ok((
                        RetailerId::new(&r).map_err(|_| RepoError::Unavailable)?,
                        RetailerSku::new(&s).map_err(|_| RepoError::Unavailable)?,
                    ))
                })
                .collect()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn availability_names_round_trip() {
        for a in [
            Availability::InStock,
            Availability::OutOfStock,
            Availability::PreOrder,
            Availability::Unknown,
        ] {
            assert_eq!(availability_from(a.as_str()), Some(a));
        }
        assert_eq!(availability_from("sold_out"), None);
    }

    #[test]
    fn invalid_rows_are_refused() {
        let row = |price, avail: &str| ObservationRow {
            price_mxn: price,
            list_price_mxn: None,
            availability: avail.to_owned(),
            observed_at_ms: 5,
        };
        assert!(point(row(-1, "in_stock")).is_none());
        assert!(point(row(1, "bogus")).is_none());
        let ok = point(row(100, "pre_order")).unwrap();
        assert_eq!(ok.price_mxn.centavos(), 100);
        assert_eq!(ok.availability, Availability::PreOrder);
        assert_eq!(ok.observed_at, UnixMillis(5));
    }
}
