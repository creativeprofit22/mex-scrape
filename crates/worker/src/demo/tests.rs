//! Demo mode runs every catalog listing through the real adapters,
//! matcher and ROI maths.

use std::collections::BTreeSet;
use std::sync::Arc;

use brightdata::{DatasetTrigger, Download, SnapshotStatus, TargetUrl, Transport as _};
use db::demo::account::DEMO_MEMBERSHIPS;
use db::demo::catalog::{DemoCatalog, Expected, ListingStock};
use domain::{
    Asin, Availability, MatchStatus, Offer, PriceMxn, ProductMatch, RoiInput, UnixMillis,
    calculate_roi,
};
use matching::{Candidate, MatchConfig, match_offer};
use retailers::{Page, ParseError, Retailer};
use serde_json::{Value, json};

use super::transport::{AMAZON_MX, decode_url};
use super::{DemoFees, DemoTransport, REFERRAL_FEE_PERCENT};
use crate::config::DEMO_SEARCH_MAX_OFFERS;
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

/// The worker's matching flow: GTIN lookup first, else a keyword query.
fn match_offer_like_the_worker(t: &DemoTransport, offer: &Offer) -> Option<ProductMatch> {
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
    match_offer(
        offer,
        &gtin_hits,
        &keyword_hits,
        &MatchConfig::default(),
        UnixMillis(0),
    )
}

/// The offers a search job for `query` would match and price: every
/// catalog store's search page, deduped by SKU and capped per store.
fn search_offers(t: &DemoTransport, query: &str) -> Vec<Offer> {
    let stores: BTreeSet<&str> = t
        .catalog
        .listings()
        .map(|(_, l)| l.retailer.as_str())
        .collect();
    let mut out = Vec::new();
    for id in stores {
        let store = adapter(id);
        let url = store.search_url(query).unwrap();
        let offers = match store.parse_search(&fetch(t, &url)) {
            Ok(offers) => offers,
            Err(ParseError::NoResults) => continue,
            Err(e) => panic!("{id} {query}: {e:?}"),
        };
        let mut seen = BTreeSet::new();
        out.extend(
            offers
                .into_iter()
                .filter(|o| seen.insert(o.retailer_sku.as_str().to_owned()))
                .take(DEMO_SEARCH_MAX_OFFERS),
        );
    }
    out
}

/// Profit per unit with zero prep and shipping, as a search prices it.
fn profit(t: &DemoTransport, fees: &DemoFees, offer: &Offer, m: &ProductMatch) -> i64 {
    let amazon = t
        .catalog
        .products
        .iter()
        .filter_map(|p| p.amazon.as_ref())
        .find(|a| a.asin == m.asin.as_str())
        .unwrap();
    let sell = PriceMxn::from_centavos(amazon.price_centavos).unwrap();
    let zero = PriceMxn::from_centavos(0).unwrap();
    calculate_roi(&RoiInput {
        sell_price: sell,
        buy_price: offer.price_mxn,
        amazon_fees: fees.estimate(&m.asin, sell).unwrap().total_mxn,
        prep_fee: zero,
        shipping: zero,
        monthly_fixed: zero,
        units_per_month: 1,
    })
    .unwrap()
    .profit_centavos
}

/// Each keyword group `seed_demo` queues shows, to the demo account, a
/// profitable and a losing accepted match plus a needs-review or no-match
/// result, and its search covers every listing in the group.
#[test]
fn every_group_has_profit_loss_and_review() {
    let t = transport();
    let fees = DemoFees::new(t.catalog.clone(), Arc::new(crate::clock::WallClock));
    for group in &t.catalog.groups {
        let offers = search_offers(&t, group);
        let listed = t
            .catalog
            .group_products(group)
            .map(|p| p.listings.len())
            .sum::<usize>();
        assert_eq!(
            offers.len(),
            listed,
            "{group}: the per-store cap cut listings"
        );
        let (mut gain, mut loss, mut review) = (0, 0, 0);
        for offer in offers
            .iter()
            .filter(|o| DEMO_MEMBERSHIPS.allows(o.membership_required, &o.retailer))
        {
            match match_offer_like_the_worker(&t, offer) {
                Some(m) if m.status == MatchStatus::Accepted => {
                    let p = profit(&t, &fees, offer, &m);
                    gain += usize::from(p > 0);
                    loss += usize::from(p < 0);
                }
                Some(m) if m.status == MatchStatus::NeedsReview => review += 1,
                Some(_) => {}
                None => review += 1,
            }
        }
        assert!(
            gain >= 1 && loss >= 1 && review >= 1,
            "{group}: {gain} profitable, {loss} losing, {review} review/no match"
        );
    }
}

/// Sold-out listings reach search results and refreshes as out of stock.
#[test]
fn out_of_stock_listings_reach_results() {
    let t = transport();
    let sold_out: BTreeSet<(String, String)> = t
        .catalog
        .listings()
        .filter(|(_, l)| l.availability == ListingStock::OutOfStock)
        .map(|(_, l)| (l.retailer.clone(), l.sku.clone()))
        .collect();
    assert!(sold_out.len() >= 3, "{sold_out:?}");
    let found: BTreeSet<(String, String)> = t
        .catalog
        .groups
        .iter()
        .flat_map(|g| search_offers(&t, g))
        .filter(|o| o.availability == Availability::OutOfStock)
        .map(|o| {
            (
                o.retailer.as_str().to_owned(),
                o.retailer_sku.as_str().to_owned(),
            )
        })
        .collect();
    assert_eq!(found, sold_out);
    for (retailer, sku) in &sold_out {
        let store = adapter(retailer);
        let url = store
            .product_url(&domain::RetailerSku::new(sku).unwrap())
            .unwrap();
        let offer = store.parse_product(&fetch(&t, &url)).unwrap();
        assert_eq!(
            offer.availability,
            Availability::OutOfStock,
            "{retailer} {sku}"
        );
    }
}

/// The demo account lacks Sam's Club, so default searches hide some results.
#[test]
fn memberships_hide_sams_results() {
    let t = transport();
    let hidden: Vec<String> = t
        .catalog
        .groups
        .iter()
        .filter(|g| {
            search_offers(&t, g)
                .iter()
                .any(|o| !DEMO_MEMBERSHIPS.allows(o.membership_required, &o.retailer))
        })
        .cloned()
        .collect();
    assert!(hidden.len() >= 3, "groups with hidden results: {hidden:?}");
    assert!(hidden.iter().any(|g| g == "electronica"));
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
    for (product, listing) in t.catalog.listings() {
        let store = adapter(&listing.retailer);
        let url = store.search_url(&product.keywords[0]).unwrap();
        let offers = store.parse_search(&fetch(&t, &url)).unwrap();
        let offer = offers
            .iter()
            .find(|o| o.retailer_sku.as_str() == listing.sku)
            .unwrap();
        let found = match_offer_like_the_worker(&t, offer);
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
