//! The demo account: the login `seed_demo` creates or resets. The demo
//! worker runs only its searches, so the e2e account is never touched.

use anyhow::Context as _;
use domain::Memberships;
use sqlx::PgPool;

use crate::users::{self, UserRow};

/// The demo login e-mail.
pub const DEMO_EMAIL: &str = "demo@example.com";

/// The demo login password (a local fixture, like `seed_e2e`'s).
pub const DEMO_PASSWORD: &str = "demo-password-123";

/// The demo account's memberships: Costco yes, Sam's Club no, so Sam's Club
/// results are hidden ("Ocultas por membresía") on every default search
/// that finds a Sam's Club listing.
pub const DEMO_MEMBERSHIPS: Memberships = Memberships {
    costco: true,
    sams: false,
};

/// Creates the demo account, or resets its password, locale (`es`), plan
/// (`free`) and memberships ([`DEMO_MEMBERSHIPS`]; also marks onboarding
/// done). Returns the stored row.
///
/// # Errors
///
/// Database errors; redact before logging.
pub async fn upsert_demo_account(pool: &PgPool, now_ms: i64) -> anyhow::Result<UserRow> {
    let hash = password_auth::generate_hash(DEMO_PASSWORD);
    let id = match users::find_credentials(pool, DEMO_EMAIL)
        .await
        .context("reading the demo account")?
    {
        Some((user, _)) => {
            sqlx::query("UPDATE users SET password_hash = $2 WHERE id = $1")
                .bind(user.id)
                .bind(&hash)
                .execute(pool)
                .await
                .context("resetting the demo password")?;
            user.id
        }
        None => {
            users::create_user(pool, DEMO_EMAIL, &hash, "es", now_ms)
                .await
                .context("creating the demo account")?
                .id
        }
    };
    users::set_locale(pool, id, "es", now_ms)
        .await
        .context("setting the demo locale")?;
    users::set_plan(pool, id, "free", now_ms)
        .await
        .context("setting the demo plan")?;
    users::set_memberships(
        pool,
        id,
        DEMO_MEMBERSHIPS.costco,
        DEMO_MEMBERSHIPS.sams,
        now_ms,
    )
    .await
    .context("setting the demo memberships")?
    .context("the demo account disappeared while seeding")
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{COSTCO_RETAILER_ID, RetailerId, SAMS_RETAILER_ID};

    #[test]
    fn sams_club_is_hidden_and_costco_shown() {
        let sams = RetailerId::new(SAMS_RETAILER_ID).unwrap();
        let costco = RetailerId::new(COSTCO_RETAILER_ID).unwrap();
        assert!(!DEMO_MEMBERSHIPS.allows(true, &sams));
        assert!(DEMO_MEMBERSHIPS.allows(true, &costco));
    }
}
