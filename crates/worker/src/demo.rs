//! Demo mode (`DEMO_DATA=true`, ADR 0016): a fixture-backed stand-in for
//! the Bright Data connector, so search jobs run the real worker → db →
//! API path against the products in `config/demo_catalog.json` with no
//! network call at all.
//!
//! - [`DemoTransport`] implements [`brightdata::Transport`]. Web Unlocker
//!   and Browser fetches of a registered store's search URL return a page
//!   with one JSON-LD `Product` per catalog listing of that store whose
//!   keyword appears in the URL; a product URL returns that listing's page.
//!   Datasets v3 triggers for the Amazon fallback answer at once with
//!   Amazon.com.mx records (GTIN or keyword discovery, product URL).
//! - [`DemoFees`] answers Amazon fee estimates, which the Bright Data
//!   fallback cannot (`Unsupported`): a 15 % referral fee plus the
//!   catalog's FBA fee. Every other lookup is `NotConfigured`, so
//!   [`app::FallbackAmazonSource`] sends it on to the Bright Data fallback.
//!
//! Runs still go through `brightdata::runs::Runner`, so the kill switch,
//! the connector flag, the caps and the run records all apply.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use app::{
    AmazonDataSource, AmazonSource, AmazonSourceError, CatalogResult, FeeDetail, FeeEstimate,
    Fulfillment, OfferSummary, PortFuture,
};
use brightdata::{
    BrightDataError, DatasetTrigger, Download, SnapshotId, SnapshotStatus, TargetUrl, Transport,
    UnlockRequest, UnlockedPage,
};
use db::demo_catalog::{DemoCatalog, DemoListing, DemoProduct};
use domain::{Asin, PriceMxn};
use retailers::Retailer;
use serde_json::{Value, json};

/// Amazon Mexico origin used in the dataset records.
const AMAZON_MX: &str = "https://www.amazon.com.mx";
/// Records per discovery input, as the real fallback asks for.
const DISCOVERY_LIMIT: usize = 5;
/// Shared title words a keyword record needs (the matcher scores the rest).
const MIN_SHARED_WORDS: usize = 2;
/// Amazon's referral fee in this demo, in per cent of the sale price.
pub const REFERRAL_FEE_PERCENT: i64 = 15;

/// `1499.00` from 149 900 centavos.
fn pesos(centavos: i64) -> String {
    format!("{}.{:02}", centavos / 100, centavos % 100)
}

/// Decodes `%XX` escapes and `+` in a URL, so query words can be compared.
fn decode_url(url: &str) -> String {
    let bytes = url.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(byte) => {
                        out.push(byte);
                        i += 3;
                        continue;
                    }
                    None => out.push(b'%'),
                }
            }
            b'+' => out.push(b' '),
            byte => out.push(byte),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The fixture-backed Bright Data connector of demo mode.
pub struct DemoTransport {
    catalog: DemoCatalog,
    snapshots: Mutex<BTreeMap<String, Vec<Value>>>,
    next_snapshot: AtomicU64,
}

impl DemoTransport {
    /// Serves `catalog`.
    #[must_use]
    pub fn new(catalog: DemoCatalog) -> Self {
        Self {
            catalog,
            snapshots: Mutex::new(BTreeMap::new()),
            next_snapshot: AtomicU64::new(1),
        }
    }

    fn snapshots(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Vec<Value>>> {
        self.snapshots
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// The HTML a store would return for `url`.
    ///
    /// # Errors
    ///
    /// [`BrightDataError::Terminal`] for a host no adapter owns.
    pub fn page(&self, url: &TargetUrl) -> Result<String, BrightDataError> {
        let adapter = retailers::retailer_for_host(&url.host())
            .and_then(|info| retailers::adapter_for(info.id))
            .ok_or_else(|| {
                BrightDataError::Terminal("demo: no store serves this host".to_owned())
            })?;
        let id = adapter.info().id;
        let here = || {
            self.catalog
                .listings()
                .filter(move |(_, l)| l.retailer == id)
        };
        for (product, listing) in here() {
            if product_url(adapter, listing).is_some_and(|u| u == url.as_str()) {
                return Ok(product_page(adapter, product, listing));
            }
        }
        let words = matching::tokens(&decode_url(url.as_str()));
        let found: Vec<Value> = here()
            .filter(|(product, _)| {
                product.keywords.iter().any(|keyword| {
                    let wanted = matching::tokens(keyword);
                    !wanted.is_empty() && wanted.is_subset(&words)
                })
            })
            .filter_map(|(product, listing)| product_json(adapter, product, listing))
            .collect();
        Ok(search_page(&found))
    }

    /// Dataset records for one trigger input.
    #[must_use]
    pub fn records(&self, input: &Value) -> Vec<Value> {
        let amazon = || {
            self.catalog
                .products
                .iter()
                .filter_map(|p| p.amazon.as_ref().map(|a| (p, a)))
        };
        if let Some(url) = input.get("url").and_then(Value::as_str) {
            let asin = url.rsplit("/dp/").next().unwrap_or_default();
            return amazon()
                .filter(|(_, a)| url.contains("/dp/") && a.asin == asin)
                .map(|(p, _)| amazon_record(p))
                .collect();
        }
        let Some(keywords) = input.get("keywords").and_then(Value::as_str) else {
            return Vec::new();
        };
        let keywords = keywords.trim();
        if !keywords.is_empty() && keywords.bytes().all(|b| b.is_ascii_digit()) {
            let wanted = keywords.trim_start_matches('0');
            return amazon()
                .filter(|(p, _)| {
                    p.gtin
                        .as_deref()
                        .is_some_and(|g| g.trim_start_matches('0') == wanted)
                })
                .map(|(p, _)| amazon_record(p))
                .take(DISCOVERY_LIMIT)
                .collect();
        }
        let words = matching::tokens(keywords);
        amazon()
            .filter(|(_, a)| {
                matching::tokens(&a.title).intersection(&words).count() >= MIN_SHARED_WORDS
            })
            .map(|(p, _)| amazon_record(p))
            .take(DISCOVERY_LIMIT)
            .collect()
    }
}

fn product_url(adapter: &dyn Retailer, listing: &DemoListing) -> Option<String> {
    let sku = domain::RetailerSku::new(&listing.sku).ok()?;
    adapter
        .product_url(&sku)
        .ok()
        .map(|u| u.as_str().to_owned())
}

/// A schema.org `Product` for one listing.
fn product_json(
    adapter: &dyn Retailer,
    product: &DemoProduct,
    listing: &DemoListing,
) -> Option<Value> {
    let url = product_url(adapter, listing)?;
    let mut node = json!({
        "@type": "Product",
        "name": listing.title,
        "brand": product.brand,
        "sku": listing.sku,
        "url": url,
        "offers": {
            "@type": "Offer",
            "price": pesos(listing.price_centavos),
            "priceCurrency": "MXN",
            "availability": "https://schema.org/InStock",
            "url": url,
        },
    });
    if let (Some(gtin), Some(obj)) = (&product.gtin, node.as_object_mut()) {
        obj.insert("gtin13".to_owned(), json!(gtin));
    }
    Some(node)
}

fn ld_script(value: &Value) -> String {
    // JSON never contains a raw `<` that could end the script early.
    let text = value.to_string().replace('<', "\\u003c");
    format!(r#"<script type="application/ld+json">{text}</script>"#)
}

fn search_page(products: &[Value]) -> String {
    let items: Vec<Value> = products
        .iter()
        .enumerate()
        .map(|(i, p)| json!({"@type": "ListItem", "position": i + 1, "item": p}))
        .collect();
    let list = json!({
        "@context": "https://schema.org",
        "@type": "ItemList",
        "numberOfItems": items.len(),
        "itemListElement": items,
    });
    format!(
        "<!doctype html><html lang=\"es-MX\"><head><title>Resultados (demo)</title>{}</head><body></body></html>",
        ld_script(&list)
    )
}

fn product_page(adapter: &dyn Retailer, product: &DemoProduct, listing: &DemoListing) -> String {
    let Some(mut node) = product_json(adapter, product, listing) else {
        return search_page(&[]);
    };
    let url = product_url(adapter, listing).unwrap_or_default();
    if let Some(obj) = node.as_object_mut() {
        obj.insert("@context".to_owned(), json!("https://schema.org"));
    }
    format!(
        "<!doctype html><html lang=\"es-MX\"><head><title>{} (demo)</title><link rel=\"canonical\" href=\"{url}\">{}</head><body></body></html>",
        listing.sku,
        ld_script(&node)
    )
}

/// The Amazon.com.mx record Bright Data's Amazon dataset would return.
fn amazon_record(product: &DemoProduct) -> Value {
    let Some(a) = &product.amazon else {
        return Value::Null;
    };
    let mut record = json!({
        "asin": a.asin,
        "title": a.title,
        "brand": a.brand,
        "domain": AMAZON_MX,
        "url": format!("{AMAZON_MX}/dp/{}", a.asin),
        "currency": "MXN",
        "final_price": pesos(a.price_centavos),
        "is_available": true,
        "number_of_sellers": 3,
    });
    if let (Some(gtin), Some(obj)) = (&product.gtin, record.as_object_mut()) {
        obj.insert("upc".to_owned(), json!(gtin));
    }
    record
}

impl DemoTransport {
    fn serve(&self, request: &UnlockRequest) -> Result<UnlockedPage, BrightDataError> {
        Ok(UnlockedPage {
            status: 200,
            body: self.page(&request.url)?,
        })
    }
}

impl Transport for DemoTransport {
    async fn unlock(&self, request: &UnlockRequest) -> Result<UnlockedPage, BrightDataError> {
        self.serve(request)
    }

    async fn browse(&self, request: &UnlockRequest) -> Result<UnlockedPage, BrightDataError> {
        self.serve(request)
    }

    async fn trigger(&self, trigger: &DatasetTrigger) -> Result<SnapshotId, BrightDataError> {
        let records: Vec<Value> = trigger
            .inputs
            .iter()
            .flat_map(|input| self.records(input))
            .collect();
        let n = self.next_snapshot.fetch_add(1, Ordering::Relaxed);
        let id = SnapshotId::parse(&format!("demo_{n}"))?;
        self.snapshots().insert(id.as_str().to_owned(), records);
        Ok(id)
    }

    async fn progress(&self, id: &SnapshotId) -> Result<SnapshotStatus, BrightDataError> {
        Ok(if self.snapshots().contains_key(id.as_str()) {
            SnapshotStatus::Ready
        } else {
            SnapshotStatus::Failed
        })
    }

    async fn download(&self, id: &SnapshotId) -> Result<Download, BrightDataError> {
        self.snapshots()
            .remove(id.as_str())
            .map(Download::Records)
            .ok_or_else(|| BrightDataError::Terminal("demo: unknown snapshot".to_owned()))
    }

    async fn cancel(&self, id: &SnapshotId) -> Result<(), BrightDataError> {
        self.snapshots().remove(id.as_str());
        Ok(())
    }
}

/// Demo fee estimates (the Bright Data fallback has none): the referral fee
/// on the sale price plus the catalog's FBA fee. Catalog and offer lookups
/// are `NotConfigured`, so they fall back to Bright Data.
pub struct DemoFees {
    catalog: DemoCatalog,
    clock: Arc<dyn app::Clock>,
}

impl DemoFees {
    /// Fees for the products of `catalog`.
    #[must_use]
    pub fn new(catalog: DemoCatalog, clock: Arc<dyn app::Clock>) -> Self {
        Self { catalog, clock }
    }

    /// The estimate for `asin` sold at `price`, or `None` for an ASIN the
    /// catalog does not have.
    #[must_use]
    pub fn estimate(&self, asin: &Asin, price: PriceMxn) -> Option<FeeEstimate> {
        let amazon = self
            .catalog
            .products
            .iter()
            .filter_map(|p| p.amazon.as_ref())
            .find(|a| a.asin == asin.as_str())?;
        let referral =
            PriceMxn::from_centavos(price.centavos() * REFERRAL_FEE_PERCENT / 100).ok()?;
        let fba = PriceMxn::from_centavos(amazon.fba_fee_centavos).ok()?;
        let total = PriceMxn::from_centavos(referral.centavos() + fba.centavos()).ok()?;
        Some(FeeEstimate {
            total_mxn: total,
            details: vec![
                FeeDetail {
                    fee_type: "ReferralFee".to_owned(),
                    amount_mxn: referral,
                },
                FeeDetail {
                    fee_type: "FBAFees".to_owned(),
                    amount_mxn: fba,
                },
            ],
            // Stands in for SP-API's fee estimate, the only real fee source.
            source: AmazonDataSource::SpApi,
            fetched_at: self.clock.now(),
        })
    }
}

impl AmazonSource for DemoFees {
    fn catalog_by_gtin<'a>(
        &'a self,
        _gtin: &'a domain::Gtin,
    ) -> PortFuture<'a, Result<CatalogResult, AmazonSourceError>> {
        Box::pin(async { Err(AmazonSourceError::NotConfigured) })
    }

    fn catalog_by_keyword<'a>(
        &'a self,
        _keywords: &'a str,
    ) -> PortFuture<'a, Result<CatalogResult, AmazonSourceError>> {
        Box::pin(async { Err(AmazonSourceError::NotConfigured) })
    }

    fn offers<'a>(
        &'a self,
        _asin: &'a Asin,
    ) -> PortFuture<'a, Result<OfferSummary, AmazonSourceError>> {
        Box::pin(async { Err(AmazonSourceError::NotConfigured) })
    }

    fn fees<'a>(
        &'a self,
        asin: &'a Asin,
        price: PriceMxn,
        _fulfillment: Fulfillment,
    ) -> PortFuture<'a, Result<FeeEstimate, AmazonSourceError>> {
        Box::pin(async move {
            self.estimate(asin, price)
                .ok_or(AmazonSourceError::NotConfigured)
        })
    }
}

#[cfg(test)]
mod tests {
    use db::demo_catalog::Expected;
    use domain::{MatchStatus, UnixMillis};
    use matching::{Candidate, MatchConfig, match_offer};
    use retailers::{Page, ParseError};

    use super::*;
    use crate::offer_matching::keyword_query;

    fn transport() -> DemoTransport {
        DemoTransport::new(DemoCatalog::embedded().unwrap())
    }

    fn adapter(id: &str) -> &'static dyn Retailer {
        retailers::adapter_for(id).unwrap_or_else(|| panic!("unknown retailer {id}"))
    }

    fn fetch(t: &DemoTransport, url: &TargetUrl) -> Page {
        Page {
            url: url.clone(),
            body: t.page(url).unwrap(),
            fetched_at: UnixMillis(0),
        }
    }

    fn candidates(records: &[Value]) -> Vec<Candidate> {
        records
            .iter()
            .map(|r| Candidate {
                asin: Asin::new(r["asin"].as_str().unwrap()).unwrap(),
                title: r["title"].as_str().unwrap().to_owned(),
                brand: r["brand"].as_str().map(str::to_owned),
            })
            .collect()
    }

    #[test]
    fn urls_are_decoded() {
        assert_eq!(decode_url("a%C3%B1o+nuevo%2"), "año nuevo%2");
        assert_eq!(decode_url("x%zz"), "x%zz");
    }

    #[test]
    fn every_listing_is_found_by_its_keywords_and_parses() {
        let t = transport();
        for (product, listing) in t.catalog.listings() {
            let store = adapter(&listing.retailer);
            for keyword in &product.keywords {
                let url = store.search_url(keyword).unwrap();
                let offers = store.parse_search(&fetch(&t, &url)).unwrap();
                let offer = offers
                    .iter()
                    .find(|o| o.retailer_sku.as_str() == listing.sku)
                    .unwrap_or_else(|| panic!("{} not found by {keyword}", listing.sku));
                assert_eq!(offer.price_mxn.centavos(), listing.price_centavos);
                assert_eq!(offer.title, listing.title);
                assert_eq!(
                    offer.gtin.as_ref().map(domain::Gtin::as_str),
                    product.gtin.as_deref()
                );
            }
        }
    }

    #[test]
    fn product_pages_parse_for_refreshes() {
        let t = transport();
        for (_, listing) in t.catalog.listings() {
            let store = adapter(&listing.retailer);
            let sku = domain::RetailerSku::new(&listing.sku).unwrap();
            let url = store.product_url(&sku).unwrap();
            let offer = store.parse_product(&fetch(&t, &url)).unwrap();
            assert_eq!(offer.retailer_sku.as_str(), listing.sku);
            assert_eq!(offer.price_mxn.centavos(), listing.price_centavos);
        }
    }

    #[test]
    fn a_search_without_demo_products_states_no_results() {
        let t = transport();
        let stores: std::collections::BTreeSet<&str> = t
            .catalog
            .listings()
            .map(|(_, l)| l.retailer.as_str())
            .collect();
        for id in stores {
            let store = adapter(id);
            let url = store.search_url("bicicleta").unwrap();
            assert_eq!(
                store.parse_search(&fetch(&t, &url)).unwrap_err(),
                ParseError::NoResults,
                "{id}"
            );
        }
        let foreign = TargetUrl::parse("https://example.com/").unwrap();
        assert!(t.page(&foreign).is_err());
    }

    /// The worker's matching flow over the fake's Amazon records gives each
    /// product its expected outcome.
    #[test]
    fn matches_come_out_as_the_catalog_expects() {
        let t = transport();
        let config = MatchConfig::default();
        for (product, listing) in t.catalog.listings() {
            let store = adapter(&listing.retailer);
            let url = store.search_url(&product.keywords[0]).unwrap();
            let offers = store.parse_search(&fetch(&t, &url)).unwrap();
            let offer = offers
                .iter()
                .find(|o| o.retailer_sku.as_str() == listing.sku)
                .unwrap();
            let gtin_hits = offer
                .gtin
                .as_ref()
                .map(|g| candidates(&t.records(&json!({"keywords": g.as_str()}))))
                .unwrap_or_default();
            let keyword_hits = if gtin_hits.is_empty() {
                let q = keyword_query(offer.brand.as_deref(), &offer.title);
                candidates(&t.records(&json!({"keywords": q})))
            } else {
                Vec::new()
            };
            let found = match_offer(offer, &gtin_hits, &keyword_hits, &config, UnixMillis(0));
            let status = found.as_ref().map(|m| m.status);
            let want = match product.expected {
                Expected::Accepted => Some(MatchStatus::Accepted),
                Expected::NeedsReview => Some(MatchStatus::NeedsReview),
                Expected::NoMatch => None,
            };
            assert_eq!(status, want, "{} at {}", product.key, listing.retailer);
            if let (Some(m), Some(a)) = (&found, &product.amazon) {
                assert_eq!(m.asin.as_str(), a.asin);
            }
        }
    }

    #[test]
    fn profits_are_mixed() {
        let t = transport();
        let fees = DemoFees::new(t.catalog.clone(), Arc::new(crate::clock::WallClock));
        let (mut gain, mut loss) = (0, 0);
        for (product, listing) in t.catalog.listings() {
            let Some(a) = &product.amazon else { continue };
            let sell = PriceMxn::from_centavos(a.price_centavos).unwrap();
            let fee = fees.estimate(&Asin::new(&a.asin).unwrap(), sell).unwrap();
            assert_eq!(
                fee.total_mxn.centavos(),
                a.price_centavos * REFERRAL_FEE_PERCENT / 100 + a.fba_fee_centavos
            );
            if a.price_centavos - listing.price_centavos - fee.total_mxn.centavos() > 0 {
                gain += 1;
            } else {
                loss += 1;
            }
        }
        assert!(gain >= 10 && loss >= 10, "{gain} gains, {loss} losses");
        let unknown = Asin::new("B0NOTDEMO1").unwrap();
        assert!(
            fees.estimate(&unknown, PriceMxn::from_centavos(100).unwrap())
                .is_none()
        );
    }

    #[tokio::test]
    async fn datasets_answer_at_once_and_only_once() {
        let t = transport();
        let trigger = DatasetTrigger {
            dataset_id: brightdata::DatasetId::parse("gd_demo").unwrap(),
            inputs: vec![json!({"url": format!("{AMAZON_MX}/dp/B0DEMO0001")})],
            discover_by: None,
            limit_per_input: None,
        };
        let id = t.trigger(&trigger).await.unwrap();
        assert_eq!(t.progress(&id).await.unwrap(), SnapshotStatus::Ready);
        let Download::Records(records) = t.download(&id).await.unwrap() else {
            panic!("not ready")
        };
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["final_price"], "1499.00");
        assert_eq!(records[0]["upc"], "7509010001012");
        assert_eq!(t.progress(&id).await.unwrap(), SnapshotStatus::Failed);
        let other = t.trigger(&trigger).await.unwrap();
        assert_ne!(id, other);
    }
}
