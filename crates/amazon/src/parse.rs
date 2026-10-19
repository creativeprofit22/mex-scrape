//! SP-API response models and pure mapping into `app` types. Every money
//! value must be in MXN; anything else is a parse failure (we never convert).

use app::{CatalogItem, FeeDetail, OfferSummary, SalesRank};
use domain::{Asin, PriceMxn};
use serde::Deserialize;
use serde_json::Number;

use crate::config::MX_MARKETPLACE_ID;
use crate::error::SpApiError;

/// The only currency accepted.
pub const MXN: &str = "MXN";

fn parse_err(message: &str) -> SpApiError {
    SpApiError::Parse(message.to_owned())
}

// ---- Catalog Items 2022-04-01 ----------------------------------------

#[derive(Debug, Deserialize)]
pub(crate) struct CatalogSearch {
    #[serde(default)]
    items: Vec<RawItem>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawItem {
    asin: String,
    #[serde(default)]
    summaries: Vec<RawSummary>,
    #[serde(default)]
    images: Vec<RawImageSet>,
    #[serde(default)]
    sales_ranks: Vec<RawSalesRanks>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawSummary {
    marketplace_id: String,
    #[serde(default)]
    item_name: Option<String>,
    #[serde(default)]
    brand: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawImageSet {
    marketplace_id: String,
    #[serde(default)]
    images: Vec<RawImage>,
}

#[derive(Debug, Deserialize)]
struct RawImage {
    #[serde(default)]
    variant: String,
    link: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawSalesRanks {
    marketplace_id: String,
    #[serde(default)]
    classification_ranks: Vec<RawRank>,
    #[serde(default)]
    display_group_ranks: Vec<RawRank>,
}

#[derive(Debug, Deserialize)]
struct RawRank {
    title: String,
    rank: u32,
}

fn clean(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Maps a `searchCatalogItems` response. Items without an MX title are
/// skipped; an invalid ASIN fails the whole response.
pub(crate) fn catalog_items(search: CatalogSearch) -> Result<Vec<CatalogItem>, SpApiError> {
    let mut out = Vec::with_capacity(search.items.len());
    for item in search.items {
        let asin =
            Asin::new(&item.asin).map_err(|_| parse_err("catalog item has an invalid ASIN"))?;
        let Some(summary) = item
            .summaries
            .iter()
            .find(|s| s.marketplace_id == MX_MARKETPLACE_ID)
        else {
            continue;
        };
        let title = clean(summary.item_name.as_deref().unwrap_or_default());
        if title.is_empty() {
            continue;
        }
        let brand = summary
            .brand
            .as_deref()
            .map(clean)
            .filter(|b| !b.is_empty());
        let mut images: Vec<&RawImage> = item
            .images
            .iter()
            .filter(|set| set.marketplace_id == MX_MARKETPLACE_ID)
            .flat_map(|set| set.images.iter())
            .filter(|image| image.link.starts_with("https://"))
            .collect();
        // MAIN first; otherwise keep Amazon's order.
        images.sort_by_key(|image| image.variant != "MAIN");
        let mut links: Vec<String> = Vec::new();
        for image in images {
            if !links.contains(&image.link) {
                links.push(image.link.clone());
            }
        }
        let mut sales_ranks: Vec<SalesRank> = item
            .sales_ranks
            .iter()
            .filter(|set| set.marketplace_id == MX_MARKETPLACE_ID)
            .flat_map(|set| {
                set.display_group_ranks
                    .iter()
                    .chain(set.classification_ranks.iter())
            })
            .map(|rank| SalesRank {
                category: clean(&rank.title),
                rank: rank.rank,
            })
            .collect();
        sales_ranks.sort_by(|a, b| {
            a.rank
                .cmp(&b.rank)
                .then_with(|| a.category.cmp(&b.category))
        });
        out.push(CatalogItem {
            asin,
            title,
            brand,
            images: links,
            sales_ranks,
        });
    }
    Ok(out)
}

// ---- Money --------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub(crate) struct Money {
    #[serde(rename = "CurrencyCode")]
    currency_code: String,
    #[serde(rename = "Amount")]
    amount: Number,
}

/// An MXN amount; any other currency or a malformed amount fails.
pub(crate) fn mxn(money: &Money) -> Result<PriceMxn, SpApiError> {
    if money.currency_code != MXN {
        return Err(parse_err("amount is not in MXN"));
    }
    PriceMxn::from_decimal_str(&money.amount.to_string())
        .map_err(|_| parse_err("amount is not a valid MXN price"))
}

/// The JSON number for `price` (e.g. `1299.5`), for request bodies.
pub(crate) fn amount_number(price: PriceMxn) -> Number {
    let centavos = price.centavos();
    let text = format!("{}.{:02}", centavos / 100, centavos % 100);
    text.parse()
        .unwrap_or_else(|_| Number::from(centavos / 100))
}

// ---- Product Pricing v0 getItemOffers ---------------------------------

#[derive(Debug, Deserialize)]
pub(crate) struct ItemOffersResponse {
    payload: Option<ItemOffersPayload>,
}

#[derive(Debug, Deserialize)]
struct ItemOffersPayload {
    #[serde(rename = "ASIN", default)]
    asin: Option<String>,
    #[serde(default)]
    status: String,
    #[serde(rename = "Summary")]
    summary: Option<OffersSummary>,
}

#[derive(Debug, Deserialize)]
struct OffersSummary {
    #[serde(rename = "TotalOfferCount", default)]
    total_offer_count: u32,
    #[serde(rename = "NumberOfOffers", default)]
    number_of_offers: Vec<OfferCount>,
    #[serde(rename = "LowestPrices", default)]
    lowest_prices: Vec<PricePoint>,
    #[serde(rename = "BuyBoxPrices", default)]
    buy_box_prices: Vec<PricePoint>,
}

#[derive(Debug, Deserialize)]
struct OfferCount {
    #[serde(default)]
    condition: String,
    #[serde(rename = "OfferCount", default)]
    offer_count: u32,
}

#[derive(Debug, Deserialize)]
struct PricePoint {
    #[serde(default)]
    condition: String,
    #[serde(rename = "LandedPrice")]
    landed_price: Option<Money>,
    #[serde(rename = "ListingPrice")]
    listing_price: Option<Money>,
}

impl PricePoint {
    fn is_new(&self) -> bool {
        self.condition.eq_ignore_ascii_case("new")
    }

    fn price(&self) -> Result<Option<PriceMxn>, SpApiError> {
        self.landed_price
            .as_ref()
            .or(self.listing_price.as_ref())
            .map(mxn)
            .transpose()
    }
}

/// Prices of an offer summary, without source and time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OfferPrices {
    pub(crate) buy_box_mxn: Option<PriceMxn>,
    pub(crate) lowest_new_mxn: Option<PriceMxn>,
    pub(crate) offer_count: u32,
}

impl OfferPrices {
    pub(crate) fn none() -> Self {
        Self {
            buy_box_mxn: None,
            lowest_new_mxn: None,
            offer_count: 0,
        }
    }

    pub(crate) fn into_summary(
        self,
        source: app::AmazonDataSource,
        fetched_at: domain::UnixMillis,
    ) -> OfferSummary {
        OfferSummary {
            buy_box_mxn: self.buy_box_mxn,
            lowest_new_mxn: self.lowest_new_mxn,
            offer_count: self.offer_count,
            source,
            fetched_at,
        }
    }
}

/// Maps a `getItemOffers` response for `asin` (new condition). Every
/// price, used or not, must be MXN.
pub(crate) fn item_offers(
    response: ItemOffersResponse,
    asin: &Asin,
) -> Result<OfferPrices, SpApiError> {
    let payload = response
        .payload
        .ok_or_else(|| parse_err("offers response has no payload"))?;
    if payload
        .asin
        .as_deref()
        .is_some_and(|got| !got.eq_ignore_ascii_case(asin.as_str()))
    {
        return Err(parse_err("offers response is for another ASIN"));
    }
    match payload.status.as_str() {
        "Success" | "NoBuyableOffers" => {}
        _ => {
            return Err(SpApiError::Invalid(
                "offers status was not Success".to_owned(),
            ));
        }
    }
    let Some(summary) = payload.summary else {
        return Ok(OfferPrices::none());
    };
    let mut buy_box_mxn = None;
    for point in &summary.buy_box_prices {
        let price = point.price()?;
        if point.is_new() && buy_box_mxn.is_none() {
            buy_box_mxn = price;
        }
    }
    let mut lowest_new_mxn: Option<PriceMxn> = None;
    for point in &summary.lowest_prices {
        let price = point.price()?;
        if point.is_new()
            && let Some(price) = price
        {
            lowest_new_mxn = Some(lowest_new_mxn.map_or(price, |low| low.min(price)));
        }
    }
    let new_counts: Vec<u32> = summary
        .number_of_offers
        .iter()
        .filter(|count| count.condition.eq_ignore_ascii_case("new"))
        .map(|count| count.offer_count)
        .collect();
    let offer_count = if new_counts.is_empty() {
        summary.total_offer_count
    } else {
        new_counts
            .iter()
            .fold(0_u32, |sum, n| sum.saturating_add(*n))
    };
    Ok(OfferPrices {
        buy_box_mxn,
        lowest_new_mxn,
        offer_count,
    })
}

// ---- Product Fees v0 getMyFeesEstimateForASIN -------------------------

#[derive(Debug, Deserialize)]
pub(crate) struct FeesResponse {
    payload: Option<FeesPayload>,
}

#[derive(Debug, Deserialize)]
struct FeesPayload {
    #[serde(rename = "FeesEstimateResult")]
    result: Option<FeesResult>,
}

#[derive(Debug, Deserialize)]
struct FeesResult {
    #[serde(rename = "Status", default)]
    status: String,
    #[serde(rename = "FeesEstimate")]
    estimate: Option<FeesEstimateBody>,
}

#[derive(Debug, Deserialize)]
struct FeesEstimateBody {
    #[serde(rename = "TotalFeesEstimate")]
    total: Option<Money>,
    #[serde(rename = "FeeDetailList", default)]
    details: Vec<RawFeeDetail>,
}

#[derive(Debug, Deserialize)]
struct RawFeeDetail {
    #[serde(rename = "FeeType")]
    fee_type: String,
    #[serde(rename = "FeeAmount")]
    fee_amount: Option<Money>,
    #[serde(rename = "FinalFee")]
    final_fee: Option<Money>,
}

/// A fee estimate without source and time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Fees {
    pub(crate) total_mxn: PriceMxn,
    pub(crate) details: Vec<FeeDetail>,
}

/// Maps a `getMyFeesEstimateForASIN` response. Every amount must be MXN.
pub(crate) fn fees(response: FeesResponse) -> Result<Fees, SpApiError> {
    let result = response
        .payload
        .and_then(|payload| payload.result)
        .ok_or_else(|| parse_err("fees response has no result"))?;
    if result.status != "Success" {
        return Err(SpApiError::Invalid(
            "fees estimate status was not Success".to_owned(),
        ));
    }
    let estimate = result
        .estimate
        .ok_or_else(|| parse_err("fees response has no estimate"))?;
    let total_mxn = mxn(estimate
        .total
        .as_ref()
        .ok_or_else(|| parse_err("fees response has no total"))?)?;
    let mut details = Vec::with_capacity(estimate.details.len());
    for detail in &estimate.details {
        if let Some(amount) = &detail.fee_amount {
            mxn(amount)?;
        }
        let money = detail
            .final_fee
            .as_ref()
            .or(detail.fee_amount.as_ref())
            .ok_or_else(|| parse_err("fee detail has no amount"))?;
        details.push(FeeDetail {
            fee_type: clean(&detail.fee_type),
            amount_mxn: mxn(money)?,
        });
    }
    Ok(Fees { total_mxn, details })
}

/// The `getMyFeesEstimateForASIN` request body. Currency is always MXN
/// (Amazon's client libraries default to USD).
pub(crate) fn fees_request_body(asin: &Asin, price: PriceMxn, fba: bool) -> serde_json::Value {
    serde_json::json!({
        "FeesEstimateRequest": {
            "MarketplaceId": MX_MARKETPLACE_ID,
            "IsAmazonFulfilled": fba,
            "PriceToEstimateFees": {
                "ListingPrice": {
                    "CurrencyCode": MXN,
                    "Amount": amount_number(price),
                },
            },
            "Identifier": format!("mex-scrape-{}", asin.as_str()),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture<T: serde::de::DeserializeOwned>(text: &str) -> T {
        serde_json::from_str(text).unwrap()
    }

    fn asin() -> Asin {
        Asin::new("B0SYNTH001").unwrap()
    }

    fn p(centavos: i64) -> PriceMxn {
        PriceMxn::from_centavos(centavos).unwrap()
    }

    #[test]
    fn catalog_maps_title_brand_images_and_ranks() {
        let items = catalog_items(fixture(include_str!(
            "../tests/fixtures/catalog_by_upc.json"
        )))
        .unwrap();
        assert_eq!(items.len(), 1);
        let item = &items[0];
        assert_eq!(item.asin.as_str(), "B0SYNTH001");
        assert_eq!(item.title, "Taladro Inalámbrico 20V Sintético");
        assert_eq!(item.brand.as_deref(), Some("MarcaPrueba"));
        assert_eq!(
            item.images,
            [
                "https://m.media-amazon.com/images/I/synthetic-main.jpg",
                "https://m.media-amazon.com/images/I/synthetic-pt01.jpg",
            ]
        );
        let ranks: Vec<(u32, &str)> = item
            .sales_ranks
            .iter()
            .map(|r| (r.rank, r.category.as_str()))
            .collect();
        assert_eq!(
            ranks,
            [(87, "Taladros"), (1520, "Herramientas y Mejoras del Hogar")]
        );
    }

    #[test]
    fn catalog_keyword_fixture_skips_items_without_an_mx_title() {
        let items = catalog_items(fixture(include_str!(
            "../tests/fixtures/catalog_by_keyword.json"
        )))
        .unwrap();
        let asins: Vec<&str> = items.iter().map(|i| i.asin.as_str()).collect();
        assert_eq!(asins, ["B0SYNTH001", "B0SYNTH002"]);
        assert!(items[1].brand.is_none());
        assert!(items[1].images.is_empty());
    }

    #[test]
    fn offers_fixture_reads_new_buy_box_lowest_and_count() {
        let prices = item_offers(
            fixture(include_str!("../tests/fixtures/item_offers.json")),
            &asin(),
        )
        .unwrap();
        assert_eq!(
            prices,
            OfferPrices {
                buy_box_mxn: Some(p(129_900)),
                lowest_new_mxn: Some(p(124_950)),
                offer_count: 3,
            }
        );
    }

    #[test]
    fn usd_anywhere_is_rejected() {
        let offers = include_str!("../tests/fixtures/item_offers.json").replace("MXN", "USD");
        assert!(matches!(
            item_offers(fixture(&offers), &asin()),
            Err(SpApiError::Parse(_))
        ));
        let fees_usd =
            include_str!("../tests/fixtures/fees_estimate.json").replacen("MXN", "USD", 1);
        assert!(matches!(
            fees(fixture(&fees_usd)),
            Err(SpApiError::Parse(_))
        ));
    }

    #[test]
    fn offers_for_another_asin_fail() {
        let offers = include_str!("../tests/fixtures/item_offers.json");
        let other = Asin::new("B0OTHER000").unwrap();
        assert!(matches!(
            item_offers(fixture(offers), &other),
            Err(SpApiError::Parse(_))
        ));
    }

    #[test]
    fn fees_fixture_reads_total_and_details() {
        let got = fees(fixture(include_str!(
            "../tests/fixtures/fees_estimate.json"
        )))
        .unwrap();
        assert_eq!(got.total_mxn, p(25_485));
        let details: Vec<(&str, i64)> = got
            .details
            .iter()
            .map(|d| (d.fee_type.as_str(), d.amount_mxn.centavos()))
            .collect();
        assert_eq!(
            details,
            [
                ("ReferralFee", 19_485),
                ("FBAFees", 6_000),
                ("VariableClosingFee", 0)
            ]
        );
    }

    #[test]
    fn fees_body_is_always_mxn() {
        let body = fees_request_body(&asin(), p(129_950), true);
        let request = &body["FeesEstimateRequest"];
        assert_eq!(request["MarketplaceId"], MX_MARKETPLACE_ID);
        assert_eq!(request["IsAmazonFulfilled"], true);
        let listing = &request["PriceToEstimateFees"]["ListingPrice"];
        assert_eq!(listing["CurrencyCode"], "MXN");
        assert_eq!(listing["Amount"].to_string(), "1299.5");
        assert_eq!(amount_number(p(5)).to_string(), "0.05");
        assert_eq!(amount_number(p(100_000)).to_string(), "1000.0");
    }
}
