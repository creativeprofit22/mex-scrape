//! The normalized retailer offer and its value types.
//!
//! Every store adapter turns a fetched page into [`Offer`] values. Prices are
//! integer centavos (never floats); GTINs are check-digit validated.

use std::fmt;

/// Why a domain value was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OfferError {
    /// A retailer id was empty or not lowercase `[a-z0-9_]`.
    InvalidRetailerId(String),
    /// A price string could not be read as Mexican pesos.
    InvalidPrice(String),
    /// A GTIN had the wrong length, a non-digit or a bad check digit.
    InvalidGtin(String),
    /// A retailer SKU was empty or longer than 100 characters.
    InvalidSku(String),
    /// The offer title was empty.
    EmptyTitle,
    /// The offer URL was not an absolute `https://` URL.
    InvalidUrl(String),
}

impl fmt::Display for OfferError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRetailerId(v) => write!(f, "invalid retailer id: {v:?}"),
            Self::InvalidPrice(v) => write!(f, "invalid MXN price: {v:?}"),
            Self::InvalidGtin(v) => write!(f, "invalid GTIN: {v:?}"),
            Self::InvalidSku(v) => write!(f, "invalid retailer SKU: {v:?}"),
            Self::EmptyTitle => f.write_str("offer title is empty"),
            Self::InvalidUrl(v) => write!(f, "invalid offer URL: {v:?}"),
        }
    }
}

impl std::error::Error for OfferError {}

/// Stable lowercase retailer id, e.g. `walmart_mx`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RetailerId(String);

impl RetailerId {
    /// Validates a lowercase `[a-z0-9_]` id of 1..=40 characters.
    ///
    /// # Errors
    /// [`OfferError::InvalidRetailerId`] if the id is empty, too long or has
    /// other characters.
    pub fn new(id: &str) -> Result<Self, OfferError> {
        let ok = !id.is_empty()
            && id.len() <= 40
            && id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
        if ok {
            Ok(Self(id.to_owned()))
        } else {
            Err(OfferError::InvalidRetailerId(id.to_owned()))
        }
    }

    /// The id text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RetailerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A non-negative amount in Mexican pesos, stored as integer centavos.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PriceMxn(i64);

impl PriceMxn {
    /// Builds a price from centavos.
    ///
    /// # Errors
    /// [`OfferError::InvalidPrice`] if `centavos` is negative.
    pub fn from_centavos(centavos: i64) -> Result<Self, OfferError> {
        if centavos < 0 {
            Err(OfferError::InvalidPrice(centavos.to_string()))
        } else {
            Ok(Self(centavos))
        }
    }

    /// The amount in centavos.
    #[must_use]
    pub const fn centavos(self) -> i64 {
        self.0
    }

    /// Parses a plain decimal such as `1299`, `1299.5` or `1299.00` (the
    /// format used in JSON-LD and page JSON). A third decimal rounds half up;
    /// further decimals are ignored.
    ///
    /// # Errors
    /// [`OfferError::InvalidPrice`] for anything else (signs, separators,
    /// exponents, empty input or overflow).
    pub fn from_decimal_str(s: &str) -> Result<Self, OfferError> {
        let bad = || OfferError::InvalidPrice(s.to_owned());
        let s_trim = s.trim();
        let (int_part, frac_part) = match s_trim.split_once('.') {
            Some((i, f)) => (i, f),
            None => (s_trim, ""),
        };
        let all_digits = |p: &str| p.bytes().all(|b| b.is_ascii_digit());
        if int_part.is_empty() && frac_part.is_empty()
            || !all_digits(int_part)
            || !all_digits(frac_part)
            || (s_trim.contains('.') && frac_part.is_empty())
        {
            return Err(bad());
        }
        let pesos: i64 = if int_part.is_empty() {
            0
        } else {
            int_part.parse().map_err(|_| bad())?
        };
        let digit = |i: usize| i64::from(frac_part.as_bytes().get(i).map_or(0, |b| b - b'0'));
        let mut cents = digit(0) * 10 + digit(1);
        if digit(2) >= 5 {
            cents += 1;
        }
        pesos
            .checked_mul(100)
            .and_then(|c| c.checked_add(cents))
            .map(Self)
            .ok_or_else(bad)
    }

    /// Parses a displayed Mexican price such as `$1,299.00`, `$ 1,299`,
    /// `MXN 1299.50` or `1,299.00 MXN` (comma thousands, dot decimals).
    ///
    /// # Errors
    /// [`OfferError::InvalidPrice`] if no valid amount remains after removing
    /// the currency marks and thousands separators.
    pub fn parse_mxn(s: &str) -> Result<Self, OfferError> {
        let bad = || OfferError::InvalidPrice(s.to_owned());
        let mut t = s.trim();
        for mark in ["MXN", "MN", "M.N.", "$"] {
            t = t.trim_start_matches(mark).trim();
            t = t.trim_end_matches(mark).trim();
        }
        t = t.trim_start_matches('$').trim();
        let (int_part, frac_part) = match t.split_once('.') {
            Some((i, f)) => (i, Some(f)),
            None => (t, None),
        };
        // Thousands groups must be 3 digits after the first group.
        if int_part.contains(',') {
            let mut groups = int_part.split(',');
            let first = groups.next().unwrap_or_default();
            let first_ok = (1..=3).contains(&first.len());
            if !first_ok || groups.any(|g| g.len() != 3) {
                return Err(bad());
            }
        }
        let digits: String = int_part.chars().filter(|c| *c != ',').collect();
        let plain = match frac_part {
            Some(f) => format!("{digits}.{f}"),
            None => digits,
        };
        Self::from_decimal_str(&plain).map_err(|_| bad())
    }
}

impl fmt::Display for PriceMxn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "${}.{:02} MXN", self.0 / 100, self.0 % 100)
    }
}

/// A GS1 GTIN (8, 12, 13 or 14 digits) with a valid check digit.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Gtin(String);

impl Gtin {
    /// Validates a GTIN-8/12/13/14 (surrounding whitespace is trimmed).
    ///
    /// # Errors
    /// [`OfferError::InvalidGtin`] on a wrong length, a non-digit, all zeros
    /// (a common "no barcode" placeholder) or a bad check digit.
    pub fn new(s: &str) -> Result<Self, OfferError> {
        let t = s.trim();
        let bad = || OfferError::InvalidGtin(s.to_owned());
        if !matches!(t.len(), 8 | 12 | 13 | 14)
            || !t.bytes().all(|b| b.is_ascii_digit())
            || t.bytes().all(|b| b == b'0')
        {
            return Err(bad());
        }
        let digits: Vec<u32> = t.bytes().map(|b| u32::from(b - b'0')).collect();
        let (body, check) = digits.split_at(digits.len() - 1);
        let sum: u32 = body
            .iter()
            .rev()
            .enumerate()
            .map(|(i, d)| if i % 2 == 0 { d * 3 } else { *d })
            .sum();
        if (10 - sum % 10) % 10 == check[0] {
            Ok(Self(t.to_owned()))
        } else {
            Err(bad())
        }
    }

    /// The digits.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The retailer's own product id (non-empty, at most 100 characters).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RetailerSku(String);

impl RetailerSku {
    /// Validates a trimmed, non-empty SKU of at most 100 characters.
    ///
    /// # Errors
    /// [`OfferError::InvalidSku`] if it is empty, too long or has control
    /// characters.
    pub fn new(s: &str) -> Result<Self, OfferError> {
        let t = s.trim();
        if t.is_empty() || t.chars().count() > 100 || t.chars().any(char::is_control) {
            Err(OfferError::InvalidSku(s.to_owned()))
        } else {
            Ok(Self(t.to_owned()))
        }
    }

    /// The SKU text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RetailerSku {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Stock state of an offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Availability {
    /// Can be bought now.
    InStock,
    /// Sold out or discontinued.
    OutOfStock,
    /// Orderable before it ships (pre-order, pre-sale, back order).
    PreOrder,
    /// The page did not say.
    Unknown,
}

impl Availability {
    /// Maps a schema.org `ItemAvailability` value (full URL or bare name,
    /// case-insensitive) to an [`Availability`]; anything else is `Unknown`.
    #[must_use]
    pub fn from_schema_org(value: &str) -> Self {
        let name = value.trim().rsplit('/').next().unwrap_or_default();
        match name.to_ascii_lowercase().as_str() {
            "instock" | "limitedavailability" | "onlineonly" | "instoreonly" => Self::InStock,
            "outofstock" | "soldout" | "discontinued" => Self::OutOfStock,
            "preorder" | "presale" | "backorder" => Self::PreOrder,
            _ => Self::Unknown,
        }
    }

    /// Stable lowercase name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InStock => "in_stock",
            Self::OutOfStock => "out_of_stock",
            Self::PreOrder => "pre_order",
            Self::Unknown => "unknown",
        }
    }
    /// Reads a name produced by [`Self::as_str`].
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "in_stock" => Some(Self::InStock),
            "out_of_stock" => Some(Self::OutOfStock),
            "pre_order" => Some(Self::PreOrder),
            "unknown" => Some(Self::Unknown),
            _ => None,
        }
    }
}

/// Milliseconds since the Unix epoch (UTC).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UnixMillis(pub i64);

/// One normalized offer from a Mexican retailer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    /// Which retailer sells it.
    pub retailer: RetailerId,
    /// Product title as shown by the retailer.
    pub title: String,
    /// Brand, when the page names one.
    pub brand: Option<String>,
    /// Current selling price.
    pub price_mxn: PriceMxn,
    /// Strike-through list price, when higher than the selling price.
    pub list_price_mxn: Option<PriceMxn>,
    /// Stock state.
    pub availability: Availability,
    /// GTIN/EAN/UPC, when the page has a valid one.
    pub gtin: Option<Gtin>,
    /// The retailer's own product id.
    pub retailer_sku: RetailerSku,
    /// Absolute `https://` product URL.
    pub url: String,
    /// True when buying needs a paid membership (Costco, Sam's Club).
    pub membership_required: bool,
    /// When the page was fetched.
    pub fetched_at: UnixMillis,
}

impl Offer {
    /// Normalizes and checks an offer: trims title and brand (an empty brand
    /// becomes `None`), drops a list price that is not above the price, and
    /// requires a non-empty title and an absolute `https://` URL.
    ///
    /// # Errors
    /// [`OfferError::EmptyTitle`] or [`OfferError::InvalidUrl`].
    pub fn validate(mut self) -> Result<Self, OfferError> {
        self.title = collapse_ws(&self.title);
        if self.title.is_empty() {
            return Err(OfferError::EmptyTitle);
        }
        self.brand = self
            .brand
            .map(|b| collapse_ws(&b))
            .filter(|b| !b.is_empty());
        if self.list_price_mxn.is_some_and(|l| l <= self.price_mxn) {
            self.list_price_mxn = None;
        }
        let url_ok = self
            .url
            .strip_prefix("https://")
            .is_some_and(|rest| !rest.is_empty() && !rest.starts_with('/'))
            && !self
                .url
                .chars()
                .any(|c| c.is_whitespace() || c.is_control());
        if !url_ok {
            return Err(OfferError::InvalidUrl(self.url));
        }
        Ok(self)
    }
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> i64 {
        PriceMxn::parse_mxn(s).unwrap().centavos()
    }

    #[test]
    fn retailer_id_accepts_lowercase_snake_only() {
        assert_eq!(
            RetailerId::new("walmart_mx").unwrap().as_str(),
            "walmart_mx"
        );
        for bad in ["", "Walmart", "walmart-mx", "a b", &"a".repeat(41)] {
            assert!(RetailerId::new(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn parse_mxn_reads_display_formats() {
        assert_eq!(p("$1,299.00"), 129_900);
        assert_eq!(p("$ 1,299"), 129_900);
        assert_eq!(p("MXN 1299.5"), 129_950);
        assert_eq!(p("1,299.00 MXN"), 129_900);
        assert_eq!(p("$12,345,678.99"), 1_234_567_899);
        assert_eq!(p("$0.99"), 99);
        for bad in [
            "", "$", "abc", "-5", "1,29.00", "1.299,00", "$1e3", "1..2", "12.",
        ] {
            assert!(PriceMxn::parse_mxn(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn decimal_str_rounds_third_digit_and_rejects_junk() {
        let d = |s| PriceMxn::from_decimal_str(s).unwrap().centavos();
        assert_eq!(d("1299"), 129_900);
        assert_eq!(d("1299.5"), 129_950);
        assert_eq!(d("1299.005"), 129_901);
        assert_eq!(d("1299.004"), 129_900);
        assert_eq!(d(".5"), 50);
        for bad in ["", ".", "1,299", "+1", "-1", "1e3", "99999999999999999999"] {
            assert!(PriceMxn::from_decimal_str(bad).is_err(), "{bad}");
        }
        assert!(PriceMxn::from_centavos(-1).is_err());
        assert_eq!(
            PriceMxn::from_centavos(129_950).unwrap().to_string(),
            "$1299.50 MXN"
        );
    }

    #[test]
    fn gtin_validates_length_and_check_digit() {
        for ok in [
            "7501055300075",
            "96385074",
            "036000291452",
            "10614141000415",
        ] {
            assert_eq!(Gtin::new(ok).unwrap().as_str(), ok);
        }
        for bad in [
            "7501055300076",
            "123",
            "75010553000a5",
            "",
            "1234567890123456",
            "0000000000000",
        ] {
            assert!(Gtin::new(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn sku_must_be_non_empty_and_short() {
        assert_eq!(RetailerSku::new(" 00750 ").unwrap().as_str(), "00750");
        assert!(RetailerSku::new("  ").is_err());
        assert!(RetailerSku::new(&"x".repeat(101)).is_err());
        assert!(RetailerSku::new("a\nb").is_err());
    }

    #[test]
    fn availability_maps_schema_org_values() {
        use Availability::*;
        assert_eq!(
            Availability::from_schema_org("https://schema.org/InStock"),
            InStock
        );
        assert_eq!(
            Availability::from_schema_org("http://schema.org/OutOfStock"),
            OutOfStock
        );
        assert_eq!(Availability::from_schema_org("instock"), InStock);
        assert_eq!(Availability::from_schema_org("PreOrder"), PreOrder);
        assert_eq!(Availability::from_schema_org("SoldOut"), OutOfStock);
        assert_eq!(Availability::from_schema_org("whatever"), Unknown);
        for a in [InStock, OutOfStock, PreOrder, Unknown] {
            assert_eq!(Availability::parse(a.as_str()), Some(a));
        }
        assert_eq!(Availability::parse("InStock"), None);
    }

    fn offer() -> Offer {
        Offer {
            retailer: RetailerId::new("liverpool").unwrap(),
            title: "  Licuadora   Oster ".into(),
            brand: Some(" ".into()),
            price_mxn: PriceMxn::from_centavos(100_000).unwrap(),
            list_price_mxn: Some(PriceMxn::from_centavos(90_000).unwrap()),
            availability: Availability::InStock,
            gtin: None,
            retailer_sku: RetailerSku::new("1100").unwrap(),
            url: "https://www.liverpool.com.mx/tienda/pdp/x/1100".into(),
            membership_required: false,
            fetched_at: UnixMillis(1),
        }
    }

    #[test]
    fn validate_normalizes_and_drops_low_list_price() {
        let o = offer().validate().unwrap();
        assert_eq!(o.title, "Licuadora Oster");
        assert_eq!(o.brand, None);
        assert_eq!(o.list_price_mxn, None);

        let mut higher = offer();
        higher.list_price_mxn = Some(PriceMxn::from_centavos(120_000).unwrap());
        assert!(higher.validate().unwrap().list_price_mxn.is_some());
    }

    #[test]
    fn validate_rejects_empty_title_and_bad_url() {
        let mut o = offer();
        o.title = " ".into();
        assert_eq!(o.validate(), Err(OfferError::EmptyTitle));
        for url in ["http://x.mx/p", "/tienda/pdp", "https://", "https://a b"] {
            let mut o = offer();
            o.url = url.into();
            assert!(o.validate().is_err(), "{url}");
        }
    }
}
