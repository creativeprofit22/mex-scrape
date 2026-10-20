//! Queries for `users` (migration `0008_users_sessions.sql`) and the
//! [`app::UserRepo`] adapter.
//!
//! Every query is keyed by user id or normalized e-mail. The tenant is a
//! generated column (`user-<id>`), so it can never be chosen by a caller.
//! Only [`find_credentials`] reads the password hash.

use app::{CreateUserError, PortFuture, RepoError, UserCredentials, UserRecord, UserRepo};
use domain::{Email, Locale, Memberships, Plan, TenantId, UserId};
use sqlx::Row as _;
use sqlx::postgres::{PgPool, PgRow};

/// A stored account, without its password hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserRow {
    /// User id.
    pub id: i64,
    /// Normalized e-mail.
    pub email: String,
    /// `user-<id>`.
    pub tenant_id: String,
    /// `free` or `pro`.
    pub plan: String,
    /// Costco membership.
    pub has_costco: bool,
    /// Sam's Club membership.
    pub has_sams: bool,
    /// Onboarding completed.
    pub onboarded: bool,
    /// `es` or `en`.
    pub locale: String,
}

/// Why [`create_user`] failed.
#[derive(Debug, thiserror::Error)]
pub enum CreateUserRowError {
    /// The e-mail is already registered (unique violation).
    #[error("e-mail already registered")]
    EmailTaken,
    /// Any other database error; redact before logging.
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

/// Columns selected for a [`UserRow`].
macro_rules! user_columns {
    () => {
        "id, email, tenant_id, plan, has_costco, has_sams, onboarded, locale"
    };
}

fn user_row(row: &PgRow) -> Result<UserRow, sqlx::Error> {
    Ok(UserRow {
        id: row.try_get("id")?,
        email: row.try_get("email")?,
        tenant_id: row.try_get("tenant_id")?,
        plan: row.try_get("plan")?,
        has_costco: row.try_get("has_costco")?,
        has_sams: row.try_get("has_sams")?,
        onboarded: row.try_get("onboarded")?,
        locale: row.try_get("locale")?,
    })
}

/// Inserts a `free`, not-onboarded account.
///
/// # Errors
///
/// [`CreateUserRowError::EmailTaken`] on a duplicate e-mail; other database
/// errors (including CHECK violations) as [`CreateUserRowError::Db`].
pub async fn create_user(
    pool: &PgPool,
    email: &str,
    password_hash: &str,
    locale: &str,
    now_ms: i64,
) -> Result<UserRow, CreateUserRowError> {
    let row = sqlx::query(concat!(
        "INSERT INTO users (email, password_hash, locale, created_at, updated_at) \
         VALUES ($1, $2, $3, to_timestamp($4 / 1000.0), to_timestamp($4 / 1000.0)) \
         RETURNING ",
        user_columns!()
    ))
    .bind(email)
    .bind(password_hash)
    .bind(locale)
    .bind(now_ms)
    .fetch_one(pool)
    .await
    .map_err(|e| match e.as_database_error() {
        Some(d) if d.is_unique_violation() => CreateUserRowError::EmailTaken,
        _ => CreateUserRowError::Db(e),
    })?;
    Ok(user_row(&row)?)
}

/// The account for `email` with its password hash.
///
/// # Errors
///
/// Database errors; redact before logging.
pub async fn find_credentials(
    pool: &PgPool,
    email: &str,
) -> Result<Option<(UserRow, String)>, sqlx::Error> {
    let row = sqlx::query(concat!(
        "SELECT ",
        user_columns!(),
        ", password_hash FROM users WHERE email = $1"
    ))
    .bind(email)
    .fetch_optional(pool)
    .await?;
    row.map(|r| Ok((user_row(&r)?, r.try_get("password_hash")?)))
        .transpose()
}

/// The account `id`.
///
/// # Errors
///
/// Database errors; redact before logging.
pub async fn find_by_id(pool: &PgPool, id: i64) -> Result<Option<UserRow>, sqlx::Error> {
    let row = sqlx::query(concat!(
        "SELECT ",
        user_columns!(),
        " FROM users WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await?;
    row.as_ref().map(user_row).transpose()
}

/// Stores memberships and marks onboarding done; `None` if `id` is gone.
///
/// # Errors
///
/// Database errors; redact before logging.
pub async fn set_memberships(
    pool: &PgPool,
    id: i64,
    has_costco: bool,
    has_sams: bool,
    now_ms: i64,
) -> Result<Option<UserRow>, sqlx::Error> {
    let row = sqlx::query(concat!(
        "UPDATE users SET has_costco = $2, has_sams = $3, onboarded = true, \
           updated_at = to_timestamp($4 / 1000.0) \
         WHERE id = $1 RETURNING ",
        user_columns!()
    ))
    .bind(id)
    .bind(has_costco)
    .bind(has_sams)
    .bind(now_ms)
    .fetch_optional(pool)
    .await?;
    row.as_ref().map(user_row).transpose()
}

/// Stores the preferred language; `None` if `id` is gone.
///
/// # Errors
///
/// Database errors (including CHECK violations); redact before logging.
pub async fn set_locale(
    pool: &PgPool,
    id: i64,
    locale: &str,
    now_ms: i64,
) -> Result<Option<UserRow>, sqlx::Error> {
    let row = sqlx::query(concat!(
        "UPDATE users SET locale = $2, updated_at = to_timestamp($3 / 1000.0) \
         WHERE id = $1 RETURNING ",
        user_columns!()
    ))
    .bind(id)
    .bind(locale)
    .bind(now_ms)
    .fetch_optional(pool)
    .await?;
    row.as_ref().map(user_row).transpose()
}

/// Sets the plan of account `id` (operator and seed use); `false` if gone.
///
/// # Errors
///
/// Database errors (including CHECK violations); redact before logging.
pub async fn set_plan(
    pool: &PgPool,
    id: i64,
    plan: &str,
    now_ms: i64,
) -> Result<bool, sqlx::Error> {
    let done = sqlx::query(
        "UPDATE users SET plan = $2, updated_at = to_timestamp($3 / 1000.0) WHERE id = $1",
    )
    .bind(id)
    .bind(plan)
    .bind(now_ms)
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Maps a stored row to the app record; `None` if it breaks the schema's
/// invariants (including a tenant other than `user-<id>`), so callers fail
/// closed.
#[must_use]
pub fn record_from_row(r: UserRow) -> Option<UserRecord> {
    let id = UserId::new(r.id)?;
    let email = Email::parse(&r.email).ok()?;
    let tenant = TenantId::new(&r.tenant_id).ok()?;
    if email.as_str() != r.email || tenant != TenantId::for_user(id) {
        return None;
    }
    Some(UserRecord {
        id,
        email,
        tenant,
        plan: Plan::parse(&r.plan)?,
        memberships: Memberships {
            costco: r.has_costco,
            sams: r.has_sams,
        },
        onboarded: r.onboarded,
        locale: Locale::parse(&r.locale)?,
    })
}

fn record(row: Option<UserRow>) -> Result<Option<UserRecord>, RepoError> {
    row.map(|r| record_from_row(r).ok_or(RepoError::Unavailable))
        .transpose()
}

/// [`UserRepo`] over the `users` table. Errors become
/// [`RepoError::Unavailable`] (or [`CreateUserError::Unavailable`]) without
/// detail.
#[derive(Clone)]
pub struct PgUsers {
    pool: PgPool,
    clock: std::sync::Arc<dyn app::Clock>,
}

impl PgUsers {
    /// Accounts in `pool`, stamped with `clock`.
    #[must_use]
    pub fn new(pool: PgPool, clock: std::sync::Arc<dyn app::Clock>) -> Self {
        Self { pool, clock }
    }
}

impl UserRepo for PgUsers {
    fn create<'a>(
        &'a self,
        email: &'a Email,
        password_hash: &'a str,
        locale: Locale,
    ) -> PortFuture<'a, Result<UserRecord, CreateUserError>> {
        Box::pin(async move {
            let row = create_user(
                &self.pool,
                email.as_str(),
                password_hash,
                locale.as_str(),
                self.clock.now().0,
            )
            .await
            .map_err(|e| match e {
                CreateUserRowError::EmailTaken => CreateUserError::EmailTaken,
                CreateUserRowError::Db(_) => CreateUserError::Unavailable,
            })?;
            record_from_row(row).ok_or(CreateUserError::Unavailable)
        })
    }

    fn find_credentials<'a>(
        &'a self,
        email: &'a Email,
    ) -> PortFuture<'a, Result<Option<UserCredentials>, RepoError>> {
        Box::pin(async move {
            let found = find_credentials(&self.pool, email.as_str())
                .await
                .map_err(|_| RepoError::Unavailable)?;
            found
                .map(|(row, password_hash)| {
                    Ok(UserCredentials {
                        user: record_from_row(row).ok_or(RepoError::Unavailable)?,
                        password_hash,
                    })
                })
                .transpose()
        })
    }

    fn find_by_id(&self, id: UserId) -> PortFuture<'_, Result<Option<UserRecord>, RepoError>> {
        Box::pin(async move {
            record(
                find_by_id(&self.pool, id.get())
                    .await
                    .map_err(|_| RepoError::Unavailable)?,
            )
        })
    }

    fn set_memberships(
        &self,
        id: UserId,
        memberships: Memberships,
    ) -> PortFuture<'_, Result<Option<UserRecord>, RepoError>> {
        Box::pin(async move {
            record(
                set_memberships(
                    &self.pool,
                    id.get(),
                    memberships.costco,
                    memberships.sams,
                    self.clock.now().0,
                )
                .await
                .map_err(|_| RepoError::Unavailable)?,
            )
        })
    }

    fn set_locale(
        &self,
        id: UserId,
        locale: Locale,
    ) -> PortFuture<'_, Result<Option<UserRecord>, RepoError>> {
        Box::pin(async move {
            record(
                set_locale(&self.pool, id.get(), locale.as_str(), self.clock.now().0)
                    .await
                    .map_err(|_| RepoError::Unavailable)?,
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> UserRow {
        UserRow {
            id: 7,
            email: "ana@example.mx".to_owned(),
            tenant_id: "user-7".to_owned(),
            plan: "pro".to_owned(),
            has_costco: true,
            has_sams: false,
            onboarded: true,
            locale: "en".to_owned(),
        }
    }

    #[test]
    fn row_maps_to_record() {
        let r = record_from_row(row()).unwrap();
        assert_eq!(r.id.get(), 7);
        assert_eq!(r.tenant.as_str(), "user-7");
        assert_eq!(r.plan, Plan::Pro);
        assert_eq!(
            r.memberships,
            Memberships {
                costco: true,
                sams: false
            }
        );
        assert_eq!(r.locale, Locale::En);
    }

    #[test]
    fn rows_breaking_invariants_fail_closed() {
        let cases: [fn(&mut UserRow); 6] = [
            |r| r.id = 0,
            |r| r.tenant_id = "user-8".to_owned(),
            |r| r.email = "Ana@example.mx".to_owned(),
            |r| r.email = "nope".to_owned(),
            |r| r.plan = "gold".to_owned(),
            |r| r.locale = "fr".to_owned(),
        ];
        for break_it in cases {
            let mut r = row();
            break_it(&mut r);
            assert!(record_from_row(r).is_none());
        }
    }
}
