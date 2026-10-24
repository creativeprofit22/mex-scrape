//! Amazon product names (ADR 0017): normalizing catalog text into an
//! [`AmazonProduct`], and the lookup the price use cases share to tell a
//! known product (named or not) from an unknown ASIN. No I/O.

use domain::{Asin, UnixMillis};

use crate::ports::{
    AmazonDataSource, AmazonProduct, AmazonProductRepo, CatalogItem, MatchRepo, RepoError,
};

/// Longest stored product title, in characters.
pub const MAX_TITLE_CHARS: usize = 500;
/// Longest stored brand, in characters.
pub const MAX_BRAND_CHARS: usize = 200;

/// The Amazon name of a known product; both fields are `None` when the
/// product is matched but no catalog name has been stored yet.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProductName {
    /// Product name on Amazon.
    pub title: Option<String>,
    /// Brand, when known.
    pub brand: Option<String>,
}

/// Trims, collapses inner whitespace (control characters included) to
/// single spaces and cuts to `max` characters; `None` when nothing is left.
fn clean(text: &str, max: usize) -> Option<String> {
    let joined = text
        .split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let cut: String = joined.chars().take(max).collect();
    let cut = cut.trim_end().to_owned();
    (!cut.is_empty()).then_some(cut)
}

impl AmazonProduct {
    /// A product with normalized text: `None` when the title is blank. A
    /// blank brand becomes `None`. Titles are cut to [`MAX_TITLE_CHARS`]
    /// and brands to [`MAX_BRAND_CHARS`] characters.
    #[must_use]
    pub fn new(
        asin: Asin,
        title: &str,
        brand: Option<&str>,
        source: AmazonDataSource,
        seen_at: UnixMillis,
    ) -> Option<Self> {
        Some(Self {
            asin,
            title: clean(title, MAX_TITLE_CHARS)?,
            brand: brand.and_then(|b| clean(b, MAX_BRAND_CHARS)),
            source,
            seen_at,
        })
    }

    /// The product a catalog lookup described, as [`AmazonProduct::new`].
    #[must_use]
    pub fn from_catalog(
        item: &CatalogItem,
        source: AmazonDataSource,
        seen_at: UnixMillis,
    ) -> Option<Self> {
        Self::new(
            item.asin.clone(),
            &item.title,
            item.brand.as_deref(),
            source,
            seen_at,
        )
    }
}

/// The name of `asin`, or `None` when the product is unknown: no stored
/// name, no match in any status and no listing the reader sees
/// (`has_listings`, which covers the reader's own accepted matches).
pub(crate) async fn lookup(
    matches: &dyn MatchRepo,
    products: &dyn AmazonProductRepo,
    asin: &Asin,
    has_listings: bool,
) -> Result<Option<ProductName>, RepoError> {
    if let Some(p) = products.find(asin).await? {
        return Ok(Some(ProductName {
            title: Some(p.title),
            brand: p.brand,
        }));
    }
    if has_listings || matches.has_any_match(asin).await? {
        return Ok(Some(ProductName::default()));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assistant::fakes::FakeProducts;
    use crate::ports::{PortFuture, UpsertOutcome};
    use domain::{ProductMatch, RetailerId, RetailerSku};

    fn asin() -> Asin {
        Asin::new("B0DEMO0001").unwrap()
    }

    fn item(title: &str, brand: Option<&str>) -> CatalogItem {
        CatalogItem {
            asin: asin(),
            title: title.to_owned(),
            brand: brand.map(str::to_owned),
            images: vec![],
            sales_ranks: vec![],
        }
    }

    #[test]
    fn catalog_text_is_normalized() {
        let p = AmazonProduct::from_catalog(
            &item("  Licuadora\t Oster \n600 W ", Some("  Oster ")),
            AmazonDataSource::SpApi,
            UnixMillis(9),
        )
        .unwrap();
        assert_eq!(p.title, "Licuadora Oster 600 W");
        assert_eq!(p.brand.as_deref(), Some("Oster"));
        assert_eq!(p.source, AmazonDataSource::SpApi);
        assert_eq!(p.seen_at, UnixMillis(9));
    }

    #[test]
    fn blank_title_is_refused_and_blank_brand_dropped() {
        let none = |t: &str| {
            AmazonProduct::from_catalog(&item(t, None), AmazonDataSource::BrightData, UnixMillis(0))
        };
        assert!(none("").is_none());
        assert!(none(" \t\n").is_none());
        let p = AmazonProduct::from_catalog(
            &item("Licuadora", Some("  ")),
            AmazonDataSource::BrightData,
            UnixMillis(0),
        )
        .unwrap();
        assert_eq!(p.brand, None);
    }

    #[test]
    fn long_text_is_cut_on_char_boundaries() {
        let title = "ñ".repeat(MAX_TITLE_CHARS + 20);
        let brand = "é".repeat(MAX_BRAND_CHARS + 1);
        let p = AmazonProduct::from_catalog(
            &item(&title, Some(&brand)),
            AmazonDataSource::SpApi,
            UnixMillis(0),
        )
        .unwrap();
        assert_eq!(p.title.chars().count(), MAX_TITLE_CHARS);
        assert_eq!(p.brand.unwrap().chars().count(), MAX_BRAND_CHARS);
        // A cut that ends on a space leaves no trailing space.
        let spaced = format!("{} b", "a".repeat(MAX_TITLE_CHARS - 1));
        let p = AmazonProduct::new(
            asin(),
            &spaced,
            None,
            AmazonDataSource::SpApi,
            UnixMillis(0),
        )
        .unwrap();
        assert_eq!(p.title, "a".repeat(MAX_TITLE_CHARS - 1));
    }

    struct Matches(Result<bool, RepoError>);

    impl MatchRepo for Matches {
        fn upsert<'a>(
            &'a self,
            _: &'a ProductMatch,
        ) -> PortFuture<'a, Result<UpsertOutcome, RepoError>> {
            Box::pin(async { Ok(UpsertOutcome::Written) })
        }

        fn accepted_listings<'a>(
            &'a self,
            _: &'a Asin,
        ) -> PortFuture<'a, Result<Vec<(RetailerId, RetailerSku)>, RepoError>> {
            Box::pin(async { Ok(vec![]) })
        }

        fn has_any_match<'a>(&'a self, _: &'a Asin) -> PortFuture<'a, Result<bool, RepoError>> {
            Box::pin(async move { self.0 })
        }
    }

    #[tokio::test]
    async fn lookup_tells_named_unnamed_and_unknown_apart() {
        let named = FakeProducts::named("Licuadora", Some("Oster"));
        let got = lookup(&Matches(Ok(false)), &named, &asin(), false)
            .await
            .unwrap();
        assert_eq!(
            got,
            Some(ProductName {
                title: Some("Licuadora".to_owned()),
                brand: Some("Oster".to_owned()),
            })
        );

        let empty = FakeProducts::default();
        let matched = lookup(&Matches(Ok(true)), &empty, &asin(), false).await;
        assert_eq!(matched, Ok(Some(ProductName::default())));
        let mine = lookup(&Matches(Ok(false)), &empty, &asin(), true).await;
        assert_eq!(mine, Ok(Some(ProductName::default())));
        let unknown = lookup(&Matches(Ok(false)), &empty, &asin(), false).await;
        assert_eq!(unknown, Ok(None));
    }

    #[tokio::test]
    async fn lookup_storage_errors_pass_through() {
        let failing = FakeProducts {
            fail: true,
            ..FakeProducts::default()
        };
        let got = lookup(&Matches(Ok(true)), &failing, &asin(), true).await;
        assert_eq!(got, Err(RepoError::Unavailable));
        let got = lookup(
            &Matches(Err(RepoError::Unavailable)),
            &FakeProducts::default(),
            &asin(),
            false,
        )
        .await;
        assert_eq!(got, Err(RepoError::Unavailable));
    }
}
