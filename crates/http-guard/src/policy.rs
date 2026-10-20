//! Transport policy and the per-client host allowlist.

use std::collections::BTreeSet;
use std::net::IpAddr;

use crate::error::GuardError;

/// Network permissions for a [`crate::GuardedClient`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransportPolicy {
    /// Plain http, loopback IP literals, any port and private DNS answers.
    pub(crate) loopback_test: bool,
    /// `Some(port)`: an in-cluster service (ADR 0013). Plain http or https,
    /// exactly this port, allowlisted names whose DNS answers may be
    /// loopback or private but never link-local, multicast or unspecified.
    pub(crate) private_service_port: Option<u16>,
}

impl TransportPolicy {
    /// Production policy: https only, allowlisted domain names on default
    /// ports, private DNS answers dropped.
    pub const PUBLIC_HTTPS: Self = Self {
        loopback_test: false,
        private_service_port: None,
    };

    /// A private in-cluster service such as the assistant sidecar
    /// (ADR 0013): allows plain `http` (and `https`) to allowlisted DNS
    /// names on exactly `port`. DNS answers may be loopback or private
    /// (RFC 1918, unique-local) addresses, but link-local (including cloud
    /// metadata `169.254.169.254` and `fe80::/10`), multicast, broadcast and
    /// unspecified addresses are still dropped. IP literals stay refused;
    /// redirects, proxies, body caps and redaction are unchanged.
    #[must_use]
    pub const fn private_service(port: u16) -> Self {
        Self {
            loopback_test: false,
            private_service_port: Some(port),
        }
    }

    /// Test-only: allows plain http and any port, accepts loopback IP
    /// literals in addition to allowlisted names, and disables the
    /// public-only DNS resolver, so tests can hit fake servers. Requires the
    /// `test-support` feature, which may only be enabled from
    /// `[dev-dependencies]`.
    #[cfg(any(test, feature = "test-support"))]
    pub const LOOPBACK_TEST: Self = Self {
        loopback_test: true,
        private_service_port: None,
    };
}

impl Default for TransportPolicy {
    fn default() -> Self {
        Self::PUBLIC_HTTPS
    }
}

/// The DNS names a client may reach (exact match, no wildcards).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostAllowlist {
    hosts: BTreeSet<String>,
}

impl HostAllowlist {
    /// Builds an allowlist. Names are lowercased and a trailing dot is
    /// dropped.
    ///
    /// # Errors
    ///
    /// [`GuardError::InvalidAllowlist`] when the list is empty or a name is
    /// empty, an IP literal, a wildcard, carries a port or path, or is not
    /// an ASCII DNS name.
    pub fn new<I, S>(hosts: I) -> Result<Self, GuardError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let hosts = hosts
            .into_iter()
            .map(|host| validate_host(host.as_ref()))
            .collect::<Result<BTreeSet<_>, _>>()?;
        if hosts.is_empty() {
            return Err(GuardError::InvalidAllowlist(
                "the allowlist must name at least one host".to_owned(),
            ));
        }
        Ok(Self { hosts })
    }

    /// True when `host` (any case, optional trailing dot) is allowlisted.
    #[must_use]
    pub fn contains(&self, host: &str) -> bool {
        self.hosts.contains(&normalize(host))
    }

    /// The allowlisted names in sorted order.
    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.hosts.iter().map(String::as_str)
    }
}

fn normalize(host: &str) -> String {
    host.strip_suffix('.').unwrap_or(host).to_ascii_lowercase()
}

fn validate_host(raw: &str) -> Result<String, GuardError> {
    let invalid = |why: &str| GuardError::InvalidAllowlist(format!("{raw:?}: {why}"));
    let host = normalize(raw);
    if host.is_empty() {
        return Err(invalid("empty host name"));
    }
    if host.parse::<IpAddr>().is_ok() || host.starts_with('[') {
        return Err(invalid("IP literals are not allowed; use a DNS name"));
    }
    if host.contains('*') {
        return Err(invalid("wildcards are not allowed"));
    }
    if host.contains([':', '/', '@', '?', '#']) {
        return Err(invalid(
            "give a bare host name without scheme, port or path",
        ));
    }
    let labels_ok = host.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    });
    if !labels_ok || host.len() > 253 {
        return Err(invalid("not a valid ASCII DNS name (use punycode)"));
    }
    // URL parsers read an all-numeric last label as an IPv4 address.
    if host
        .rsplit('.')
        .next()
        .is_some_and(|tld| tld.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err(invalid(
            "numeric host names are IP addresses to URL parsers",
        ));
    }
    Ok(host)
}

#[cfg(test)]
mod tests {
    use super::HostAllowlist;

    #[test]
    fn normalizes_and_sorts() {
        let list = HostAllowlist::new(["API.Amazon.com.", "api.brightdata.com"]).unwrap();
        assert_eq!(
            list.iter().collect::<Vec<_>>(),
            ["api.amazon.com", "api.brightdata.com"]
        );
        assert!(list.contains("api.amazon.com."));
        assert!(list.contains("API.BRIGHTDATA.COM"));
        assert!(!list.contains("evil.api.brightdata.com"));
    }

    #[test]
    fn rejects_bad_entries() {
        for bad in [
            "",
            ".",
            "127.0.0.1",
            "::1",
            "[::1]",
            "*.amazon.com",
            "api.amazon.com:443",
            "https://api.amazon.com",
            "api.amazon.com/path",
            "user@api.amazon.com",
            "bad..name",
            "-bad.com",
            "café.mx",
            "127.1",
        ] {
            assert!(
                HostAllowlist::new([bad]).is_err(),
                "{bad:?} must be rejected"
            );
        }
        assert!(HostAllowlist::new(Vec::<String>::new()).is_err());
    }
}
