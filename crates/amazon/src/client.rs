//! The SP-API client: LWA-authorized calls to the North America endpoint
//! for the Mexico marketplace, rate limited per operation, with capped
//! backoff on 429, 5xx and transport failures.

use brightdata::{RetryPolicy, parse_retry_after};
use domain::{Asin, Gtin, PriceMxn};
use http_guard::{
    DEFAULT_MAX_RESPONSE_BYTES, GuardError, GuardedClient, GuardedRequest, HostAllowlist, Redactor,
    SMALL_RESPONSE_BYTES, Secret, StatusCode, TransportPolicy, Url, header,
};
use serde::Deserialize;
use serde::de::DeserializeOwned;

use app::CatalogItem;

use crate::config::{LWA_HOST, LWA_TOKEN_URL, MX_MARKETPLACE_ID, Operation, SpApiConfig};
use crate::error::SpApiError;
use crate::lwa::TokenCache;
use crate::parse::{self, Fees, OfferPrices};
use crate::rate::RateLimiter;

/// Most identifiers one `searchCatalogItems` call accepts.
pub const MAX_IDENTIFIERS: usize = 20;
/// Longest keyword query accepted (characters).
pub const MAX_KEYWORD_CHARS: usize = 200;
/// Catalog data requested on every search.
const INCLUDED_DATA: &str = "summaries,images,salesRanks,identifiers";
/// Response header carrying the applied usage-plan rate.
const RATE_LIMIT_HEADER: &str = "x-amzn-ratelimit-limit";

/// Barcode kind for `identifiersType`, from the GTIN length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentifierType {
    /// 12 digits.
    Upc,
    /// 13 digits.
    Ean,
    /// 8 or 14 digits.
    Gtin,
}

impl IdentifierType {
    /// The kind for `gtin`.
    #[must_use]
    pub fn of(gtin: &Gtin) -> Self {
        match gtin.as_str().len() {
            12 => Self::Upc,
            13 => Self::Ean,
            _ => Self::Gtin,
        }
    }

    /// The query value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Upc => "UPC",
            Self::Ean => "EAN",
            Self::Gtin => "GTIN",
        }
    }
}

#[derive(Deserialize, Default)]
struct ErrorList {
    #[serde(default)]
    errors: Vec<ErrorEntry>,
}

#[derive(Deserialize)]
struct ErrorEntry {
    #[serde(default)]
    code: String,
    #[serde(default)]
    message: String,
}

/// Async SP-API client. Holds credentials (so it has no `Debug`); they and
/// every access token are redacted from errors.
pub struct SpApiClient {
    http: GuardedClient,
    base: Url,
    tokens: TokenCache,
    limiter: RateLimiter,
    retry: RetryPolicy,
    user_agent: String,
}

impl SpApiClient {
    /// The production (or sandbox, per config) client.
    ///
    /// # Errors
    ///
    /// [`SpApiError::Build`] if an HTTP client cannot be created.
    pub fn new(config: SpApiConfig) -> Result<Self, SpApiError> {
        let host = config.endpoint.host();
        let base = Url::parse(&format!("https://{host}/"))
            .map_err(|_| SpApiError::Build("invalid SP-API base URL".to_owned()))?;
        let token_url = Url::parse(LWA_TOKEN_URL)
            .map_err(|_| SpApiError::Build("invalid LWA token URL".to_owned()))?;
        Self::build(config, base, token_url, host, TransportPolicy::PUBLIC_HTTPS)
    }

    /// A client whose SP-API and LWA endpoints are loopback fakes.
    #[cfg(test)]
    pub(crate) fn for_test(base: &str, token_url: &str, config: SpApiConfig) -> Self {
        let base = Url::parse(base).expect("test base URL");
        let token_url = Url::parse(token_url).expect("test token URL");
        let host = config.endpoint.host();
        Self::build(
            config,
            base,
            token_url,
            host,
            TransportPolicy::LOOPBACK_TEST,
        )
        .expect("test client")
    }

    fn build(
        config: SpApiConfig,
        base: Url,
        token_url: Url,
        host: &str,
        policy: TransportPolicy,
    ) -> Result<Self, SpApiError> {
        let redactor = Redactor::new([
            &config.client_id,
            &config.client_secret,
            &config.refresh_token,
        ]);
        let build = |allowed: &str, timeout| {
            let allowlist = HostAllowlist::new([allowed])?;
            GuardedClient::builder(allowlist)
                .policy(policy)
                .total_timeout(timeout)
                .redactor(redactor.clone())
                .build()
        };
        let build_error = |error: GuardError| SpApiError::Build(error.to_string());
        let lwa = build(LWA_HOST, config.timeout).map_err(build_error)?;
        let http = build(host, config.timeout).map_err(build_error)?;
        Ok(Self {
            http,
            base,
            tokens: TokenCache::new(
                lwa,
                token_url,
                config.client_id,
                config.client_secret,
                config.refresh_token,
                config.retry,
            ),
            limiter: RateLimiter::new(config.rates),
            retry: config.retry,
            user_agent: format!("mex-scrape/{} (Language=Rust)", env!("CARGO_PKG_VERSION")),
        })
    }

    fn endpoint(&self, path: &str, query: &[(&str, &str)]) -> Url {
        let mut url = self.base.clone();
        url.set_path(path);
        if !query.is_empty() {
            url.query_pairs_mut().extend_pairs(query);
        }
        url
    }

    /// Catalog items carrying any of `gtins` (1 to [`MAX_IDENTIFIERS`], all
    /// of one [`IdentifierType`]).
    ///
    /// # Errors
    ///
    /// [`SpApiError::Invalid`] for an empty, too long or mixed list; else as
    /// the request.
    pub async fn search_catalog_by_identifiers(
        &self,
        gtins: &[Gtin],
    ) -> Result<Vec<CatalogItem>, SpApiError> {
        let Some(first) = gtins.first() else {
            return Err(SpApiError::Invalid("no identifiers".to_owned()));
        };
        let kind = IdentifierType::of(first);
        if gtins.len() > MAX_IDENTIFIERS || gtins.iter().any(|g| IdentifierType::of(g) != kind) {
            return Err(SpApiError::Invalid(format!(
                "1 to {MAX_IDENTIFIERS} identifiers of one type are allowed"
            )));
        }
        let joined = gtins.iter().map(Gtin::as_str).collect::<Vec<_>>().join(",");
        let page_size = gtins.len().to_string();
        let url = self.endpoint(
            "/catalog/2022-04-01/items",
            &[
                ("marketplaceIds", MX_MARKETPLACE_ID),
                ("identifiers", &joined),
                ("identifiersType", kind.as_str()),
                ("includedData", INCLUDED_DATA),
                ("locale", "es_MX"),
                ("pageSize", &page_size),
            ],
        );
        let search: Option<parse::CatalogSearch> = self
            .send_json(Operation::SearchCatalog, |token| {
                self.authorized(self.http.get(&url)?, token)
            })
            .await?;
        search.map_or_else(|| Ok(Vec::new()), parse::catalog_items)
    }

    /// Catalog items matching `keywords` (first page, up to 10).
    ///
    /// # Errors
    ///
    /// [`SpApiError::Invalid`] for blank, too long or control-character
    /// keywords; else as the request.
    pub async fn search_catalog_by_keyword(
        &self,
        keywords: &str,
    ) -> Result<Vec<CatalogItem>, SpApiError> {
        let keywords = valid_keywords(keywords)?;
        let url = self.endpoint(
            "/catalog/2022-04-01/items",
            &[
                ("marketplaceIds", MX_MARKETPLACE_ID),
                ("keywords", &keywords),
                ("includedData", INCLUDED_DATA),
                ("locale", "es_MX"),
                ("pageSize", "10"),
            ],
        );
        let search: Option<parse::CatalogSearch> = self
            .send_json(Operation::SearchCatalog, |token| {
                self.authorized(self.http.get(&url)?, token)
            })
            .await?;
        search.map_or_else(|| Ok(Vec::new()), parse::catalog_items)
    }

    /// New-condition offer prices for `asin` (empty when Amazon has none).
    ///
    /// # Errors
    ///
    /// As the request; [`SpApiError::Parse`] for non-MXN prices.
    pub(crate) async fn item_offers(&self, asin: &Asin) -> Result<OfferPrices, SpApiError> {
        let url = self.endpoint(
            &format!("/products/pricing/v0/items/{}/offers", asin.as_str()),
            &[
                ("MarketplaceId", MX_MARKETPLACE_ID),
                ("ItemCondition", "New"),
            ],
        );
        let response: Option<parse::ItemOffersResponse> = self
            .send_json(Operation::ItemOffers, |token| {
                self.authorized(self.http.get(&url)?, token)
            })
            .await?;
        response.map_or_else(|| Ok(OfferPrices::none()), |r| parse::item_offers(r, asin))
    }

    /// Estimated fees for selling `asin` at `price` (always MXN).
    ///
    /// # Errors
    ///
    /// As the request; [`SpApiError::Invalid`] when Amazon cannot estimate
    /// (e.g. unknown ASIN); [`SpApiError::Parse`] for non-MXN amounts.
    pub(crate) async fn fees_estimate(
        &self,
        asin: &Asin,
        price: PriceMxn,
        fba: bool,
    ) -> Result<Fees, SpApiError> {
        let url = self.endpoint(
            &format!("/products/fees/v0/items/{}/feesEstimate", asin.as_str()),
            &[],
        );
        let body = parse::fees_request_body(asin, price, fba);
        let response: Option<parse::FeesResponse> = self
            .send_json(Operation::FeesEstimate, |token| {
                self.authorized(self.http.post(&url)?, token)?.json(&body)
            })
            .await?;
        let response =
            response.ok_or_else(|| SpApiError::Invalid("ASIN not found for fees".to_owned()))?;
        parse::fees(response)
    }

    fn authorized(
        &self,
        request: GuardedRequest,
        token: &Secret,
    ) -> Result<GuardedRequest, GuardError> {
        request
            .secret_header("x-amz-access-token", token)?
            .header("user-agent", &self.user_agent)?
            .header("accept", "application/json")
    }

    /// Sends a request built by `make` (every call here is safe to repeat:
    /// GETs and the fees estimate). Returns `None` on 404.
    async fn send_json<T: DeserializeOwned>(
        &self,
        op: Operation,
        make: impl Fn(&Secret) -> Result<GuardedRequest, GuardError>,
    ) -> Result<Option<T>, SpApiError> {
        let mut attempt = 0_u32;
        let mut refreshed = false;
        loop {
            self.limiter.acquire(op).await;
            let token = self.tokens.token().await?;
            let request = make(&token).map_err(|e| SpApiError::Invalid(e.to_string()))?;
            let (error, retry_after) = match request.send().await {
                Err(error) if error.is_transient() => {
                    (SpApiError::Transient(error.to_string()), None)
                }
                Err(error) => return Err(SpApiError::Invalid(error.to_string())),
                Ok(response) => {
                    if let Some(limit) = response
                        .headers()
                        .get(RATE_LIMIT_HEADER)
                        .and_then(|v| v.to_str().ok())
                    {
                        self.limiter.observe_limit_header(op, limit).await;
                    }
                    let status = response.status();
                    if status.is_success() {
                        let value = response
                            .json::<T>(DEFAULT_MAX_RESPONSE_BYTES)
                            .await
                            .map_err(|e| match e {
                                GuardError::InvalidJson(m) => SpApiError::Parse(m),
                                other if other.is_transient() => {
                                    SpApiError::Transient(other.to_string())
                                }
                                other => SpApiError::Parse(other.to_string()),
                            })?;
                        return Ok(Some(value));
                    }
                    if status == StatusCode::NOT_FOUND {
                        return Ok(None);
                    }
                    let retry_after = response
                        .headers()
                        .get(header::RETRY_AFTER)
                        .and_then(|v| v.to_str().ok())
                        .and_then(parse_retry_after);
                    let detail = error_detail(status, response).await;
                    match status.as_u16() {
                        401 if !refreshed => {
                            // The token may have been revoked early: refresh once.
                            refreshed = true;
                            self.tokens.invalidate(&token).await;
                            continue;
                        }
                        401 | 403 => return Err(SpApiError::NotAuthorized(detail)),
                        429 => (SpApiError::RateLimited(detail), retry_after),
                        408 | 500..=599 => (SpApiError::Transient(detail), retry_after),
                        _ => return Err(SpApiError::Invalid(detail)),
                    }
                }
            };
            if attempt >= self.retry.max_retries {
                return Err(error);
            }
            tokio::time::sleep(self.retry.delay(attempt, retry_after)).await;
            attempt += 1;
        }
    }
}

/// `"<status> <code>: <message>"` from an SP-API error body, redacted.
async fn error_detail(status: StatusCode, response: http_guard::GuardedResponse) -> String {
    let text = response
        .error_text(SMALL_RESPONSE_BYTES)
        .await
        .unwrap_or_default();
    let list: ErrorList = serde_json::from_str(&text).unwrap_or_default();
    match list.errors.first() {
        Some(entry) => format!("{} {}: {}", status.as_u16(), entry.code, entry.message),
        None => format!("{} {}", status.as_u16(), text),
    }
}

/// Trimmed keywords, or [`SpApiError::Invalid`].
pub(crate) fn valid_keywords(keywords: &str) -> Result<String, SpApiError> {
    let trimmed = keywords.split_whitespace().collect::<Vec<_>>().join(" ");
    if trimmed.is_empty()
        || trimmed.chars().count() > MAX_KEYWORD_CHARS
        || trimmed.chars().any(char::is_control)
    {
        return Err(SpApiError::Invalid(format!(
            "keywords must be 1 to {MAX_KEYWORD_CHARS} printable characters"
        )));
    }
    Ok(trimmed)
}

#[cfg(test)]
mod tests;
