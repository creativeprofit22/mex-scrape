//! Server-side secrets and the redaction applied to every error string.
//!
//! Pure (no I/O beyond reading the process environment, no network), so
//! any crate can hold a [`Secret`] without depending on the HTTP client.
//! `http-guard` re-exports everything here.

use std::ffi::OsString;
use std::fmt;
use std::sync::Arc;

/// Untrusted remote error text is cut to this many characters.
pub const MAX_ERROR_MESSAGE_CHARS: usize = 500;

/// Replacement text for a redacted secret value.
const REDACTED: &str = "[redacted]";

/// A secret value (API key, client secret, refresh token, zone name).
///
/// `Secret` deliberately implements no `Debug`, `Display`, `Serialize` or
/// `Default`, so it cannot be printed, logged or serialized into an API
/// response by accident. It has no sqlx `Encode` impl either (and the orphan
/// rule stops other crates adding one), so it cannot be bound into a DB row.
/// Send it only through `http_guard::GuardedRequest::secret_header` or
/// `http_guard::GuardedRequest::bearer`; reach for [`Secret::expose_secret`]
/// only when a provider requires the value elsewhere (e.g. a request body
/// or a database connection string).
///
/// ```
/// let key = secrets::Secret::new("k-123".to_owned())?;
/// assert_eq!(key.expose_secret(), "k-123");
/// # Ok::<(), secrets::SecretError>(())
/// ```
///
/// It cannot be Debug-formatted:
///
/// ```compile_fail
/// let key = secrets::Secret::new("k-123".to_owned()).unwrap();
/// let _ = format!("{key:?}");
/// ```
///
/// nor Display-formatted:
///
/// ```compile_fail
/// let key = secrets::Secret::new("k-123".to_owned()).unwrap();
/// let _ = format!("{key}");
/// ```
///
/// nor serialized:
///
/// ```compile_fail
/// let key = secrets::Secret::new("k-123".to_owned()).unwrap();
/// let _ = serde_json::to_string(&key);
/// ```
#[derive(Clone)]
pub struct Secret(Box<str>);

/// Why a secret could not be loaded. Messages name the variable, never the
/// value.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SecretError {
    /// The value is empty or only whitespace.
    #[error("{name} is empty or only whitespace")]
    Empty {
        /// Variable (or source) name.
        name: String,
    },
    /// The value contains control characters (newlines, NUL, ...).
    #[error("{name} contains control characters")]
    ControlCharacters {
        /// Variable (or source) name.
        name: String,
    },
    /// The environment variable is not valid UTF-8.
    #[error("{name} is not valid UTF-8")]
    NotUnicode {
        /// Variable name.
        name: String,
    },
    /// The value is not in the format its consumer requires (e.g. not a
    /// connection URL). `reason` is fixed text, never derived from the value.
    #[error("{name} is not a valid connection URL: {reason}")]
    Invalid {
        /// Variable (or source) name.
        name: String,
        /// Fixed description of what is wrong.
        reason: &'static str,
    },
}

impl Secret {
    /// Wraps a value from a secret store. Empty, whitespace-only and
    /// control-character values are rejected so loading fails closed.
    ///
    /// # Errors
    ///
    /// [`SecretError::Empty`] or [`SecretError::ControlCharacters`].
    pub fn new(value: String) -> Result<Self, SecretError> {
        Self::named("secret value", value)
    }

    fn named(name: &str, value: String) -> Result<Self, SecretError> {
        if value.trim().is_empty() {
            return Err(SecretError::Empty {
                name: name.to_owned(),
            });
        }
        if value.chars().any(char::is_control) {
            return Err(SecretError::ControlCharacters {
                name: name.to_owned(),
            });
        }
        Ok(Self(value.into_boxed_str()))
    }

    /// Loads `name` through `lookup` (e.g. `std::env::var_os`). An absent
    /// variable is `Ok(None)`; a present but empty, malformed or non-UTF-8
    /// one is an error.
    ///
    /// # Errors
    ///
    /// Any [`SecretError`]; the message names `name`, never the value.
    pub fn from_lookup(
        name: &str,
        lookup: impl FnOnce(&str) -> Option<OsString>,
    ) -> Result<Option<Self>, SecretError> {
        let Some(raw) = lookup(name) else {
            return Ok(None);
        };
        let value = raw.into_string().map_err(|_| SecretError::NotUnicode {
            name: name.to_owned(),
        })?;
        Self::named(name, value).map(Some)
    }

    /// Loads `name` from the process environment; see [`Secret::from_lookup`].
    ///
    /// # Errors
    ///
    /// Any [`SecretError`]; the message names `name`, never the value.
    pub fn from_env(name: &str) -> Result<Option<Self>, SecretError> {
        Self::from_lookup(name, |key| std::env::var_os(key))
    }

    /// The raw value. Never log or store it.
    #[must_use]
    pub fn expose_secret(&self) -> &str {
        &self.0
    }
}

/// Removes known secret values from text before it is logged or returned.
///
/// Cheap to clone (shared list). Longer secrets are replaced first, so one
/// secret that contains another is fully removed.
#[derive(Clone, Default)]
pub struct Redactor {
    secrets: Arc<[Box<str>]>,
}

impl Redactor {
    /// Builds a redactor for `secrets`.
    pub fn new<'a>(secrets: impl IntoIterator<Item = &'a Secret>) -> Self {
        Self::from_values(secrets.into_iter().map(|s| s.0.clone()).collect())
    }

    /// This redactor plus `secret`.
    #[must_use]
    pub fn with(&self, secret: &Secret) -> Self {
        if self.secrets.contains(&secret.0) {
            return self.clone();
        }
        let mut values = self.secrets.to_vec();
        values.push(secret.0.clone());
        Self::from_values(values)
    }

    fn from_values(mut values: Vec<Box<str>>) -> Self {
        values.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        values.dedup();
        Self {
            secrets: values.into(),
        }
    }

    /// Replaces every known secret value in `text` with `[redacted]`.
    #[must_use]
    pub fn redact(&self, text: &str) -> String {
        let mut out = text.to_owned();
        for secret in self.secrets.iter() {
            if out.contains(&**secret) {
                out = out.replace(&**secret, REDACTED);
            }
        }
        out
    }

    /// Redacts, then truncates, so truncation can never leave part of a
    /// secret behind.
    #[must_use]
    pub fn sanitize(&self, text: &str) -> String {
        truncate_error_message(&self.redact(text))
    }
}

impl fmt::Debug for Redactor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Redactor")
            .field("secrets", &self.secrets.len())
            .finish()
    }
}

/// Truncates untrusted remote error text to [`MAX_ERROR_MESSAGE_CHARS`]
/// characters (plus `…`) before it is shown or stored.
#[must_use]
pub fn truncate_error_message(message: &str) -> String {
    let mut characters = message.chars();
    let truncated: String = characters.by_ref().take(MAX_ERROR_MESSAGE_CHARS).collect();
    if characters.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_ERROR_MESSAGE_CHARS, Redactor, Secret, SecretError, truncate_error_message};
    use std::ffi::OsString;

    fn secret(value: &str) -> Secret {
        Secret::new(value.to_owned()).unwrap()
    }

    #[test]
    fn with_adds_a_secret_and_keeps_longest_first() {
        let short = secret("abc");
        let long = secret("abcdef-long");
        let redactor = Redactor::new([&short]).with(&long).with(&short);
        assert_eq!(
            redactor.redact("x abcdef-long abc"),
            "x [redacted] [redacted]"
        );
        assert_eq!(format!("{redactor:?}"), "Redactor { secrets: 2 }");
    }

    #[test]
    fn rejects_blank_and_control_values() {
        assert!(matches!(
            Secret::new(String::new()),
            Err(SecretError::Empty { .. })
        ));
        assert!(matches!(
            Secret::new("  \t".to_owned()),
            Err(SecretError::Empty { .. })
        ));
        assert!(matches!(
            Secret::new("abc\ndef".to_owned()),
            Err(SecretError::ControlCharacters { .. })
        ));
    }

    #[test]
    fn lookup_absent_is_none_and_empty_is_an_error() {
        assert!(Secret::from_lookup("KEY", |_| None).unwrap().is_none());
        let error = Secret::from_lookup("KEY", |_| Some(OsString::new()))
            .err()
            .unwrap();
        assert_eq!(error.to_string(), "KEY is empty or only whitespace");
        let loaded = Secret::from_lookup("KEY", |_| Some("v-1".into()))
            .unwrap()
            .unwrap();
        assert_eq!(loaded.expose_secret(), "v-1");
    }

    #[test]
    fn errors_never_contain_the_value() {
        let error = Secret::from_lookup("KEY", |_| Some("super-secret\u{7}".into()))
            .err()
            .unwrap();
        assert!(!error.to_string().contains("super-secret"), "{error}");
        assert!(error.to_string().contains("KEY"));
    }

    #[cfg(windows)]
    fn not_unicode() -> OsString {
        use std::os::windows::ffi::OsStringExt as _;
        OsString::from_wide(&[0xD800])
    }

    #[cfg(unix)]
    fn not_unicode() -> OsString {
        use std::os::unix::ffi::OsStringExt as _;
        OsString::from_vec(vec![0xff])
    }

    #[test]
    fn non_unicode_values_are_rejected() {
        let error = Secret::from_lookup("KEY", |_| Some(not_unicode()))
            .err()
            .unwrap();
        assert_eq!(
            error,
            SecretError::NotUnicode {
                name: "KEY".to_owned()
            }
        );
    }

    #[test]
    fn redacts_overlapping_secrets_longest_first() {
        let short = secret("abc");
        let long = secret("abcdef");
        let redactor = Redactor::new([&short, &long]);
        assert_eq!(
            redactor.redact("key=abcdef other=abc"),
            "key=[redacted] other=[redacted]"
        );
        assert_eq!(format!("{redactor:?}"), "Redactor { secrets: 2 }");
    }

    #[test]
    fn redacts_before_truncating() {
        let key = secret("SECRETKEY-0123456789");
        let redactor = Redactor::new([&key]);
        // The secret straddles the truncation boundary.
        let text = format!(
            "{}{}",
            "e".repeat(MAX_ERROR_MESSAGE_CHARS - 5),
            key.expose_secret()
        );
        let sanitized = redactor.sanitize(&text);
        assert!(!sanitized.contains("SECRE"), "{sanitized}");
        assert!(sanitized.ends_with('…'));
    }

    #[test]
    fn truncates_long_error_messages() {
        let long = "e".repeat(MAX_ERROR_MESSAGE_CHARS + 50);
        let truncated = truncate_error_message(&long);
        assert_eq!(truncated.chars().count(), MAX_ERROR_MESSAGE_CHARS + 1);
        assert!(truncated.ends_with('…'));
        assert_eq!(truncate_error_message("short"), "short");
    }
}
