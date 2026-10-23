//! The fixture-backed Bright Data connector: store pages and Amazon
//! dataset records from the demo catalog.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use brightdata::{
    BrightDataError, DatasetTrigger, Download, SnapshotId, SnapshotStatus, TargetUrl, Transport,
    UnlockRequest, UnlockedPage,
};
use db::demo::catalog::{DemoCatalog, DemoListing, DemoProduct, ListingStock};
use retailers::Retailer;
use serde_json::{Value, json};

/// Amazon Mexico origin used in the dataset records.
pub(super) const AMAZON_MX: &str = "https://www.amazon.com.mx";
/// Records per discovery input, as the real fallback asks for.
const DISCOVERY_LIMIT: usize = 5;
/// Shared title words a keyword record needs (the matcher scores the rest).
const MIN_SHARED_WORDS: usize = 2;

/// `1499.00` from 149 900 centavos.
fn pesos(centavos: i64) -> String {
    format!("{}.{:02}", centavos / 100, centavos % 100)
}

/// Decodes `%XX` escapes and `+` in a URL, so query words can be compared.
pub(super) fn decode_url(url: &str) -> String {
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
    pub(super) catalog: DemoCatalog,
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
            "availability": match listing.availability {
                ListingStock::InStock => "https://schema.org/InStock",
                ListingStock::OutOfStock => "https://schema.org/OutOfStock",
            },
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
