//! Amazon identifiers.

use std::fmt;

/// Why an Amazon identifier was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AsinError {
    /// Not exactly 10 ASCII letters or digits.
    Invalid(String),
}

impl fmt::Display for AsinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(v) => write!(f, "invalid ASIN: {v:?}"),
        }
    }
}

impl std::error::Error for AsinError {}

/// An Amazon Standard Identification Number: 10 ASCII uppercase letters or
/// digits, e.g. `B0CHHSFMRL`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Asin(String);

impl Asin {
    /// Validates an ASIN. Surrounding whitespace is trimmed and lowercase
    /// letters are upper-cased.
    ///
    /// # Errors
    /// [`AsinError::Invalid`] unless exactly 10 ASCII alphanumerics remain.
    pub fn new(s: &str) -> Result<Self, AsinError> {
        let t = s.trim();
        if t.len() == 10 && t.bytes().all(|b| b.is_ascii_alphanumeric()) {
            Ok(Self(t.to_ascii_uppercase()))
        } else {
            Err(AsinError::Invalid(s.to_owned()))
        }
    }

    /// The ASIN text (uppercase).
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Asin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asin_accepts_ten_alphanumerics_and_uppercases() {
        assert_eq!(Asin::new("B0CHHSFMRL").unwrap().as_str(), "B0CHHSFMRL");
        assert_eq!(Asin::new(" b0chhsfmrl ").unwrap().as_str(), "B0CHHSFMRL");
        assert_eq!(Asin::new("0123456789").unwrap().as_str(), "0123456789");
    }

    #[test]
    fn asin_rejects_wrong_length_and_symbols() {
        for bad in [
            "",
            "B0CHHSFMR",
            "B0CHHSFMRLX",
            "B0CHHSFMR-",
            "B0CHHS FMR",
            "B0CHHSFMRÉ",
            "../../etc",
        ] {
            assert!(Asin::new(bad).is_err(), "{bad}");
        }
    }
}
