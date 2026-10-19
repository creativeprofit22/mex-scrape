//! Validated identifiers and inputs. Everything that ends up in a URL path
//! or query is checked before any I/O happens.

use std::fmt;

use http_guard::Url;

use crate::error::BrightDataError;

/// Longest snapshot or dataset id accepted.
const MAX_ID_LEN: usize = 200;
/// Longest target URL accepted by [`TargetUrl::parse`].
const MAX_TARGET_URL_LEN: usize = 2000;

fn is_plain_id(value: &str, max_len: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/// A Datasets v3 snapshot id: 1–200 characters of `[A-Za-z0-9_-]`, so it is
/// safe to put in a URL path.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SnapshotId(String);

impl SnapshotId {
    /// Validates a snapshot id (from Bright Data or from storage).
    ///
    /// # Errors
    ///
    /// [`BrightDataError::Terminal`] with fixed text for anything else.
    pub fn parse(value: &str) -> Result<Self, BrightDataError> {
        if is_plain_id(value, MAX_ID_LEN) {
            Ok(Self(value.to_owned()))
        } else {
            Err(BrightDataError::Terminal(
                "Bright Data returned an invalid snapshot id".to_owned(),
            ))
        }
    }

    /// The id text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SnapshotId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A Datasets v3 dataset id (e.g. `gd_l7q7dkf244hwjntr0`): same character
/// rule as [`SnapshotId`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DatasetId(String);

impl DatasetId {
    /// Validates a dataset id.
    ///
    /// # Errors
    ///
    /// [`BrightDataError::Terminal`] with fixed text.
    pub fn parse(value: &str) -> Result<Self, BrightDataError> {
        if is_plain_id(value, MAX_ID_LEN) {
            Ok(Self(value.to_owned()))
        } else {
            Err(BrightDataError::Terminal("invalid dataset id".to_owned()))
        }
    }

    /// The id text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A Datasets v3 `discover_by` value (e.g. `keyword`, `url`): 1–100
/// characters of `[A-Za-z0-9_-]`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DiscoverBy(String);

impl DiscoverBy {
    /// Validates a `discover_by` value.
    ///
    /// # Errors
    ///
    /// [`BrightDataError::Terminal`] with fixed text.
    pub fn parse(value: &str) -> Result<Self, BrightDataError> {
        if is_plain_id(value, 100) {
            Ok(Self(value.to_owned()))
        } else {
            Err(BrightDataError::Terminal(
                "invalid discover_by value".to_owned(),
            ))
        }
    }

    /// The value text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A page for Web Unlocker to fetch: absolute `http`/`https` URL with a
/// host, no userinfo, at most 2000 characters.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TargetUrl(String);

impl TargetUrl {
    /// Validates a target URL. Error text is fixed and never echoes the
    /// value.
    ///
    /// # Errors
    ///
    /// [`BrightDataError::Terminal`].
    pub fn parse(value: &str) -> Result<Self, BrightDataError> {
        let invalid = || BrightDataError::Terminal("invalid target URL".to_owned());
        let trimmed = value.trim();
        if trimmed.is_empty() || trimmed.len() > MAX_TARGET_URL_LEN {
            return Err(invalid());
        }
        let url = Url::parse(trimmed).map_err(|_| invalid())?;
        let scheme_ok = matches!(url.scheme(), "http" | "https");
        let has_host = url.host_str().is_some_and(|host| !host.is_empty());
        if !scheme_ok || !has_host || !url.username().is_empty() || url.password().is_some() {
            return Err(invalid());
        }
        Ok(Self(url.into()))
    }

    /// The normalized URL text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// An ISO 3166-1 alpha-2 country for Web Unlocker geo-targeting, stored
/// lowercase (e.g. `mx`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CountryCode([u8; 2]);

impl CountryCode {
    /// Mexico.
    pub const MX: Self = Self(*b"mx");

    /// Validates two ASCII letters (any case).
    ///
    /// # Errors
    ///
    /// [`BrightDataError::Terminal`] with fixed text.
    pub fn parse(value: &str) -> Result<Self, BrightDataError> {
        match value.as_bytes() {
            [a, b] if a.is_ascii_alphabetic() && b.is_ascii_alphabetic() => {
                Ok(Self([a.to_ascii_lowercase(), b.to_ascii_lowercase()]))
            }
            _ => Err(BrightDataError::Terminal("invalid country code".to_owned())),
        }
    }

    /// The lowercase code.
    #[must_use]
    pub fn as_str(&self) -> &str {
        // Both bytes are ASCII letters (checked in `parse`).
        std::str::from_utf8(&self.0).unwrap_or("")
    }
}

#[cfg(test)]
mod tests {
    use super::{CountryCode, DatasetId, DiscoverBy, SnapshotId, TargetUrl};

    #[test]
    fn snapshot_ids_must_be_plain() {
        for bad in [
            "",
            "../x",
            "a/b",
            "a?b",
            "a b",
            "a%2F",
            "é",
            &"a".repeat(201),
        ] {
            assert!(SnapshotId::parse(bad).is_err(), "{bad:?}");
        }
        let ok = "sd_ABC-123_x".to_owned() + &"a".repeat(188);
        assert_eq!(SnapshotId::parse(&ok).unwrap().as_str(), ok);
        assert!(DatasetId::parse("gd_l7q7dkf244hwjntr0").is_ok());
        assert!(DatasetId::parse("gd/../x").is_err());
        assert!(DiscoverBy::parse("keyword").is_ok());
        assert!(DiscoverBy::parse("key word").is_err());
    }

    #[test]
    fn target_urls_are_absolute_web_urls_without_userinfo() {
        assert_eq!(
            TargetUrl::parse(" https://www.liverpool.com.mx/tienda?s=tv ")
                .unwrap()
                .as_str(),
            "https://www.liverpool.com.mx/tienda?s=tv"
        );
        assert!(TargetUrl::parse("http://example.com").is_ok());
        for bad in [
            "",
            "/relative",
            "ftp://example.com/",
            "file:///etc/passwd",
            "https://user:pw@example.com/",
            "https://user@example.com/",
            "javascript:alert(1)",
        ] {
            let error = TargetUrl::parse(bad).unwrap_err();
            assert_eq!(error.message(), "invalid target URL", "{bad}");
        }
        let long = format!("https://example.com/{}", "a".repeat(2000));
        assert!(TargetUrl::parse(&long).is_err());
    }

    #[test]
    fn country_codes_are_two_letters() {
        assert_eq!(CountryCode::parse("MX").unwrap(), CountryCode::MX);
        assert_eq!(CountryCode::MX.as_str(), "mx");
        for bad in ["", "m", "mex", "m1", "ñx"] {
            assert!(CountryCode::parse(bad).is_err(), "{bad}");
        }
    }
}
