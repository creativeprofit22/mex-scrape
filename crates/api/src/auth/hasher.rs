//! Argon2 password hashing (`password-auth`), off the async runtime.

use app::{HashError, PasswordHasher, PortFuture};

/// Argon2id hashes in PHC format. Hashing and verifying run on the
/// blocking pool so a slow hash never stalls request handling.
pub struct Argon2Hasher {
    dummy: String,
}

impl Argon2Hasher {
    /// A hasher with a fresh dummy hash (of a random secret), used to keep
    /// sign-in timing the same for unknown e-mail addresses.
    #[must_use]
    pub fn new() -> Self {
        let secret = format!(
            "{}{}",
            tower_sessions::session::Id::default(),
            tower_sessions::session::Id::default()
        );
        Self {
            dummy: password_auth::generate_hash(secret),
        }
    }
}

impl Default for Argon2Hasher {
    fn default() -> Self {
        Self::new()
    }
}

impl PasswordHasher for Argon2Hasher {
    fn hash<'a>(&'a self, password: &'a str) -> PortFuture<'a, Result<String, HashError>> {
        let password = password.to_owned();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || password_auth::generate_hash(password))
                .await
                .map_err(|_| HashError)
        })
    }

    fn verify<'a>(&'a self, password: &'a str, hash: &'a str) -> PortFuture<'a, bool> {
        let password = password.to_owned();
        let hash = hash.to_owned();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                password_auth::verify_password(password, &hash).is_ok()
            })
            .await
            .unwrap_or(false)
        })
    }

    fn dummy_hash(&self) -> &str {
        &self.dummy
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn hashes_verify_and_malformed_hashes_never_match() {
        let h = Argon2Hasher::new();
        let hash = h.hash("contraseña1").await.unwrap();
        assert!(hash.starts_with("$argon2"));
        assert!(h.verify("contraseña1", &hash).await);
        assert!(!h.verify("contraseña2", &hash).await);
        assert!(!h.verify("contraseña1", "not-a-hash").await);
        assert!(!h.verify("contraseña1", h.dummy_hash()).await);
    }
}
