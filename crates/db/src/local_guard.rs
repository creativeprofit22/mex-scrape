//! The local-only database guard shared by the seed binaries and the
//! worker's demo mode: they write fixture data (a known password, fake
//! prices into the append-only `price_observations`), so they refuse a
//! `DATABASE_URL` that is not on a loopback host.

use std::ffi::OsStr;

use crate::DatabaseUrl;

/// Opt-in for `seed_e2e` to seed a database that is not on a loopback host.
pub const ALLOW_REMOTE_ENV: &str = "E2E_ALLOW_REMOTE_DB";

/// Whether `url` points at a loopback TCP host (`localhost`, `127.0.0.1` or
/// `::1`). A Unix socket is not accepted: it may be a real server's local
/// socket.
#[must_use]
pub fn is_local_target(url: &DatabaseUrl) -> bool {
    url.host().is_some_and(|host| {
        let host = host
            .strip_prefix('[')
            .and_then(|h| h.strip_suffix(']'))
            .unwrap_or(host);
        host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1"
    })
}

/// Why a seed refused its target. The message names only variables, never
/// the URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LocalTargetError {
    /// Not a loopback host and no `E2E_ALLOW_REMOTE_DB=true`.
    #[error(
        "refusing to seed: the DATABASE_URL host is not localhost, 127.0.0.1 or ::1 (set E2E_ALLOW_REMOTE_DB=true to seed a non-local test database)"
    )]
    RemoteWithoutOptIn,
    /// Not a loopback host; this caller has no opt-in.
    #[error("refusing to run: the DATABASE_URL host is not localhost, 127.0.0.1 or ::1")]
    Remote,
}

/// Refuses non-local targets unless `allow_remote` (the value of
/// [`ALLOW_REMOTE_ENV`]) is exactly `true` (used by `seed_e2e`).
///
/// # Errors
///
/// [`LocalTargetError::RemoteWithoutOptIn`].
pub fn check_target(
    url: &DatabaseUrl,
    allow_remote: Option<&OsStr>,
) -> Result<(), LocalTargetError> {
    if is_local_target(url) || allow_remote == Some(OsStr::new("true")) {
        return Ok(());
    }
    Err(LocalTargetError::RemoteWithoutOptIn)
}

/// Refuses every non-local target, with no opt-in (demo data must never
/// reach a shared database).
///
/// # Errors
///
/// [`LocalTargetError::Remote`].
pub fn require_local(url: &DatabaseUrl) -> Result<(), LocalTargetError> {
    if is_local_target(url) {
        Ok(())
    } else {
        Err(LocalTargetError::Remote)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrets::Secret;

    fn url(value: &str) -> DatabaseUrl {
        DatabaseUrl::new(Secret::new(value.to_owned()).unwrap()).unwrap()
    }

    #[test]
    fn local_hosts_pass() {
        for value in [
            "postgres://x:y@127.0.0.1:5432/mex_scrape",
            "postgres://x:y@localhost/mex_scrape",
            "postgresql://x:y@LOCALHOST:5432/mex_scrape",
            "postgres://x:y@[::1]:5432/mex_scrape",
        ] {
            assert!(is_local_target(&url(value)), "{value}");
            assert!(check_target(&url(value), None).is_ok(), "{value}");
            assert!(require_local(&url(value)).is_ok(), "{value}");
        }
    }

    #[test]
    fn remote_hosts_fail_without_leaking_the_url() {
        for value in [
            "postgres://x:y@db.example.com/z",
            "postgres://x:y@10.0.0.5:5432/z",
            "postgres://x:y@127.0.0.1.example.com/z",
            "postgres://x:y@127.0.0.1/z?host=db.example.com",
            "postgres://x:y@localhost/z?host=/var/run/postgresql",
        ] {
            assert!(!is_local_target(&url(value)), "{value}");
            for allow in [None, Some(OsStr::new("1")), Some(OsStr::new("TRUE"))] {
                let error = check_target(&url(value), allow).unwrap_err().to_string();
                assert!(error.contains(ALLOW_REMOTE_ENV));
                assert!(!error.contains("example.com") && !error.contains("x:y"));
            }
            let error = require_local(&url(value)).unwrap_err().to_string();
            assert!(!error.contains("example.com") && !error.contains("x:y"));
        }
    }

    #[test]
    fn remote_host_with_opt_in_passes_only_where_allowed() {
        let remote = url("postgres://x:y@db.example.com/z");
        assert!(check_target(&remote, Some(OsStr::new("true"))).is_ok());
        assert!(require_local(&remote).is_err());
    }
}
