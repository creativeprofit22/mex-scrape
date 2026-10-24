//! Queries for `amazon_products` (migration `0010_amazon_products.sql`,
//! ADR 0017) and the [`app::AmazonProductRepo`] adapter over them.
//!
//! One row per ASIN with the Amazon name and brand seen during matching.
//! The newest `seen_at` wins; a newer lookup without a brand keeps the
//! stored one. Timestamps cross the boundary as epoch milliseconds.

use app::{AmazonDataSource, AmazonProduct, AmazonProductRepo, PortFuture, RepoError};
use domain::{Asin, UnixMillis};
use sqlx::Row as _;
use sqlx::postgres::PgPool;

/// An Amazon product row, to write or as read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductRow {
    /// Amazon product id.
    pub asin: String,
    /// Product name on Amazon (1–500 characters).
    pub title: String,
    /// Brand, when known (1–200 characters).
    pub brand: Option<String>,
    /// `sp_api` or `bright_data`.
    pub source: String,
    /// When the catalog data was read (epoch ms).
    pub seen_at_ms: i64,
}

/// Inserts the product, or refreshes the stored one when `row` is at least
/// as new. A `None` brand keeps the stored brand. Returns the number of
/// rows written: 1, or 0 when the stored row is newer.
///
/// # Errors
///
/// Database errors (including CHECK violations); redact before logging.
pub async fn upsert_product(pool: &PgPool, row: &ProductRow) -> Result<u64, sqlx::Error> {
    let done = sqlx::query(
        "INSERT INTO amazon_products (asin, title, brand, source, seen_at) \
         VALUES ($1, $2, $3, $4, to_timestamp($5 / 1000.0)) \
         ON CONFLICT (asin) DO UPDATE SET \
           title = EXCLUDED.title, \
           brand = COALESCE(EXCLUDED.brand, amazon_products.brand), \
           source = EXCLUDED.source, seen_at = EXCLUDED.seen_at \
         WHERE EXCLUDED.seen_at >= amazon_products.seen_at",
    )
    .bind(&row.asin)
    .bind(&row.title)
    .bind(row.brand.as_deref())
    .bind(&row.source)
    .bind(row.seen_at_ms)
    .execute(pool)
    .await?;
    Ok(done.rows_affected())
}

/// The stored product for `asin`, if any.
///
/// # Errors
///
/// Database errors; redact before logging.
pub async fn find_product(pool: &PgPool, asin: &str) -> Result<Option<ProductRow>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT asin, title, brand, source, \
         (EXTRACT(EPOCH FROM seen_at) * 1000)::BIGINT AS seen_at_ms \
         FROM amazon_products WHERE asin = $1",
    )
    .bind(asin)
    .fetch_optional(pool)
    .await?;
    row.map(|r| {
        Ok(ProductRow {
            asin: r.try_get("asin")?,
            title: r.try_get("title")?,
            brand: r.try_get("brand")?,
            source: r.try_get("source")?,
            seen_at_ms: r.try_get("seen_at_ms")?,
        })
    })
    .transpose()
}

fn to_row(p: &AmazonProduct) -> ProductRow {
    ProductRow {
        asin: p.asin.as_str().to_owned(),
        title: p.title.clone(),
        brand: p.brand.clone(),
        source: p.source.as_str().to_owned(),
        seen_at_ms: p.seen_at.0,
    }
}

/// The app value for a stored row; `None` when the row breaks the app
/// rules (schema drift).
fn from_row(row: ProductRow) -> Option<AmazonProduct> {
    let product = AmazonProduct::new(
        Asin::new(&row.asin).ok()?,
        &row.title,
        row.brand.as_deref(),
        AmazonDataSource::from_name(&row.source)?,
        UnixMillis(row.seen_at_ms),
    )?;
    // A stored value that normalization would change was not written by
    // this adapter; refuse it rather than show something else.
    (product.title == row.title && product.brand == row.brand).then_some(product)
}

/// [`AmazonProductRepo`] over `amazon_products`. Errors become
/// [`RepoError::Unavailable`] without detail.
#[derive(Clone)]
pub struct PgAmazonProducts(pub PgPool);

impl AmazonProductRepo for PgAmazonProducts {
    fn upsert<'a>(&'a self, product: &'a AmazonProduct) -> PortFuture<'a, Result<(), RepoError>> {
        Box::pin(async move {
            upsert_product(&self.0, &to_row(product))
                .await
                .map(|_| ())
                .map_err(|_| RepoError::Unavailable)
        })
    }

    fn find<'a>(
        &'a self,
        asin: &'a Asin,
    ) -> PortFuture<'a, Result<Option<AmazonProduct>, RepoError>> {
        Box::pin(async move {
            let row = find_product(&self.0, asin.as_str())
                .await
                .map_err(|_| RepoError::Unavailable)?;
            // Fail closed on a row the CHECKs should have refused.
            row.map(|r| from_row(r).ok_or(RepoError::Unavailable))
                .transpose()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(title: &str, brand: Option<&str>, source: &str) -> ProductRow {
        ProductRow {
            asin: "B0DEMO0001".to_owned(),
            title: title.to_owned(),
            brand: brand.map(str::to_owned),
            source: source.to_owned(),
            seen_at_ms: 7,
        }
    }

    #[test]
    fn valid_rows_round_trip() {
        let stored = row("Licuadora 600 W", Some("Oster"), "sp_api");
        let product = from_row(stored.clone()).unwrap();
        assert_eq!(product.source, AmazonDataSource::SpApi);
        assert_eq!(product.seen_at, UnixMillis(7));
        assert_eq!(to_row(&product), stored);
    }

    #[test]
    fn invalid_rows_are_refused() {
        assert!(from_row(row("", None, "sp_api")).is_none());
        assert!(from_row(row("  Licuadora", None, "sp_api")).is_none());
        assert!(from_row(row("Licuadora", Some(" "), "sp_api")).is_none());
        assert!(from_row(row("Licuadora", None, "keepa")).is_none());
        let mut bad_asin = row("Licuadora", None, "bright_data");
        bad_asin.asin = "nope".to_owned();
        assert!(from_row(bad_asin).is_none());
    }
}
