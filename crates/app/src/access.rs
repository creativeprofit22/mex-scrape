//! Use case: the Bright Data access report (`blocked_needs_kyc` and the
//! other outcomes) per host and per retailer.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::ports::{AccessCountsRepo, AccessTotals, OfferSource};

/// Why the access report could not be built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AccessReportError {
    /// The stored counts could not be read.
    #[error("access counts could not be read")]
    Storage,
}

/// Access totals for one host, labelled with its retailer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostAccessReport {
    /// Retailer id from the registry, or the host when it is not registered.
    pub retailer: String,
    /// Target host.
    pub host: String,
    /// Outcome totals.
    pub totals: AccessTotals,
}

/// Builds access reports from injected ports; never constructs adapters.
pub struct AccessReport {
    counts: Arc<dyn AccessCountsRepo>,
    offers: Arc<dyn OfferSource>,
}

impl AccessReport {
    /// A report reading `counts` and labelling hosts through `offers`.
    #[must_use]
    pub fn new(counts: Arc<dyn AccessCountsRepo>, offers: Arc<dyn OfferSource>) -> Self {
        Self { counts, offers }
    }

    fn label(&self, host: &str) -> String {
        self.offers
            .retailer_for_host(host)
            .map_or_else(|| host.to_owned(), |id| id.as_str().to_owned())
    }

    /// One row per stored host, in the repository's order.
    ///
    /// # Errors
    /// [`AccessReportError::Storage`] if the counts cannot be read.
    pub async fn per_host(&self) -> Result<Vec<HostAccessReport>, AccessReportError> {
        let hosts = self
            .counts
            .counts_by_host()
            .await
            .map_err(|_| AccessReportError::Storage)?;
        Ok(hosts
            .into_iter()
            .map(|h| HostAccessReport {
                retailer: self.label(&h.host),
                host: h.host,
                totals: h.totals,
            })
            .collect())
    }

    /// Totals summed per retailer id (unregistered hosts under their host
    /// name), sorted by label.
    ///
    /// # Errors
    /// [`AccessReportError::Storage`] if the counts cannot be read.
    pub async fn per_retailer(&self) -> Result<BTreeMap<String, AccessTotals>, AccessReportError> {
        let mut totals: BTreeMap<String, AccessTotals> = BTreeMap::new();
        for row in self.per_host().await? {
            totals.entry(row.retailer).or_default().add(row.totals);
        }
        Ok(totals)
    }
}

#[cfg(test)]
mod tests {
    use domain::{Offer, RetailerId, RetailerSku};

    use super::*;
    use crate::ports::{HostAccess, OfferSourceError, PortFuture, RepoError};

    struct FakeCounts(Result<Vec<HostAccess>, RepoError>);

    impl AccessCountsRepo for FakeCounts {
        fn counts_by_host(&self) -> PortFuture<'_, Result<Vec<HostAccess>, RepoError>> {
            Box::pin(async move { self.0.clone() })
        }
    }

    /// Knows one retailer, `liverpool`, by a case-insensitive host match.
    struct FakeOffers;

    impl OfferSource for FakeOffers {
        fn retailer_for_host(&self, host: &str) -> Option<RetailerId> {
            host.eq_ignore_ascii_case("www.liverpool.com.mx")
                .then(|| RetailerId::new("liverpool").unwrap())
        }

        fn search<'a>(
            &'a self,
            _: &'a RetailerId,
            _: &'a str,
        ) -> PortFuture<'a, Result<Vec<Offer>, OfferSourceError>> {
            Box::pin(async { Err(OfferSourceError::Unavailable) })
        }

        fn product<'a>(
            &'a self,
            _: &'a RetailerId,
            _: &'a RetailerSku,
        ) -> PortFuture<'a, Result<Offer, OfferSourceError>> {
            Box::pin(async { Err(OfferSourceError::Unavailable) })
        }
    }

    fn host(name: &str, ok: i64, blocked: i64) -> HostAccess {
        HostAccess {
            host: name.to_owned(),
            totals: AccessTotals {
                ok,
                blocked_needs_kyc: blocked,
                skipped_robots: 1,
                failed: 0,
            },
        }
    }

    fn report(counts: Result<Vec<HostAccess>, RepoError>) -> AccessReport {
        AccessReport::new(Arc::new(FakeCounts(counts)), Arc::new(FakeOffers))
    }

    #[tokio::test]
    async fn per_host_labels_registered_hosts_and_keeps_unknown_hosts() {
        let rows = report(Ok(vec![
            host("www.liverpool.com.mx", 1, 2),
            host("other.example", 5, 0),
        ]))
        .per_host()
        .await
        .unwrap();
        let labels: Vec<(&str, &str)> = rows
            .iter()
            .map(|r| (r.retailer.as_str(), r.host.as_str()))
            .collect();
        assert_eq!(
            labels,
            [
                ("liverpool", "www.liverpool.com.mx"),
                ("other.example", "other.example")
            ]
        );
        assert_eq!(rows[0].totals.blocked_needs_kyc, 2);
        assert_eq!(rows[1].totals.ok, 5);
    }

    #[tokio::test]
    async fn per_retailer_groups_case_variants_and_sorts_by_label() {
        let totals = report(Ok(vec![
            host("www.liverpool.com.mx", 1, 2),
            host("zz.example", 7, 0),
            host("WWW.LIVERPOOL.COM.MX", 0, 1),
            host("other.example", 5, 0),
        ]))
        .per_retailer()
        .await
        .unwrap();
        let keys: Vec<&str> = totals.keys().map(String::as_str).collect();
        assert_eq!(keys, ["liverpool", "other.example", "zz.example"]);
        assert_eq!(
            totals["liverpool"],
            AccessTotals {
                ok: 1,
                blocked_needs_kyc: 3,
                skipped_robots: 2,
                failed: 0
            }
        );
        assert_eq!(totals["other.example"].ok, 5);
    }

    #[tokio::test]
    async fn repo_failure_is_a_storage_error() {
        let failing = report(Err(RepoError::Unavailable));
        assert_eq!(failing.per_host().await, Err(AccessReportError::Storage));
        assert_eq!(
            failing.per_retailer().await,
            Err(AccessReportError::Storage)
        );
    }

    #[test]
    fn totals_add_saturates() {
        let mut total = AccessTotals {
            ok: i64::MAX,
            ..AccessTotals::default()
        };
        total.add(AccessTotals {
            ok: 1,
            failed: 2,
            ..AccessTotals::default()
        });
        assert_eq!(total.ok, i64::MAX);
        assert_eq!(total.failed, 2);
    }
}
