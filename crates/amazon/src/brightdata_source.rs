//! [`AmazonSource`] over Bright Data's "Amazon products global dataset"
//! (Datasets v3), the fallback while SP-API is unavailable or not yet
//! authorized (ADR 0010).
//!
//! Every lookup is one dataset run through [`Runner`], so the kill switch,
//! spend caps and durable run records apply. Only records from
//! `amazon.com.mx` are used, and prices only when the record is in MXN.

use std::sync::{Arc, Mutex as StdMutex};

use app::{
    AmazonDataSource, AmazonSource, AmazonSourceError, CatalogItem, CatalogResult, FeeEstimate,
    Fulfillment, OfferSummary, PortFuture, SalesRank,
};
use brightdata::runs::{
    Caps, Clock, RunError, RunScope, RunSink, RunStatus, RunStore, Runner, Timing,
};
use brightdata::{DatasetId, DatasetTrigger, DiscoverBy, TargetUrl, Transport, UnlockedPage};
use domain::{Asin, Gtin, PriceMxn, UnixMillis};
use http_guard::Url;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::client::valid_keywords;
use crate::parse::{MXN, OfferPrices};

/// Bright Data "Amazon products global dataset".
pub const AMAZON_GLOBAL_DATASET_ID: &str = "gd_lwhideng15g8jg63s7";
/// The `domain` input for Amazon Mexico.
pub const AMAZON_MX_ORIGIN: &str = "https://www.amazon.com.mx";
/// Records per discovery input (each record is billed).
pub const LIMIT_PER_INPUT: u32 = 5;
/// Run scope tenant for system-owned lookups.
pub const FALLBACK_TENANT: &str = "system";
/// Run scope job for this fallback.
pub const FALLBACK_JOB: &str = "amazon_mx_fallback";

/// Hosts accepted as Amazon Mexico.
const MX_HOSTS: [&str; 2] = ["www.amazon.com.mx", "amazon.com.mx"];

/// Collects downloaded records in memory.
#[derive(Default)]
struct CollectingSink {
    records: StdMutex<Vec<Value>>,
}

impl CollectingSink {
    fn take(&self) -> Vec<Value> {
        self.records
            .lock()
            .map(|mut records| std::mem::take(&mut *records))
            .unwrap_or_default()
    }
}

impl RunSink for CollectingSink {
    async fn accept_page(&self, _url: &TargetUrl, _page: UnlockedPage) -> Result<(), String> {
        Err("the Amazon fallback expects dataset records, not pages".to_owned())
    }

    async fn accept_records(&self, records: Vec<Value>) -> Result<u64, String> {
        let count = u64::try_from(records.len()).unwrap_or(u64::MAX);
        let mut held = self
            .records
            .lock()
            .map_err(|_| "record buffer unavailable".to_owned())?;
        held.extend(records);
        Ok(count)
    }
}

/// Bright Data's Amazon dataset as an [`AmazonSource`]. Lookups run one at
/// a time (a run scope allows one active run). Fees are `Unsupported`.
pub struct BrightDataAmazonSource<T, S, C> {
    transport: Arc<T>,
    store: Arc<S>,
    clock: C,
    scope: RunScope,
    timing: Timing,
    caps: Caps,
    one_at_a_time: Mutex<()>,
}

impl<T: Transport, S: RunStore, C: Clock> BrightDataAmazonSource<T, S, C> {
    /// A source running under scope `system` / `amazon_mx_fallback` with
    /// the default caps and poll timing.
    ///
    /// # Errors
    ///
    /// [`RunError::Invalid`] if the fixed scope is rejected (never in
    /// practice).
    pub fn new(transport: Arc<T>, store: Arc<S>, clock: C) -> Result<Self, RunError> {
        Ok(Self {
            transport,
            store,
            clock,
            scope: RunScope::new(FALLBACK_TENANT, FALLBACK_JOB)?,
            timing: Timing::default(),
            caps: Caps::default(),
            one_at_a_time: Mutex::new(()),
        })
    }

    /// The same source with other poll timing.
    #[must_use]
    pub fn with_timing(mut self, timing: Timing) -> Self {
        self.timing = timing;
        self
    }

    fn now(&self) -> UnixMillis {
        UnixMillis(self.clock.now_ms())
    }

    /// Runs one collection and returns its records. A run still collecting
    /// when the poll budget ends is cancelled (it would block the scope).
    async fn collect(&self, trigger: DatasetTrigger) -> Result<Vec<Value>, AmazonSourceError> {
        let _one = self.one_at_a_time.lock().await;
        let runner = Runner {
            transport: Some(self.transport.as_ref()),
            store: self.store.as_ref(),
            clock: &self.clock,
            timing: self.timing,
            caps: self.caps,
            robots: None,
        };
        let sink = CollectingSink::default();
        let outcome = runner
            .start_dataset_run(&self.scope, &trigger, &sink)
            .await
            .map_err(|error| match error {
                RunError::Invalid(_) => AmazonSourceError::InvalidInput,
                _ => AmazonSourceError::Unavailable,
            })?;
        match outcome.status {
            RunStatus::Imported => Ok(sink.take()),
            status if status.is_active() => {
                let _ = runner.cancel_run(outcome.run_id).await;
                Err(AmazonSourceError::Unavailable)
            }
            _ => Err(AmazonSourceError::Unavailable),
        }
    }

    async fn discover(&self, keywords: &str) -> Result<Vec<Value>, AmazonSourceError> {
        let trigger = DatasetTrigger {
            dataset_id: dataset_id()?,
            inputs: vec![json!({
                "keywords": keywords,
                "domain": AMAZON_MX_ORIGIN,
                "pages_to_search": 1,
            })],
            discover_by: Some(
                DiscoverBy::parse("keywords").map_err(|_| AmazonSourceError::InvalidInput)?,
            ),
            limit_per_input: Some(LIMIT_PER_INPUT),
        };
        self.collect(trigger).await
    }

    fn catalog(&self, items: Vec<CatalogItem>) -> CatalogResult {
        CatalogResult {
            items,
            source: AmazonDataSource::BrightData,
            fetched_at: self.now(),
        }
    }
}

fn dataset_id() -> Result<DatasetId, AmazonSourceError> {
    DatasetId::parse(AMAZON_GLOBAL_DATASET_ID).map_err(|_| AmazonSourceError::InvalidInput)
}

impl<T: Transport, S: RunStore, C: Clock> AmazonSource for BrightDataAmazonSource<T, S, C> {
    fn catalog_by_gtin<'a>(
        &'a self,
        gtin: &'a Gtin,
    ) -> PortFuture<'a, Result<CatalogResult, AmazonSourceError>> {
        Box::pin(async move {
            let records = self.discover(gtin.as_str()).await?;
            let items = records
                .iter()
                .filter(|record| upc_matches(record, gtin))
                .filter_map(catalog_item)
                .collect();
            Ok(self.catalog(dedupe(items)))
        })
    }

    fn catalog_by_keyword<'a>(
        &'a self,
        keywords: &'a str,
    ) -> PortFuture<'a, Result<CatalogResult, AmazonSourceError>> {
        Box::pin(async move {
            let keywords = valid_keywords(keywords).map_err(|_| AmazonSourceError::InvalidInput)?;
            let records = self.discover(&keywords).await?;
            let items = records.iter().filter_map(catalog_item).collect();
            Ok(self.catalog(dedupe(items)))
        })
    }

    fn offers<'a>(
        &'a self,
        asin: &'a Asin,
    ) -> PortFuture<'a, Result<OfferSummary, AmazonSourceError>> {
        Box::pin(async move {
            let url = format!("{AMAZON_MX_ORIGIN}/dp/{}", asin.as_str());
            let trigger = DatasetTrigger {
                dataset_id: dataset_id()?,
                inputs: vec![json!({ "url": url })],
                discover_by: None,
                limit_per_input: None,
            };
            let records = self.collect(trigger).await?;
            let prices = offer_prices(&records, asin)?;
            Ok(prices.into_summary(AmazonDataSource::BrightData, self.now()))
        })
    }

    fn fees<'a>(
        &'a self,
        _asin: &'a Asin,
        _price: PriceMxn,
        _fulfillment: Fulfillment,
    ) -> PortFuture<'a, Result<FeeEstimate, AmazonSourceError>> {
        Box::pin(async { Err(AmazonSourceError::Unsupported) })
    }
}

// ---- Record mapping (pure) ------------------------------------------------

fn text<'a>(record: &'a Value, field: &str) -> Option<&'a str> {
    record
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn host_is_mx(value: &str) -> bool {
    Url::parse(value)
        .ok()
        .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
        .is_some_and(|host| MX_HOSTS.contains(&host.as_str()))
}

/// True when the record comes from Amazon Mexico (its `domain`, else its
/// `url`); records without either are rejected.
pub(crate) fn is_mx_record(record: &Value) -> bool {
    match text(record, "domain") {
        Some(domain) => host_is_mx(domain),
        None => text(record, "url").is_some_and(host_is_mx),
    }
}

/// Bright Data's label for Mexican pesos on amazon.com.mx records (seen in
/// the 2026-10-19 live check). Not an ISO 4217 code, so it cannot mean any
/// other currency; it is accepted only because every caller has already
/// required an amazon.com.mx record.
const BRIGHTDATA_MX_PESO: &str = "MXD";

/// True unless the record names a currency other than Mexican pesos (`MXN`,
/// or Bright Data's `MXD`). Call only on records that passed
/// [`is_mx_record`].
fn currency_allows(record: &Value, required: bool) -> bool {
    match text(record, "currency") {
        Some(currency) => {
            currency.eq_ignore_ascii_case(MXN) || currency.eq_ignore_ascii_case(BRIGHTDATA_MX_PESO)
        }
        None => !required,
    }
}

fn digits_without_leading_zeros(text: &str) -> &str {
    text.trim_start_matches('0')
}

/// True when the record's `upc` field (one or more codes) contains `gtin`,
/// ignoring leading zeros (UPC-A `012…` equals EAN-13 `0012…`).
pub(crate) fn upc_matches(record: &Value, gtin: &Gtin) -> bool {
    let wanted = digits_without_leading_zeros(gtin.as_str());
    text(record, "upc").is_some_and(|codes| {
        codes
            .split(|c: char| !c.is_ascii_digit())
            .filter(|code| !code.is_empty())
            .any(|code| digits_without_leading_zeros(code) == wanted)
    })
}

fn rank(record: &Value, rank_field: &str, category_field: &str) -> Option<SalesRank> {
    let rank = record.get(rank_field)?.as_u64()?;
    Some(SalesRank {
        category: text(record, category_field).unwrap_or_default().to_owned(),
        rank: u32::try_from(rank).ok()?,
    })
}

/// A catalog item from an MX record (non-MXN currencies are dropped).
pub(crate) fn catalog_item(record: &Value) -> Option<CatalogItem> {
    if !is_mx_record(record) || !currency_allows(record, false) {
        return None;
    }
    let asin = Asin::new(text(record, "asin")?).ok()?;
    let title = text(record, "title")?
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let brand = text(record, "brand").map(str::to_owned);
    let mut images: Vec<String> = Vec::new();
    let listed = record
        .get("images")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str);
    for link in text(record, "image_url").into_iter().chain(listed) {
        if link.starts_with("https://") && !images.iter().any(|seen| seen == link) {
            images.push(link.to_owned());
        }
    }
    let mut sales_ranks: Vec<SalesRank> = [
        rank(record, "bs_rank", "bs_category"),
        rank(record, "root_bs_rank", "root_bs_category"),
    ]
    .into_iter()
    .flatten()
    .collect();
    sales_ranks.sort_by(|a, b| {
        a.rank
            .cmp(&b.rank)
            .then_with(|| a.category.cmp(&b.category))
    });
    sales_ranks.dedup();
    Some(CatalogItem {
        asin,
        title,
        brand,
        images,
        sales_ranks,
    })
}

fn dedupe(items: Vec<CatalogItem>) -> Vec<CatalogItem> {
    let mut out: Vec<CatalogItem> = Vec::with_capacity(items.len());
    for item in items {
        if !out.iter().any(|seen| seen.asin == item.asin) {
            out.push(item);
        }
    }
    out
}

fn price(value: &Value) -> Option<PriceMxn> {
    match value {
        Value::Number(number) => PriceMxn::from_decimal_str(&number.to_string()).ok(),
        Value::String(text) => PriceMxn::parse_mxn(text).ok(),
        _ => None,
    }
}

/// Offer prices for `asin` from a product-page collection. No records means
/// no offers; records that exist but are not Amazon Mexico in MXN fail.
pub(crate) fn offer_prices(
    records: &[Value],
    asin: &Asin,
) -> Result<OfferPrices, AmazonSourceError> {
    if records.is_empty() {
        return Ok(OfferPrices::none());
    }
    let same_asin = |record: &&Value| {
        ["asin", "input_asin"]
            .iter()
            .filter_map(|field| text(record, field))
            .any(|got| got.eq_ignore_ascii_case(asin.as_str()))
    };
    let record = records
        .iter()
        .filter(same_asin)
        .find(|record| is_mx_record(record) && currency_allows(record, true))
        .ok_or(AmazonSourceError::ParseFailed)?;
    let available = record.get("is_available").and_then(Value::as_bool) != Some(false);
    let buy_box_mxn = record
        .get("final_price")
        .and_then(price)
        .filter(|_| available);
    let offer_count = record
        .get("number_of_sellers")
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(u32::from(buy_box_mxn.is_some()));
    Ok(OfferPrices {
        buy_box_mxn,
        lowest_new_mxn: None,
        offer_count,
    })
}

#[cfg(test)]
mod tests;
