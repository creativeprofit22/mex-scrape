//! Postgres access for mex-scrape: connection pool and sqlx migrations.
//!
//! Owns every table in the database. Queries use runtime-checked
//! `sqlx::query` calls so builds never need a live database.

pub mod brightdata_runs;

use std::ffi::OsString;
use std::str::FromStr as _;

use secrets::{Redactor, Secret, SecretError};
use sqlx::migrate::Migrator;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

/// The connection pool type returned by [`connect`].
pub use sqlx::postgres::PgPool;

/// Embedded migrations from `crates/db/migrations`.
pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// Environment variable holding the Postgres connection URL.
pub const DATABASE_URL_ENV: &str = "DATABASE_URL";

/// The Postgres connection URL. It embeds the password, so it arrives as a
/// [`Secret`] and is kept only as parsed connection options; like `Secret`,
/// `DatabaseUrl` has no `Debug`, `Display` or `Serialize`. Its [`Redactor`] removes
/// the whole URL and the password (raw and percent-decoded) from any text,
/// so every error mentioning the database can be cleaned before it is logged.
///
/// The URL is validated when it is loaded: it must use the `postgres://` or
/// `postgresql://` scheme and parse as Postgres connection options.
#[derive(Clone)]
pub struct DatabaseUrl {
    // Holds the password; `DatabaseUrl` has no `Debug`, so it can't leak.
    options: PgConnectOptions,
    redactor: Redactor,
}

impl DatabaseUrl {
    /// Wraps a connection URL from a secret store.
    ///
    /// # Errors
    ///
    /// [`SecretError::Invalid`] if `url` is not a Postgres connection URL;
    /// the message never contains the value.
    pub fn new(url: Secret) -> Result<Self, SecretError> {
        Self::named("database URL", url)
    }

    fn named(name: &str, url: Secret) -> Result<Self, SecretError> {
        let options = parse_options(name, url.expose_secret())?;
        let mut redactor = Redactor::new([&url]);
        for candidate in password_candidates(url.expose_secret()) {
            // A candidate that is blank or has control characters can't be
            // matched in messages anyway; the full URL is still redacted.
            if let Ok(password) = Secret::new(candidate) {
                redactor = redactor.with(&password);
            }
        }
        Ok(Self { options, redactor })
    }

    /// Loads [`DATABASE_URL_ENV`] through `lookup` (e.g. `std::env::var_os`).
    /// Absent is `Ok(None)`; empty, malformed (not a Postgres connection
    /// URL) or non-UTF-8 is an error.
    ///
    /// # Errors
    ///
    /// Any [`SecretError`]; the message names the variable, never the value.
    pub fn from_lookup(
        lookup: impl FnOnce(&str) -> Option<OsString>,
    ) -> Result<Option<Self>, SecretError> {
        Secret::from_lookup(DATABASE_URL_ENV, lookup)?
            .map(|url| Self::named(DATABASE_URL_ENV, url))
            .transpose()
    }

    /// Loads [`DATABASE_URL_ENV`] from the process environment.
    ///
    /// # Errors
    ///
    /// As [`DatabaseUrl::from_lookup`].
    pub fn from_env() -> Result<Option<Self>, SecretError> {
        Self::from_lookup(|key| std::env::var_os(key))
    }

    /// Removes the URL and password from error text; apply it before
    /// logging or returning any database error.
    #[must_use]
    pub fn redactor(&self) -> &Redactor {
        &self.redactor
    }
}

/// Checks the scheme and parses `url` into connection options. Errors carry
/// fixed text only: sqlx's own message may echo parts of the URL.
fn parse_options(name: &str, url: &str) -> Result<PgConnectOptions, SecretError> {
    let invalid = |reason| SecretError::Invalid {
        name: name.to_owned(),
        reason,
    };
    let scheme = url.split_once("://").map(|(scheme, _)| scheme);
    if !scheme.is_some_and(|scheme| {
        scheme.eq_ignore_ascii_case("postgres") || scheme.eq_ignore_ascii_case("postgresql")
    }) {
        return Err(invalid("expected a postgres:// or postgresql:// URL"));
    }
    PgConnectOptions::from_str(url).map_err(|_| invalid("could not be parsed"))
}

/// Password values to redact: the userinfo password and any `password=`
/// query parameter, each raw and percent-decoded.
fn password_candidates(url: &str) -> Vec<String> {
    let mut raw = Vec::new();
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let (before_fragment, _) = rest.split_once('#').unwrap_or((rest, ""));
    let (before_query, query) = before_fragment
        .split_once('?')
        .unwrap_or((before_fragment, ""));
    // `/` and `@` must be percent-encoded inside userinfo, so the authority
    // ends at the first `/` and the userinfo at the last `@`.
    let authority = before_query.split('/').next().unwrap_or_default();
    if let Some((userinfo, _)) = authority.rsplit_once('@')
        && let Some((_, password)) = userinfo.split_once(':')
    {
        raw.push(password.to_owned());
    }
    for pair in query.split('&') {
        if let Some(("password", value)) = pair.split_once('=') {
            raw.push(value.to_owned());
        }
    }
    let mut candidates = Vec::new();
    for value in raw {
        if let Some(decoded) = percent_decode(&value)
            && decoded != value
        {
            candidates.push(decoded);
        }
        candidates.push(value);
    }
    candidates
}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let byte = bytes[i];
        if byte == b'%'
            && let Some(hex) = text.get(i + 1..i + 3)
            && let Ok(decoded) = u8::from_str_radix(hex, 16)
        {
            out.push(decoded);
            i += 3;
            continue;
        }
        out.push(byte);
        i += 1;
    }
    String::from_utf8(out).ok()
}

/// Opens a Postgres connection pool.
///
/// # Errors
///
/// Returns an error if the database cannot be reached. Pass its message
/// through [`DatabaseUrl::redactor`] before logging it.
pub async fn connect(url: &DatabaseUrl) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(5)
        .connect_with(url.options.clone())
        .await
}

/// Applies all pending migrations to `pool`.
///
/// # Errors
/// Returns an error if a migration fails or the applied history diverges.
pub async fn migrate(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    MIGRATOR.run(pool).await
}

#[cfg(test)]
mod tests {
    use super::{DatabaseUrl, password_candidates};
    use secrets::Secret;
    use std::ffi::OsString;

    fn load(value: &str) -> DatabaseUrl {
        DatabaseUrl::from_lookup(|_| Some(OsString::from(value)))
            .unwrap()
            .unwrap()
    }

    #[test]
    fn absent_is_none_and_empty_fails_without_the_value() {
        assert!(DatabaseUrl::from_lookup(|_| None).unwrap().is_none());
        let error = DatabaseUrl::from_lookup(|_| Some(OsString::new()))
            .err()
            .unwrap();
        assert_eq!(
            error.to_string(),
            "DATABASE_URL is empty or only whitespace"
        );
    }

    #[test]
    fn malformed_urls_fail_at_load_without_the_value() {
        for value in [
            "notaurl://mex:pw-123@@@",
            "notaurl://mex:pw-123@db/app",
            "postgres://mex:pw-123@@@",
            "pw-123",
        ] {
            let error = DatabaseUrl::from_lookup(|_| Some(value.into()))
                .err()
                .unwrap()
                .to_string();
            assert!(error.contains("DATABASE_URL"), "{error}");
            assert!(!error.contains("pw-123"), "{error}");
        }
        let error = DatabaseUrl::new(Secret::new("x://pw-123".to_owned()).unwrap())
            .err()
            .unwrap()
            .to_string();
        assert!(!error.contains("pw-123"), "{error}");
        assert!(DatabaseUrl::new(Secret::new("postgresql://db/app".to_owned()).unwrap()).is_ok());
    }

    #[test]
    fn finds_userinfo_and_query_passwords() {
        assert_eq!(
            password_candidates("postgres://mex:p%40ss%2Fw@db:5432/app?sslmode=require"),
            ["p@ss/w", "p%40ss%2Fw"]
        );
        assert_eq!(
            password_candidates("postgres://db/app?user=mex&password=hunter2"),
            ["hunter2"]
        );
        assert!(password_candidates("postgres://mex@db/app").is_empty());
        assert!(password_candidates("postgres://db/app").is_empty());
    }

    #[test]
    fn redactor_removes_url_and_password_forms() {
        let url = load("postgres://mex:p%40ss-9@127.0.0.1:5432/mex_scrape");
        let message = url.redactor().sanitize(
            "connect postgres://mex:p%40ss-9@127.0.0.1:5432/mex_scrape failed; \
             auth p@ss-9 / p%40ss-9 rejected",
        );
        assert_eq!(
            message,
            "connect [redacted] failed; auth [redacted] / [redacted] rejected"
        );
    }
}
