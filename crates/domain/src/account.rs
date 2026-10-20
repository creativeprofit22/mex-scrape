//! User accounts: ids, e-mail addresses, plans, store memberships and the UI
//! language (ADR 0012).

use std::fmt;

use crate::RetailerId;

/// Retailer id of Costco México.
pub const COSTCO_RETAILER_ID: &str = "costco_mx";
/// Retailer id of Sam's Club México.
pub const SAMS_RETAILER_ID: &str = "sams_mx";

/// Whether `retailer` is a membership store (Costco or Sam's Club). Must
/// agree with the retailer registry's `membership_required` flags.
#[must_use]
pub fn is_membership_retailer(retailer: &RetailerId) -> bool {
    matches!(retailer.as_str(), COSTCO_RETAILER_ID | SAMS_RETAILER_ID)
}

/// Shortest accepted password, in characters.
pub const MIN_PASSWORD_CHARS: usize = 8;
/// Longest accepted password, in characters (bounds hashing cost).
pub const MAX_PASSWORD_CHARS: usize = 128;
/// Longest accepted e-mail address, in bytes (RFC 5321 path limit).
pub const MAX_EMAIL_BYTES: usize = 254;

/// A user's numeric id (positive, assigned by the database).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UserId(i64);

impl UserId {
    /// Wraps a database id.
    ///
    /// Returns `None` unless `id > 0`.
    #[must_use]
    pub const fn new(id: i64) -> Option<Self> {
        if id > 0 { Some(Self(id)) } else { None }
    }

    /// The raw id.
    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }
}

impl fmt::Display for UserId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// An address that is not a plausible e-mail. The value is never echoed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmailError;

impl fmt::Display for EmailError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("invalid e-mail address")
    }
}

impl std::error::Error for EmailError {}

/// A normalized e-mail address: trimmed, lowercased, at most 254 bytes, one
/// `@` with a non-empty local part and a dotted domain, no whitespace or
/// control characters.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Email(String);

impl Email {
    /// Normalizes and validates an address.
    ///
    /// # Errors
    /// [`EmailError`] if the address is malformed or too long.
    pub fn parse(raw: &str) -> Result<Self, EmailError> {
        let e = raw.trim().to_lowercase();
        if e.is_empty()
            || e.len() > MAX_EMAIL_BYTES
            || e.chars().any(|c| c.is_whitespace() || c.is_control())
        {
            return Err(EmailError);
        }
        let mut parts = e.split('@');
        let (Some(local), Some(domain), None) = (parts.next(), parts.next(), parts.next()) else {
            return Err(EmailError);
        };
        let domain_ok = domain.contains('.')
            && !domain.starts_with('.')
            && !domain.ends_with('.')
            && !domain.contains("..");
        if local.is_empty() || !domain_ok {
            return Err(EmailError);
        }
        Ok(Self(e))
    }

    /// The normalized address.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A new password outside the length policy. The value is never echoed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasswordPolicyError {
    /// Fewer than [`MIN_PASSWORD_CHARS`] characters.
    TooShort,
    /// More than [`MAX_PASSWORD_CHARS`] characters.
    TooLong,
}

impl fmt::Display for PasswordPolicyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooShort => write!(
                f,
                "password must have at least {MIN_PASSWORD_CHARS} characters"
            ),
            Self::TooLong => write!(
                f,
                "password must have at most {MAX_PASSWORD_CHARS} characters"
            ),
        }
    }
}

impl std::error::Error for PasswordPolicyError {}

/// Checks a new password's length (in characters, not bytes).
///
/// # Errors
/// [`PasswordPolicyError`] if it is too short or too long.
pub fn check_new_password(password: &str) -> Result<(), PasswordPolicyError> {
    let n = password.chars().count();
    if n < MIN_PASSWORD_CHARS {
        Err(PasswordPolicyError::TooShort)
    } else if n > MAX_PASSWORD_CHARS {
        Err(PasswordPolicyError::TooLong)
    } else {
        Ok(())
    }
}

/// A user's subscription plan; it selects the API rate limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Plan {
    /// Default plan for new accounts.
    #[default]
    Free,
    /// Paid plan with higher limits.
    Pro,
}

impl Plan {
    /// Stable lowercase name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Free => "free",
            Self::Pro => "pro",
        }
    }

    /// Reads a name produced by [`Self::as_str`].
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "free" => Some(Self::Free),
            "pro" => Some(Self::Pro),
            _ => None,
        }
    }
}

/// Paid store memberships a user holds. Offers from a membership store are
/// hidden unless the user has that membership.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Memberships {
    /// Has a Costco membership.
    pub costco: bool,
    /// Has a Sam's Club membership.
    pub sams: bool,
}

impl Memberships {
    /// Whether an offer from `retailer` may be shown. Offers that need no
    /// membership always pass; a membership store other than Costco or
    /// Sam's Club is hidden (fails closed).
    #[must_use]
    pub fn allows(self, membership_required: bool, retailer: &RetailerId) -> bool {
        if !membership_required {
            return true;
        }
        match retailer.as_str() {
            COSTCO_RETAILER_ID => self.costco,
            SAMS_RETAILER_ID => self.sams,
            _ => false,
        }
    }
}

/// UI language. Spanish is the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Locale {
    /// Mexican Spanish.
    #[default]
    Es,
    /// English.
    En,
}

impl Locale {
    /// Stable lowercase code (`es`, `en`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Es => "es",
            Self::En => "en",
        }
    }

    /// Reads a code produced by [`Self::as_str`].
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "es" => Some(Self::Es),
            "en" => Some(Self::En),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_id_must_be_positive() {
        assert_eq!(UserId::new(7).map(UserId::get), Some(7));
        assert_eq!(UserId::new(0), None);
        assert_eq!(UserId::new(-1), None);
    }

    #[test]
    fn email_is_normalized() {
        let e = Email::parse("  Ana.Lopez@Ejemplo.COM.mx \n").unwrap();
        assert_eq!(e.as_str(), "ana.lopez@ejemplo.com.mx");
    }

    #[test]
    fn email_rejects_malformed() {
        for bad in [
            "",
            "   ",
            "sin-arroba",
            "@ejemplo.com",
            "a@",
            "a@b",
            "a@@b.com",
            "a@b@c.com",
            "a b@c.com",
            "a@.com",
            "a@b.com.",
            "a@b..com",
            "a\u{0}@b.com",
        ] {
            assert_eq!(Email::parse(bad), Err(EmailError), "{bad:?}");
        }
        let long = format!("{}@b.mx", "a".repeat(MAX_EMAIL_BYTES - 4));
        assert_eq!(long.len(), MAX_EMAIL_BYTES + 1);
        assert_eq!(Email::parse(&long), Err(EmailError));
        assert!(Email::parse(&long[1..]).is_ok());
    }

    #[test]
    fn email_error_does_not_echo() {
        assert!(!EmailError.to_string().contains('@'));
    }

    #[test]
    fn password_policy_counts_chars() {
        assert_eq!(
            check_new_password("ñññññññ"),
            Err(PasswordPolicyError::TooShort)
        );
        assert_eq!(check_new_password("ññññññññ"), Ok(()));
        assert_eq!(
            check_new_password(&"a".repeat(MAX_PASSWORD_CHARS + 1)),
            Err(PasswordPolicyError::TooLong)
        );
    }

    #[test]
    fn plan_and_locale_round_trip() {
        for p in [Plan::Free, Plan::Pro] {
            assert_eq!(Plan::parse(p.as_str()), Some(p));
        }
        for l in [Locale::Es, Locale::En] {
            assert_eq!(Locale::parse(l.as_str()), Some(l));
        }
        assert_eq!(Plan::default(), Plan::Free);
        assert_eq!(Locale::default(), Locale::Es);
        assert_eq!(Locale::parse("ES"), None);
    }

    #[test]
    fn memberships_gate_membership_stores() {
        let costco = RetailerId::new(COSTCO_RETAILER_ID).unwrap();
        let sams = RetailerId::new(SAMS_RETAILER_ID).unwrap();
        let other = RetailerId::new("club_x").unwrap();
        let none = Memberships::default();
        let only_costco = Memberships {
            costco: true,
            sams: false,
        };
        assert!(none.allows(false, &costco));
        assert!(!none.allows(true, &costco));
        assert!(only_costco.allows(true, &costco));
        assert!(!only_costco.allows(true, &sams));
        let both = Memberships {
            costco: true,
            sams: true,
        };
        assert!(!both.allows(true, &other));
        assert!(is_membership_retailer(&costco));
        assert!(is_membership_retailer(&sams));
        assert!(!is_membership_retailer(&other));
    }
}
