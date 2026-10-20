//! Server secrets, loaded once from the environment at start-up.

use std::ffi::OsString;

use http_guard::{Redactor, Secret, SecretError};

/// Every secret the worker reads, in log order.
const SECRET_VARS: [&str; 7] = [
    "BRIGHTDATA_API_KEY",
    "BRIGHTDATA_UNLOCKER_ZONE",
    "BRIGHTDATA_BROWSER_ZONE",
    "SPAPI_LWA_CLIENT_ID",
    "SPAPI_LWA_CLIENT_SECRET",
    "SPAPI_REFRESH_TOKEN",
    "MODEL_PROVIDER_API_KEY",
];

/// Provider credentials. Each is optional so the worker starts in
/// development; an adapter requires its own secrets when it is built.
pub struct ServerSecrets {
    brightdata_api_key: Option<Secret>,
    brightdata_unlocker_zone: Option<Secret>,
    brightdata_browser_zone: Option<Secret>,
    spapi_lwa_client_id: Option<Secret>,
    spapi_lwa_client_secret: Option<Secret>,
    spapi_refresh_token: Option<Secret>,
    model_provider_api_key: Option<Secret>,
}

impl ServerSecrets {
    /// Loads every secret from the process environment.
    pub fn from_env() -> Result<Self, SecretError> {
        Self::from_lookup(|name| std::env::var_os(name))
    }

    /// Loads every secret through `lookup`. Absent is `None`; a present but
    /// empty, malformed or non-UTF-8 value is an error naming the variable.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<OsString>) -> Result<Self, SecretError> {
        let [
            brightdata_api_key,
            brightdata_unlocker_zone,
            brightdata_browser_zone,
            spapi_lwa_client_id,
            spapi_lwa_client_secret,
            spapi_refresh_token,
            model_provider_api_key,
        ] = SECRET_VARS;
        let load = |name: &str| Secret::from_lookup(name, &lookup);
        Ok(Self {
            brightdata_api_key: load(brightdata_api_key)?,
            brightdata_unlocker_zone: load(brightdata_unlocker_zone)?,
            brightdata_browser_zone: load(brightdata_browser_zone)?,
            spapi_lwa_client_id: load(spapi_lwa_client_id)?,
            spapi_lwa_client_secret: load(spapi_lwa_client_secret)?,
            spapi_refresh_token: load(spapi_refresh_token)?,
            model_provider_api_key: load(model_provider_api_key)?,
        })
    }

    fn entries(&self) -> [(&'static str, Option<&Secret>); 7] {
        let [a, b, c, d, e, f, g] = SECRET_VARS;
        [
            (a, self.brightdata_api_key.as_ref()),
            (b, self.brightdata_unlocker_zone.as_ref()),
            (c, self.brightdata_browser_zone.as_ref()),
            (d, self.spapi_lwa_client_id.as_ref()),
            (e, self.spapi_lwa_client_secret.as_ref()),
            (f, self.spapi_refresh_token.as_ref()),
            (g, self.model_provider_api_key.as_ref()),
        ]
    }

    /// `BRIGHTDATA_API_KEY`, `BRIGHTDATA_UNLOCKER_ZONE` and the optional
    /// `BRIGHTDATA_BROWSER_ZONE`, for the client config.
    pub fn brightdata(&self) -> (Option<Secret>, Option<Secret>, Option<Secret>) {
        (
            self.brightdata_api_key.clone(),
            self.brightdata_unlocker_zone.clone(),
            self.brightdata_browser_zone.clone(),
        )
    }

    /// `SPAPI_LWA_CLIENT_ID`, `SPAPI_LWA_CLIENT_SECRET` and
    /// `SPAPI_REFRESH_TOKEN`, for `amazon::SpApiConfig::from_secrets`.
    pub fn spapi(&self) -> (Option<Secret>, Option<Secret>, Option<Secret>) {
        (
            self.spapi_lwa_client_id.clone(),
            self.spapi_lwa_client_secret.clone(),
            self.spapi_refresh_token.clone(),
        )
    }

    /// Which variables are set, in fixed order; safe to log.
    pub fn presence(&self) -> [(&'static str, bool); 7] {
        self.entries()
            .map(|(name, secret)| (name, secret.is_some()))
    }

    /// A redactor covering every loaded value.
    pub fn redactor(&self) -> Redactor {
        Redactor::new(self.entries().into_iter().filter_map(|(_, secret)| secret))
    }

    /// `base` (e.g. the database URL redactor) extended with every loaded
    /// value, for errors that may mention either.
    pub fn redactor_extending(&self, base: &Redactor) -> Redactor {
        self.entries()
            .into_iter()
            .filter_map(|(_, secret)| secret)
            .fold(base.clone(), |r, secret| r.with(secret))
    }
}

#[cfg(test)]
mod tests {
    use super::{SECRET_VARS, ServerSecrets};
    use std::collections::BTreeMap;
    use std::ffi::OsString;

    fn load(vars: &[(&str, &str)]) -> Result<ServerSecrets, http_guard::SecretError> {
        let map: BTreeMap<String, OsString> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), OsString::from(*v)))
            .collect();
        ServerSecrets::from_lookup(|name| map.get(name).cloned())
    }

    #[test]
    fn spapi_returns_the_three_lwa_values() {
        let secrets = load(&[
            ("SPAPI_LWA_CLIENT_ID", "id-1"),
            ("SPAPI_REFRESH_TOKEN", "refresh-1"),
        ])
        .unwrap();
        let (id, secret, refresh) = secrets.spapi();
        assert_eq!(id.unwrap().expose_secret(), "id-1");
        assert!(secret.is_none());
        assert_eq!(refresh.unwrap().expose_secret(), "refresh-1");
    }

    #[test]
    fn absent_variables_are_none() {
        let secrets = load(&[]).unwrap();
        assert!(secrets.presence().iter().all(|(_, set)| !set));
        assert_eq!(secrets.presence().map(|(name, _)| name), SECRET_VARS);
    }

    #[test]
    fn empty_variables_fail_without_leaking_values() {
        let error = load(&[
            ("BRIGHTDATA_API_KEY", "key-value-1"),
            ("SPAPI_REFRESH_TOKEN", ""),
        ])
        .err()
        .unwrap();
        let message = error.to_string();
        assert!(message.contains("SPAPI_REFRESH_TOKEN"), "{message}");
        assert!(!message.contains("key-value-1"), "{message}");
    }

    #[test]
    fn redactor_removes_every_loaded_value() {
        let values: Vec<(&str, String)> = SECRET_VARS
            .iter()
            .map(|name| (*name, format!("value-of-{name}")))
            .collect();
        let pairs: Vec<(&str, &str)> = values.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let secrets = load(&pairs).unwrap();
        assert!(secrets.presence().iter().all(|(_, set)| *set));
        let text = values
            .iter()
            .map(|(_, v)| v.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let redacted = secrets.redactor().redact(&text);
        assert_eq!(redacted, vec!["[redacted]"; SECRET_VARS.len()].join(" "));
    }
}
