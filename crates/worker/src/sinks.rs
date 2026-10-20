//! The [`RunSink`] used by jobs: parses every fetched page with its
//! retailer's adapter (through `retailers::parse_page`, so the shared
//! [`ParseHealth`] sees every parse) and collects the offers for the job to
//! process once the run is over.
//!
//! A page that does not parse is still accepted: the Bright Data access
//! was fine, and parse failures are counted in [`JobMetrics`] and the parse
//! health instead of being reported as failed fetches.

use std::sync::{Mutex, PoisonError};

use brightdata::runs::{PageAccess, RunSink};
use brightdata::{TargetUrl, UnlockedPage};
use domain::{Offer, UnixMillis};
use retailers::{Page, ParseHealth, ParseKind, parse_page};
use serde_json::Value;

use crate::metrics::JobMetrics;

/// Refusal text for pages on hosts no adapter owns.
pub const UNKNOWN_HOST: &str = "Fetched page is not on a registered retailer host";
/// Refusal text for dataset records (jobs only start page runs).
pub const NO_RECORDS: &str = "This job does not import dataset records";

/// What a collector gathered.
#[derive(Debug, Default)]
pub struct Collected {
    /// Offers in fetch order, tagged with the retailer id.
    pub offers: Vec<(&'static str, Offer)>,
    /// Page and parse counters (access outcomes are added by the job).
    pub metrics: JobMetrics,
}

/// Collects offers from the pages of one run.
pub struct OfferCollector<'a> {
    kind: ParseKind,
    access: PageAccess,
    fetched_at: UnixMillis,
    health: &'a Mutex<ParseHealth>,
    state: Mutex<Collected>,
}

impl<'a> OfferCollector<'a> {
    /// Parses pages as `kind`, fetched with `access`, stamped `fetched_at`.
    #[must_use]
    pub fn new(
        kind: ParseKind,
        access: PageAccess,
        fetched_at: UnixMillis,
        health: &'a Mutex<ParseHealth>,
    ) -> Self {
        Self {
            kind,
            access,
            fetched_at,
            health,
            state: Mutex::new(Collected::default()),
        }
    }

    /// The offers and counters gathered so far.
    #[must_use]
    pub fn into_collected(self) -> Collected {
        self.state
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

impl RunSink for OfferCollector<'_> {
    async fn accept_page(&self, url: &TargetUrl, page: UnlockedPage) -> Result<(), String> {
        let Some(adapter) = retailers::retailer_for_host(&url.host())
            .and_then(|info| retailers::adapter_for(info.id))
        else {
            return Err(UNKNOWN_HOST.to_owned());
        };
        let bytes = page.body.len();
        let page = Page {
            url: url.clone(),
            body: page.body,
            fetched_at: self.fetched_at,
        };
        let parsed = {
            // A panic elsewhere mid-record only leaves a counter half-done.
            let mut health = self.health.lock().unwrap_or_else(PoisonError::into_inner);
            parse_page(adapter, self.kind, &page, &mut health)
        };
        let id = adapter.info().id;
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let offers = parsed.unwrap_or_default();
        state
            .metrics
            .page(id, self.access, bytes, !offers.is_empty());
        state.offers.extend(offers.into_iter().map(|o| (id, o)));
        Ok(())
    }

    async fn accept_records(&self, _records: Vec<Value>) -> Result<u64, String> {
        Err(NO_RECORDS.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use retailers::HealthConfig;

    use super::*;

    fn health() -> Mutex<ParseHealth> {
        Mutex::new(ParseHealth::new(HealthConfig::default()))
    }

    #[tokio::test]
    async fn unknown_hosts_and_records_are_refused() {
        let h = health();
        let sink = OfferCollector::new(ParseKind::Search, PageAccess::Unlocker, UnixMillis(1), &h);
        let url = TargetUrl::parse("https://example.com/x").unwrap();
        let page = UnlockedPage {
            status: 200,
            body: String::new(),
        };
        assert_eq!(
            sink.accept_page(&url, page).await,
            Err(UNKNOWN_HOST.to_owned())
        );
        assert_eq!(
            sink.accept_records(Vec::new()).await,
            Err(NO_RECORDS.to_owned())
        );
        assert!(sink.into_collected().offers.is_empty());
    }

    #[tokio::test]
    async fn unparsable_page_is_accepted_and_counted() {
        let h = health();
        let sink = OfferCollector::new(ParseKind::Search, PageAccess::Unlocker, UnixMillis(1), &h);
        let url = TargetUrl::parse("https://www.walmart.com.mx/search?q=x").unwrap();
        let page = UnlockedPage {
            status: 200,
            body: "<html></html>".to_owned(),
        };
        assert_eq!(sink.accept_page(&url, page).await, Ok(()));
        let c = sink.into_collected();
        assert!(c.offers.is_empty());
        let m = c.metrics.get("walmart_mx");
        assert_eq!((m.parse_ok, m.parse_failed, m.unlocker_pages), (0, 1, 1));
        assert_eq!(h.lock().unwrap().rate("walmart_mx"), Some((0.0, 1)));
    }
}
