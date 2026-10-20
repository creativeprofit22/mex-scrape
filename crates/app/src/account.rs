//! Use cases: sign up, sign in, read the profile, finish onboarding and set
//! the UI language (ADR 0012). Sessions and cookies live in `api`; these
//! only check credentials and read or write accounts.

use std::sync::Arc;

use domain::{
    Email, Locale, MAX_PASSWORD_CHARS, Memberships, PasswordPolicyError, UserId, check_new_password,
};

use crate::ports::{CreateUserError, PasswordHasher, UserRecord, UserRepo};

/// Why an account could not be created.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SignUpError {
    /// The e-mail address is malformed.
    #[error("invalid e-mail address")]
    InvalidEmail,
    /// The password breaks the length policy.
    #[error("{0}")]
    WeakPassword(PasswordPolicyError),
    /// The address is already registered.
    #[error("e-mail address already registered")]
    EmailTaken,
    /// Storage or hashing failed.
    #[error("account could not be created")]
    Unavailable,
}

/// Creates accounts.
pub struct SignUp {
    users: Arc<dyn UserRepo>,
    hasher: Arc<dyn PasswordHasher>,
}

impl SignUp {
    /// Creates accounts in `users`, hashing passwords with `hasher`.
    #[must_use]
    pub fn new(users: Arc<dyn UserRepo>, hasher: Arc<dyn PasswordHasher>) -> Self {
        Self { users, hasher }
    }

    /// Validates the input, hashes the password and stores a new `free`
    /// account using `locale`.
    ///
    /// # Errors
    /// [`SignUpError::InvalidEmail`] / [`SignUpError::WeakPassword`] before
    /// anything is hashed or stored; [`SignUpError::EmailTaken`] if the
    /// address is registered; [`SignUpError::Unavailable`] otherwise.
    pub async fn sign_up(
        &self,
        email: &str,
        password: &str,
        locale: Locale,
    ) -> Result<UserRecord, SignUpError> {
        let email = Email::parse(email).map_err(|_| SignUpError::InvalidEmail)?;
        check_new_password(password).map_err(SignUpError::WeakPassword)?;
        let hash = self
            .hasher
            .hash(password)
            .await
            .map_err(|_| SignUpError::Unavailable)?;
        self.users
            .create(&email, &hash, locale)
            .await
            .map_err(|e| match e {
                CreateUserError::EmailTaken => SignUpError::EmailTaken,
                CreateUserError::Unavailable => SignUpError::Unavailable,
            })
    }
}

/// Why sign-in failed. Never says whether the address exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SignInError {
    /// Unknown address or wrong password.
    #[error("invalid e-mail or password")]
    InvalidCredentials,
    /// Storage could not be read.
    #[error("sign-in unavailable")]
    Unavailable,
}

/// Checks e-mail and password.
pub struct SignIn {
    users: Arc<dyn UserRepo>,
    hasher: Arc<dyn PasswordHasher>,
}

impl SignIn {
    /// Checks credentials stored in `users` with `hasher`.
    #[must_use]
    pub fn new(users: Arc<dyn UserRepo>, hasher: Arc<dyn PasswordHasher>) -> Self {
        Self { users, hasher }
    }

    /// The account whose address and password match.
    ///
    /// An unknown or malformed address is verified against
    /// [`PasswordHasher::dummy_hash`], so it takes as long as a wrong
    /// password.
    ///
    /// # Errors
    /// [`SignInError::InvalidCredentials`] if they do not match;
    /// [`SignInError::Unavailable`] if storage fails.
    pub async fn sign_in(&self, email: &str, password: &str) -> Result<UserRecord, SignInError> {
        if password.chars().count() > MAX_PASSWORD_CHARS {
            // No account accepts it; refuse before spending hashing time.
            return Err(SignInError::InvalidCredentials);
        }
        let found = match Email::parse(email) {
            Ok(email) => self
                .users
                .find_credentials(&email)
                .await
                .map_err(|_| SignInError::Unavailable)?,
            Err(_) => None,
        };
        match found {
            Some(c) if self.hasher.verify(password, &c.password_hash).await => Ok(c.user),
            Some(_) => Err(SignInError::InvalidCredentials),
            None => {
                // Same work as a wrong password; the result is ignored.
                let _ = self.hasher.verify(password, self.hasher.dummy_hash()).await;
                Err(SignInError::InvalidCredentials)
            }
        }
    }
}

/// Why a profile could not be read or changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ProfileError {
    /// The account no longer exists.
    #[error("account not found")]
    NotFound,
    /// Storage failed.
    #[error("profile unavailable")]
    Unavailable,
}

/// Reads the signed-in user's account and sets their UI language.
pub struct Profile {
    users: Arc<dyn UserRepo>,
}

impl Profile {
    /// Reads accounts from `users`.
    #[must_use]
    pub fn new(users: Arc<dyn UserRepo>) -> Self {
        Self { users }
    }

    /// The account `id`.
    ///
    /// # Errors
    /// [`ProfileError::NotFound`] if it is gone; [`ProfileError::Unavailable`]
    /// if storage fails.
    pub async fn get(&self, id: UserId) -> Result<UserRecord, ProfileError> {
        found(self.users.find_by_id(id).await)
    }

    /// Stores `locale` as the preferred language of `id`.
    ///
    /// # Errors
    /// As [`Self::get`].
    pub async fn set_locale(&self, id: UserId, locale: Locale) -> Result<UserRecord, ProfileError> {
        found(self.users.set_locale(id, locale).await)
    }
}

/// Stores the memberships chosen during onboarding.
pub struct UpdateOnboarding {
    users: Arc<dyn UserRepo>,
}

impl UpdateOnboarding {
    /// Writes accounts in `users`.
    #[must_use]
    pub fn new(users: Arc<dyn UserRepo>) -> Self {
        Self { users }
    }

    /// Stores `memberships` for `id` and marks onboarding done.
    ///
    /// # Errors
    /// As [`Profile::get`].
    pub async fn set(
        &self,
        id: UserId,
        memberships: Memberships,
    ) -> Result<UserRecord, ProfileError> {
        found(self.users.set_memberships(id, memberships).await)
    }
}

fn found(
    r: Result<Option<UserRecord>, crate::ports::RepoError>,
) -> Result<UserRecord, ProfileError> {
    r.map_err(|_| ProfileError::Unavailable)?
        .ok_or(ProfileError::NotFound)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use domain::{Plan, TenantId};

    use super::*;
    use crate::ports::{HashError, PortFuture, RepoError, UserCredentials};

    /// Reversible "hash" that counts verifications.
    #[derive(Default)]
    struct FakeHasher {
        verifies: AtomicUsize,
        fail: bool,
    }

    impl PasswordHasher for FakeHasher {
        fn hash<'a>(&'a self, password: &'a str) -> PortFuture<'a, Result<String, HashError>> {
            Box::pin(async move {
                if self.fail {
                    Err(HashError)
                } else {
                    Ok(format!("h:{password}"))
                }
            })
        }

        fn verify<'a>(&'a self, password: &'a str, hash: &'a str) -> PortFuture<'a, bool> {
            Box::pin(async move {
                self.verifies.fetch_add(1, Ordering::SeqCst);
                hash.strip_prefix("h:") == Some(password)
            })
        }

        fn dummy_hash(&self) -> &str {
            "dummy"
        }
    }

    #[derive(Default)]
    struct FakeUsers {
        rows: Mutex<Vec<UserCredentials>>,
        fail: bool,
    }

    impl FakeUsers {
        fn update(
            &self,
            id: UserId,
            f: impl FnOnce(&mut UserRecord),
        ) -> Result<Option<UserRecord>, RepoError> {
            if self.fail {
                return Err(RepoError::Unavailable);
            }
            let mut rows = self.rows.lock().unwrap();
            Ok(rows.iter_mut().find(|c| c.user.id == id).map(|c| {
                f(&mut c.user);
                c.user.clone()
            }))
        }
    }

    impl UserRepo for FakeUsers {
        fn create<'a>(
            &'a self,
            email: &'a Email,
            password_hash: &'a str,
            locale: Locale,
        ) -> PortFuture<'a, Result<UserRecord, CreateUserError>> {
            Box::pin(async move {
                if self.fail {
                    return Err(CreateUserError::Unavailable);
                }
                let mut rows = self.rows.lock().unwrap();
                if rows.iter().any(|c| &c.user.email == email) {
                    return Err(CreateUserError::EmailTaken);
                }
                let id = UserId::new(i64::try_from(rows.len()).unwrap() + 1).unwrap();
                let user = UserRecord {
                    id,
                    email: email.clone(),
                    tenant: TenantId::for_user(id),
                    plan: Plan::Free,
                    memberships: Memberships::default(),
                    onboarded: false,
                    locale,
                };
                rows.push(UserCredentials {
                    user: user.clone(),
                    password_hash: password_hash.to_owned(),
                });
                Ok(user)
            })
        }

        fn find_credentials<'a>(
            &'a self,
            email: &'a Email,
        ) -> PortFuture<'a, Result<Option<UserCredentials>, RepoError>> {
            Box::pin(async move {
                if self.fail {
                    return Err(RepoError::Unavailable);
                }
                let rows = self.rows.lock().unwrap();
                Ok(rows.iter().find(|c| &c.user.email == email).cloned())
            })
        }

        fn find_by_id(&self, id: UserId) -> PortFuture<'_, Result<Option<UserRecord>, RepoError>> {
            Box::pin(async move { self.update(id, |_| {}) })
        }

        fn set_memberships(
            &self,
            id: UserId,
            memberships: Memberships,
        ) -> PortFuture<'_, Result<Option<UserRecord>, RepoError>> {
            Box::pin(async move {
                self.update(id, |u| {
                    u.memberships = memberships;
                    u.onboarded = true;
                })
            })
        }

        fn set_locale(
            &self,
            id: UserId,
            locale: Locale,
        ) -> PortFuture<'_, Result<Option<UserRecord>, RepoError>> {
            Box::pin(async move { self.update(id, |u| u.locale = locale) })
        }
    }

    fn setup() -> (Arc<FakeUsers>, Arc<FakeHasher>, SignUp, SignIn) {
        let users = Arc::new(FakeUsers::default());
        let hasher = Arc::new(FakeHasher::default());
        let up = SignUp::new(users.clone(), hasher.clone());
        let inn = SignIn::new(users.clone(), hasher.clone());
        (users, hasher, up, inn)
    }

    #[tokio::test]
    async fn sign_up_normalizes_and_hashes() {
        let (users, _, up, _) = setup();
        let u = up
            .sign_up(" Ana@Ejemplo.MX ", "contraseña1", Locale::En)
            .await
            .unwrap();
        assert_eq!(u.email.as_str(), "ana@ejemplo.mx");
        assert_eq!(u.tenant.as_str(), "user-1");
        assert_eq!(
            (u.plan, u.onboarded, u.locale),
            (Plan::Free, false, Locale::En)
        );
        let stored = users.rows.lock().unwrap()[0].password_hash.clone();
        assert_eq!(stored, "h:contraseña1");
        assert_eq!(
            up.sign_up("ana@ejemplo.mx", "otraclave1", Locale::Es).await,
            Err(SignUpError::EmailTaken)
        );
    }

    #[tokio::test]
    async fn sign_up_rejects_bad_input_before_storing() {
        let (users, _, up, _) = setup();
        assert_eq!(
            up.sign_up("no-es-correo", "contraseña1", Locale::Es).await,
            Err(SignUpError::InvalidEmail)
        );
        assert_eq!(
            up.sign_up("ana@ejemplo.mx", "corta", Locale::Es).await,
            Err(SignUpError::WeakPassword(PasswordPolicyError::TooShort))
        );
        assert!(users.rows.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn sign_up_hash_failure_is_unavailable() {
        let users = Arc::new(FakeUsers::default());
        let hasher = Arc::new(FakeHasher {
            fail: true,
            ..FakeHasher::default()
        });
        let up = SignUp::new(users.clone(), hasher);
        assert_eq!(
            up.sign_up("ana@ejemplo.mx", "contraseña1", Locale::Es)
                .await,
            Err(SignUpError::Unavailable)
        );
        assert!(users.rows.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn sign_in_checks_password_and_hides_unknown_addresses() {
        let (_, hasher, up, inn) = setup();
        let u = up
            .sign_up("ana@ejemplo.mx", "contraseña1", Locale::Es)
            .await
            .unwrap();
        assert_eq!(inn.sign_in("ANA@ejemplo.mx", "contraseña1").await, Ok(u));
        assert_eq!(
            inn.sign_in("ana@ejemplo.mx", "equivocada").await,
            Err(SignInError::InvalidCredentials)
        );
        let before = hasher.verifies.load(Ordering::SeqCst);
        assert_eq!(
            inn.sign_in("nadie@ejemplo.mx", "contraseña1").await,
            Err(SignInError::InvalidCredentials)
        );
        assert_eq!(
            inn.sign_in("malformado", "contraseña1").await,
            Err(SignInError::InvalidCredentials)
        );
        // Unknown and malformed addresses still run one verification each.
        assert_eq!(hasher.verifies.load(Ordering::SeqCst), before + 2);
        assert_eq!(
            inn.sign_in("ana@ejemplo.mx", &"x".repeat(MAX_PASSWORD_CHARS + 1))
                .await,
            Err(SignInError::InvalidCredentials)
        );
    }

    #[tokio::test]
    async fn sign_in_storage_failure_is_unavailable() {
        let users = Arc::new(FakeUsers {
            fail: true,
            ..FakeUsers::default()
        });
        let inn = SignIn::new(users, Arc::new(FakeHasher::default()));
        assert_eq!(
            inn.sign_in("ana@ejemplo.mx", "contraseña1").await,
            Err(SignInError::Unavailable)
        );
    }

    #[tokio::test]
    async fn onboarding_and_locale_update_the_account() {
        let (users, _, up, _) = setup();
        let u = up
            .sign_up("ana@ejemplo.mx", "contraseña1", Locale::Es)
            .await
            .unwrap();
        let m = Memberships {
            costco: true,
            sams: false,
        };
        let after = UpdateOnboarding::new(users.clone())
            .set(u.id, m)
            .await
            .unwrap();
        assert_eq!((after.memberships, after.onboarded), (m, true));
        let profile = Profile::new(users.clone());
        assert_eq!(
            profile.set_locale(u.id, Locale::En).await.unwrap().locale,
            Locale::En
        );
        assert_eq!(profile.get(u.id).await.unwrap().locale, Locale::En);
        let gone = UserId::new(99).unwrap();
        assert_eq!(profile.get(gone).await, Err(ProfileError::NotFound));
    }
}
