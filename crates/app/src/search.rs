//! Use cases: start a background search and read its results. The search
//! itself runs in the worker (ADR 0011); these only enqueue and read.

use std::sync::Arc;

use domain::{Memberships, SearchQuery, SearchQueryError, TenantId};

use crate::ports::{SearchId, SearchRepo, SearchView};

/// Why a search could not be started or read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SearchError {
    /// The query is empty or too long.
    #[error("invalid search query: {0}")]
    InvalidQuery(SearchQueryError),
    /// Searches could not be stored or read.
    #[error("searches could not be stored or read")]
    Storage,
}

/// Validates a query, stores the search and enqueues its job.
pub struct StartSearch {
    repo: Arc<dyn SearchRepo>,
}

impl StartSearch {
    /// Starts searches through `repo`.
    #[must_use]
    pub fn new(repo: Arc<dyn SearchRepo>) -> Self {
        Self { repo }
    }

    /// Cleans `raw` and enqueues a search owned by `tenant`.
    ///
    /// # Errors
    /// [`SearchError::InvalidQuery`] before anything is stored;
    /// [`SearchError::Storage`] if the search could not be enqueued.
    pub async fn start(&self, tenant: &TenantId, raw: &str) -> Result<SearchId, SearchError> {
        let query = SearchQuery::parse(raw).map_err(SearchError::InvalidQuery)?;
        self.repo
            .create_and_enqueue(tenant, &query)
            .await
            .map_err(|_| SearchError::Storage)
    }
}

/// Reads a tenant's search with its outcomes and results.
pub struct SearchResults {
    repo: Arc<dyn SearchRepo>,
}

impl SearchResults {
    /// Reads searches through `repo`.
    #[must_use]
    pub fn new(repo: Arc<dyn SearchRepo>) -> Self {
        Self { repo }
    }

    /// The search `id` of `tenant`; `None` when unknown or owned by another
    /// tenant. Results that need a membership not in `memberships` are
    /// removed and counted in [`SearchView::hidden_results`].
    ///
    /// # Errors
    /// [`SearchError::Storage`] if it could not be read.
    pub async fn get(
        &self,
        tenant: &TenantId,
        id: SearchId,
        memberships: Memberships,
    ) -> Result<Option<SearchView>, SearchError> {
        let view = self
            .repo
            .load(tenant, id)
            .await
            .map_err(|_| SearchError::Storage)?;
        Ok(view.map(|mut v| {
            let before = v.results.len();
            v.results
                .retain(|r| memberships.allows(r.membership_required, &r.retailer));
            let hidden = before - v.results.len();
            v.hidden_results = u32::try_from(hidden).unwrap_or(u32::MAX);
            v
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use domain::{Availability, PriceMxn, RetailerId, RetailerSku, SearchStatus, UnixMillis};

    use super::*;
    use crate::ports::{PortFuture, RepoError, SearchResultView};

    /// A stored search: its query and results.
    #[derive(Clone)]
    struct Stored {
        query: String,
        results: Vec<SearchResultView>,
    }

    /// In-memory searches; `fail` makes every call fail.
    #[derive(Default)]
    struct FakeRepo {
        rows: Mutex<Vec<(TenantId, Stored)>>,
        fail: bool,
    }

    impl SearchRepo for FakeRepo {
        fn create_and_enqueue<'a>(
            &'a self,
            tenant: &'a TenantId,
            query: &'a SearchQuery,
        ) -> PortFuture<'a, Result<SearchId, RepoError>> {
            Box::pin(async move {
                if self.fail {
                    return Err(RepoError::Unavailable);
                }
                let mut rows = self.rows.lock().unwrap();
                rows.push((
                    tenant.clone(),
                    Stored {
                        query: query.as_str().to_owned(),
                        results: Vec::new(),
                    },
                ));
                Ok(SearchId(i64::try_from(rows.len()).unwrap()))
            })
        }

        fn load<'a>(
            &'a self,
            tenant: &'a TenantId,
            id: SearchId,
        ) -> PortFuture<'a, Result<Option<SearchView>, RepoError>> {
            Box::pin(async move {
                if self.fail {
                    return Err(RepoError::Unavailable);
                }
                let rows = self.rows.lock().unwrap();
                let idx = usize::try_from(id.0 - 1).ok();
                Ok(idx
                    .and_then(|i| rows.get(i))
                    .filter(|(t, _)| t == tenant)
                    .map(|(_, q)| SearchView {
                        id,
                        query: q.query.clone(),
                        status: SearchStatus::Queued,
                        reason: String::new(),
                        created_at: UnixMillis(1),
                        finished_at: None,
                        retailers: Vec::new(),
                        results: q.results.clone(),
                        hidden_results: 0,
                    }))
            })
        }
    }

    const NONE: Memberships = Memberships {
        costco: false,
        sams: false,
    };

    fn result(retailer: &str, membership_required: bool) -> SearchResultView {
        SearchResultView {
            retailer: RetailerId::new(retailer).unwrap(),
            retailer_sku: RetailerSku::new("1").unwrap(),
            title: "Taladro".to_owned(),
            url: "https://example.mx/p/1".to_owned(),
            price_mxn: PriceMxn::from_centavos(100).unwrap(),
            availability: Availability::InStock,
            membership_required,
            amazon: None,
        }
    }

    #[tokio::test]
    async fn membership_results_are_hidden_unless_the_reader_has_it() {
        let repo = Arc::new(FakeRepo::default());
        let me = TenantId::default_tenant();
        let id = StartSearch::new(repo.clone())
            .start(&me, "taladro")
            .await
            .unwrap();
        repo.rows.lock().unwrap()[0].1.results = vec![
            result("costco_mx", true),
            result("homedepot_mx", false),
            result("sams_mx", true),
        ];
        let read = SearchResults::new(repo);
        let shown = |v: SearchView| {
            let names: Vec<String> = v
                .results
                .iter()
                .map(|r| r.retailer.as_str().to_owned())
                .collect();
            (names, v.hidden_results)
        };
        let none = read.get(&me, id, NONE).await.unwrap().unwrap();
        assert_eq!(shown(none), (vec!["homedepot_mx".to_owned()], 2));
        let costco = Memberships {
            costco: true,
            sams: false,
        };
        let v = read.get(&me, id, costco).await.unwrap().unwrap();
        assert_eq!(
            shown(v),
            (vec!["costco_mx".to_owned(), "homedepot_mx".to_owned()], 1)
        );
        let both = Memberships {
            costco: true,
            sams: true,
        };
        assert_eq!(
            read.get(&me, id, both)
                .await
                .unwrap()
                .unwrap()
                .hidden_results,
            0
        );
    }

    #[tokio::test]
    async fn start_cleans_the_query_and_reads_back_for_the_owner_only() {
        let repo = Arc::new(FakeRepo::default());
        let start = StartSearch::new(repo.clone());
        let read = SearchResults::new(repo.clone());
        let me = TenantId::default_tenant();
        let id = start.start(&me, "  taladro   inalámbrico ").await.unwrap();
        let view = read.get(&me, id, NONE).await.unwrap().unwrap();
        assert_eq!(view.query, "taladro inalámbrico");
        let other = TenantId::new("other").unwrap();
        assert_eq!(read.get(&other, id, NONE).await.unwrap(), None);
        assert_eq!(read.get(&me, SearchId(99), NONE).await.unwrap(), None);
    }

    #[tokio::test]
    async fn invalid_query_stores_nothing() {
        let repo = Arc::new(FakeRepo::default());
        let start = StartSearch::new(repo.clone());
        let me = TenantId::default_tenant();
        assert_eq!(
            start.start(&me, "   ").await,
            Err(SearchError::InvalidQuery(SearchQueryError::Empty))
        );
        assert_eq!(
            start.start(&me, &"x".repeat(101)).await,
            Err(SearchError::InvalidQuery(SearchQueryError::TooLong))
        );
        assert!(repo.rows.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn storage_failures_are_reported_without_detail() {
        let repo = Arc::new(FakeRepo {
            fail: true,
            ..FakeRepo::default()
        });
        let me = TenantId::default_tenant();
        assert_eq!(
            StartSearch::new(repo.clone()).start(&me, "taladro").await,
            Err(SearchError::Storage)
        );
        assert_eq!(
            SearchResults::new(repo).get(&me, SearchId(1), NONE).await,
            Err(SearchError::Storage)
        );
    }
}
