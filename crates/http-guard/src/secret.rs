//! Sending [`Secret`] values as headers.

use reqwest::header::{HeaderValue, InvalidHeaderValue};
use secrets::Secret;

/// `prefix` + the secret as a header value marked sensitive (hidden from
/// `Debug` output and HPACK indexing).
pub(crate) fn header_value(
    secret: &Secret,
    prefix: &str,
) -> Result<HeaderValue, InvalidHeaderValue> {
    let mut value = HeaderValue::from_str(&format!("{prefix}{}", secret.expose_secret()))?;
    value.set_sensitive(true);
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::header_value;
    use secrets::Secret;

    #[test]
    fn header_values_are_sensitive() {
        let secret = Secret::new("abc".to_owned()).unwrap();
        let value = header_value(&secret, "Bearer ").unwrap();
        assert!(value.is_sensitive());
        assert_eq!(value.to_str().unwrap(), "Bearer abc");
    }
}
