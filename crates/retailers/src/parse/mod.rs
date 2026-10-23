//! Shared parsing helpers. Adapters try, in order: JSON-LD ([`jsonld`]),
//! page JSON ([`embedded`]), then CSS selectors ([`css`]).

pub mod css;
pub mod embedded;
pub mod jsonld;
pub mod price;
pub mod vtex;

use domain::{Availability, Gtin, Offer, PriceMxn, RetailerId, RetailerSku};

use crate::adapter::{Page, ParseError};
use crate::registry::RetailerInfo;

/// Offer fields gathered from any source before validation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Draft {
    /// Product title.
    pub title: Option<String>,
    /// Brand name.
    pub brand: Option<String>,
    /// Selling price.
    pub price: Option<PriceMxn>,
    /// Strike-through list price.
    pub list_price: Option<PriceMxn>,
    /// Stock state.
    pub availability: Option<Availability>,
    /// Raw GTIN text (validated in [`Draft::finish`]).
    pub gtin: Option<String>,
    /// Retailer SKU text.
    pub sku: Option<String>,
    /// Product URL, absolute or relative to the page.
    pub url: Option<String>,
}

impl Draft {
    /// Fills every empty field of `self` from `other`.
    #[must_use]
    pub fn or(self, other: Self) -> Self {
        Self {
            title: self.title.or(other.title),
            brand: self.brand.or(other.brand),
            price: self.price.or(other.price),
            list_price: self.list_price.or(other.list_price),
            availability: self.availability.or(other.availability),
            gtin: self.gtin.or(other.gtin),
            sku: self.sku.or(other.sku),
            url: self.url.or(other.url),
        }
    }

    /// Builds a validated [`Offer`]. The URL defaults to the page URL and must
    /// resolve to one of the retailer's hosts; an invalid GTIN is dropped;
    /// `membership_required` comes from the registry.
    ///
    /// # Errors
    /// [`ParseError::MissingField`] for a missing title, price or SKU;
    /// [`ParseError::InvalidField`] for an invalid SKU or off-site URL.
    pub fn finish(self, info: &RetailerInfo, page: &Page) -> Result<Offer, ParseError> {
        let title = self
            .title
            .filter(|t| !t.trim().is_empty())
            .ok_or(ParseError::MissingField("title"))?;
        let price = self.price.ok_or(ParseError::MissingField("price"))?;
        let sku_text = self.sku.ok_or(ParseError::MissingField("retailer_sku"))?;
        let retailer_sku =
            RetailerSku::new(&sku_text).map_err(|_| ParseError::InvalidField("retailer_sku"))?;
        let url = match self.url {
            Some(u) => absolutize(page, &u).ok_or(ParseError::InvalidField("url"))?,
            None => page.url.as_str().to_owned(),
        };
        if !on_retailer_host(info, &url) {
            return Err(ParseError::InvalidField("url"));
        }
        let retailer =
            RetailerId::new(info.id).map_err(|_| ParseError::InvalidField("retailer"))?;
        Offer {
            retailer,
            title,
            brand: self.brand,
            price_mxn: price,
            list_price_mxn: self.list_price,
            availability: self.availability.unwrap_or(Availability::Unknown),
            gtin: self.gtin.as_deref().and_then(gtin),
            retailer_sku,
            url,
            membership_required: info.membership_required,
            fetched_at: page.fetched_at,
        }
        .validate()
        .map_err(|e| match e {
            domain::OfferError::EmptyTitle => ParseError::MissingField("title"),
            _ => ParseError::InvalidField("offer"),
        })
    }
}

/// A valid GTIN from `raw`, left-padding short codes with zeros to GTIN-13/14.
#[must_use]
pub fn gtin(raw: &str) -> Option<Gtin> {
    let t = raw.trim();
    if t.len() < 8 || !t.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Gtin::new(t)
        .ok()
        .or_else(|| (t.len() < 13).then(|| Gtin::new(&format!("{t:0>13}")).ok())?)
        .or_else(|| (t.len() < 14).then(|| Gtin::new(&format!("{t:0>14}")).ok())?)
}

/// Resolves `href` against the page URL; returns an `https://` URL or `None`
/// for non-web schemes.
#[must_use]
pub fn absolutize(page: &Page, href: &str) -> Option<String> {
    let h = href.trim();
    if h.is_empty() {
        return None;
    }
    let host = page.url.host();
    let abs = if let Some(rest) = h.strip_prefix("https://") {
        format!("https://{rest}")
    } else if let Some(rest) = h.strip_prefix("http://") {
        format!("https://{rest}")
    } else if let Some(rest) = h.strip_prefix("//") {
        format!("https://{rest}")
    } else if h.starts_with('/') {
        format!("https://{host}{h}")
    } else if h.contains(':') {
        return None;
    } else {
        format!("https://{host}/{h}")
    };
    Some(abs)
}

/// Lowercase host of an absolute URL (between `://` and the next `/?#:`).
#[must_use]
pub fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let end = rest.find(['/', '?', '#', ':']).unwrap_or(rest.len());
    let host = &rest[..end];
    (!host.is_empty() && !host.contains('@')).then(|| host.to_ascii_lowercase())
}

/// True when `url`'s host is one of the retailer's hosts.
#[must_use]
pub fn on_retailer_host(info: &RetailerInfo, url: &str) -> bool {
    host_of(url).is_some_and(|h| info.hosts.iter().any(|x| x.eq_ignore_ascii_case(&h)))
}

/// Fails with [`ParseError::WrongHost`] unless the page is on the retailer.
///
/// # Errors
/// [`ParseError::WrongHost`].
pub fn check_host(info: &RetailerInfo, page: &Page) -> Result<(), ParseError> {
    if on_retailer_host(info, page.url.as_str()) {
        Ok(())
    } else {
        Err(ParseError::WrongHost)
    }
}

/// Collapses whitespace; `None` if nothing remains.
#[must_use]
pub fn clean_text(s: &str) -> Option<String> {
    let t = s.split_whitespace().collect::<Vec<_>>().join(" ");
    (!t.is_empty()).then_some(t)
}

/// Keeps the offers whose URL is new, in order (search pages repeat items).
#[must_use]
pub fn dedup_by_url(offers: Vec<Offer>) -> Vec<Offer> {
    let mut seen = std::collections::BTreeSet::new();
    offers
        .into_iter()
        .filter(|o| seen.insert(o.url.clone()))
        .collect()
}

/// Converts parsed drafts into offers, skipping failures; errors with
/// [`ParseError::NoOffers`] when none survive.
///
/// # Errors
/// [`ParseError::NoOffers`].
pub fn finish_all(
    drafts: Vec<Draft>,
    info: &RetailerInfo,
    page: &Page,
) -> Result<Vec<Offer>, ParseError> {
    let offers = dedup_by_url(
        drafts
            .into_iter()
            .filter_map(|d| d.finish(info, page).ok())
            .collect(),
    );
    if offers.is_empty() {
        Err(ParseError::NoOffers)
    } else {
        Ok(offers)
    }
}

/// One way of extracting search-page drafts (JSON-LD, page JSON or CSS).
pub type SearchSource = fn(&scraper::Html) -> Vec<Draft>;

/// One way of extracting a product-page draft.
pub type ProductSource = fn(&scraper::Html) -> Draft;

/// Search offers from the first source (in priority order) that yields any.
///
/// # Errors
/// [`ParseError::WrongHost`]; [`ParseError::NoResults`] when no source
/// yields an offer and the page states it has zero results
/// ([`jsonld::declares_no_results`]); otherwise [`ParseError::NoOffers`].
pub fn search_from(
    info: &RetailerInfo,
    page: &Page,
    sources: &[SearchSource],
) -> Result<Vec<Offer>, ParseError> {
    check_host(info, page)?;
    let doc = scraper::Html::parse_document(&page.body);
    for source in sources {
        if let Ok(offers) = finish_all(source(&doc), info, page) {
            return Ok(offers);
        }
    }
    if jsonld::declares_no_results(&doc) {
        Err(ParseError::NoResults)
    } else {
        Err(ParseError::NoOffers)
    }
}

/// The product offer: fields from earlier sources win; later sources only
/// fill what is missing.
///
/// # Errors
/// [`ParseError::WrongHost`] or the first missing required field.
pub fn product_from(
    info: &RetailerInfo,
    page: &Page,
    sources: &[ProductSource],
) -> Result<Offer, ParseError> {
    check_host(info, page)?;
    let doc = scraper::Html::parse_document(&page.body);
    sources
        .iter()
        .fold(Draft::default(), |acc, source| acc.or(source(&doc)))
        .finish(info, page)
}

/// Drafts for every JSON-LD `Product` on the page.
#[must_use]
pub fn jsonld_drafts(doc: &scraper::Html) -> Vec<Draft> {
    jsonld::products(doc)
        .iter()
        .map(jsonld::product_draft)
        .collect()
}

/// Draft for the first JSON-LD `Product` on the page.
#[must_use]
pub fn jsonld_draft(doc: &scraper::Html) -> Draft {
    jsonld::products(doc)
        .first()
        .map(jsonld::product_draft)
        .unwrap_or_default()
}

/// Draft from Open Graph / product `<meta>` tags (CSS fallback on product
/// pages): `og:title`, `og:url`, `product:price:amount`, `product:brand`,
/// `product:retailer_item_id`, `product:availability`.
#[must_use]
pub fn meta_draft(doc: &scraper::Html) -> Draft {
    let meta = |names: &[&str]| {
        names.iter().find_map(|n| {
            css::first_attr(
                doc,
                &[
                    &format!(r#"meta[property="{n}"]"#),
                    &format!(r#"meta[name="{n}"]"#),
                ],
                "content",
            )
        })
    };
    Draft {
        title: meta(&["og:title"]),
        brand: meta(&["product:brand", "og:brand"]),
        price: meta(&[
            "product:price:amount",
            "og:price:amount",
            "og:product:price:amount",
        ])
        .and_then(|p| price::from_str(&p)),
        availability: meta(&["product:availability", "og:availability"])
            .map(|a| Availability::from_schema_org(&a.replace(' ', ""))),
        sku: meta(&["product:retailer_item_id"]),
        url: css::first_attr(doc, &[r#"link[rel="canonical"]"#], "href")
            .or_else(|| meta(&["og:url"])),
        ..Draft::default()
    }
}

/// Text with every whitespace removed (prices split across spans).
#[must_use]
pub fn compact(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

#[cfg(test)]
pub(crate) mod test_support {
    use brightdata::TargetUrl;
    use domain::UnixMillis;

    use crate::adapter::Page;

    /// Removes every JSON-LD and data script, leaving only HTML for the CSS
    /// fallback tests.
    pub fn strip_json(html: &str) -> String {
        let mut out = String::with_capacity(html.len());
        let mut rest = html;
        while let Some(start) = rest.find("<script") {
            out.push_str(&rest[..start]);
            match rest[start..].find("</script>") {
                Some(end) => rest = &rest[start + end + "</script>".len()..],
                None => return out,
            }
        }
        out.push_str(rest);
        out
    }

    /// A page at `url` with `body`, fetched at a fixed time.
    pub fn page(url: &str, body: &str) -> Page {
        Page {
            url: TargetUrl::parse(url).unwrap(),
            body: body.to_owned(),
            fetched_at: UnixMillis(1_760_000_000_000),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::page;
    use super::*;
    use crate::registry::LIVERPOOL;

    fn draft() -> Draft {
        Draft {
            title: Some("Licuadora".into()),
            price: Some(PriceMxn::from_centavos(99_900).unwrap()),
            sku: Some("1100".into()),
            url: Some("/tienda/pdp/licuadora/1100".into()),
            gtin: Some("36000291452".into()),
            ..Draft::default()
        }
    }

    #[test]
    fn finish_resolves_url_and_pads_gtin() {
        let p = page("https://www.liverpool.com.mx/tienda?s=x", "");
        let o = draft().finish(&LIVERPOOL, &p).unwrap();
        assert_eq!(
            o.url,
            "https://www.liverpool.com.mx/tienda/pdp/licuadora/1100"
        );
        assert_eq!(o.gtin.unwrap().as_str(), "0036000291452");
        assert_eq!(o.availability, Availability::Unknown);
        assert_eq!(o.fetched_at, p.fetched_at);
        assert!(!o.membership_required);
    }

    #[test]
    fn finish_rejects_missing_fields_and_off_site_urls() {
        let p = page("https://www.liverpool.com.mx/", "");
        let mut d = draft();
        d.price = None;
        assert_eq!(
            d.finish(&LIVERPOOL, &p),
            Err(ParseError::MissingField("price"))
        );
        let mut d = draft();
        d.url = Some("https://evil.example/x".into());
        assert_eq!(
            d.finish(&LIVERPOOL, &p),
            Err(ParseError::InvalidField("url"))
        );
        let mut d = draft();
        d.url = Some("javascript:alert(1)".into());
        assert_eq!(
            d.finish(&LIVERPOOL, &p),
            Err(ParseError::InvalidField("url"))
        );
        let mut d = draft();
        d.gtin = Some("123".into());
        assert!(d.finish(&LIVERPOOL, &p).unwrap().gtin.is_none());
    }

    #[test]
    fn host_checks() {
        assert_eq!(host_of("https://A.b.mx:443/x"), Some("a.b.mx".into()));
        assert_eq!(host_of("https://u@evil.mx/"), None);
        let wrong = page("https://evil.example/", "");
        assert_eq!(check_host(&LIVERPOOL, &wrong), Err(ParseError::WrongHost));
    }

    #[test]
    fn finish_all_dedups_and_requires_one() {
        let p = page("https://www.liverpool.com.mx/", "");
        let offers = finish_all(vec![draft(), draft(), Draft::default()], &LIVERPOOL, &p).unwrap();
        assert_eq!(offers.len(), 1);
        assert_eq!(
            finish_all(vec![Draft::default()], &LIVERPOOL, &p),
            Err(ParseError::NoOffers)
        );
    }
}
