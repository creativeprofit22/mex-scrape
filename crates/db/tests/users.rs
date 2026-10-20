//! Postgres tests for accounts, sessions and the search-result columns
//! added by migration 0008. Ignored by default (they need a database); run
//! with `cargo test -p db --test users -- --ignored` and `DATABASE_URL` set.
//!
//! Tests run in parallel, so each one uses unique e-mails, session ids and
//! tenants.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use app::{Clock, CreateUserError, SearchId, SearchRepo, UserRepo};
use db::DatabaseUrl;
use db::searches::{self, PgSearches, SearchResultRow};
use db::sessions;
use db::users::{self, PgUsers};
use domain::{Availability, Email, Locale, Memberships, Plan, TenantId, UnixMillis};
use sqlx::postgres::PgPool;

const NOW: i64 = 1_760_000_000_000;

struct FixedClock(i64);

impl Clock for FixedClock {
    fn now(&self) -> UnixMillis {
        UnixMillis(self.0)
    }
}

fn unique(name: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{name}{nanos}")
}

async fn pool() -> PgPool {
    let url = DatabaseUrl::from_env()
        .expect("DATABASE_URL is valid")
        .expect("DATABASE_URL is set");
    let pool = db::connect(&url).await.expect("connect");
    db::migrate(&pool).await.expect("migrate");
    pool
}

#[tokio::test]
#[ignore = "needs Postgres (DATABASE_URL)"]
async fn accounts_create_find_update_and_reject_duplicates() {
    let pool = pool().await;
    let repo = PgUsers::new(pool.clone(), Arc::new(FixedClock(NOW)));
    let email = Email::parse(&format!("{}@example.mx", unique("ana"))).unwrap();

    let user = repo
        .create(&email, "$argon2id$fake", Locale::En)
        .await
        .unwrap();
    assert_eq!(user.email, email);
    assert_eq!(user.tenant, TenantId::for_user(user.id));
    assert_eq!(user.plan, Plan::Free);
    assert_eq!(user.memberships, Memberships::default());
    assert!(!user.onboarded);
    assert_eq!(user.locale, Locale::En);

    assert_eq!(
        repo.create(&email, "$argon2id$other", Locale::Es).await,
        Err(CreateUserError::EmailTaken)
    );

    let creds = repo.find_credentials(&email).await.unwrap().unwrap();
    assert_eq!(creds.user, user);
    assert_eq!(creds.password_hash, "$argon2id$fake");
    let other = Email::parse(&format!("{}@example.mx", unique("nobody"))).unwrap();
    assert!(repo.find_credentials(&other).await.unwrap().is_none());

    let m = Memberships {
        costco: true,
        sams: false,
    };
    let onboarded = repo.set_memberships(user.id, m).await.unwrap().unwrap();
    assert!(onboarded.onboarded);
    assert_eq!(onboarded.memberships, m);
    let es = repo.set_locale(user.id, Locale::Es).await.unwrap().unwrap();
    assert_eq!(es.locale, Locale::Es);
    assert!(
        users::set_plan(&pool, user.id.get(), "pro", NOW)
            .await
            .unwrap()
    );
    assert_eq!(
        repo.find_by_id(user.id).await.unwrap().unwrap().plan,
        Plan::Pro
    );

    // Bad values are refused by the schema.
    assert!(
        users::set_plan(&pool, user.id.get(), "gold", NOW)
            .await
            .is_err()
    );
    assert!(
        users::set_locale(&pool, user.id.get(), "fr", NOW)
            .await
            .is_err()
    );
    assert!(
        users::create_user(&pool, "Upper@Example.mx", "h", "es", NOW)
            .await
            .is_err()
    );

    // Unknown ids read and update nothing.
    let gone = domain::UserId::new(i64::MAX).unwrap();
    assert!(repo.find_by_id(gone).await.unwrap().is_none());
    assert!(repo.set_locale(gone, Locale::En).await.unwrap().is_none());
}

#[tokio::test]
#[ignore = "needs Postgres (DATABASE_URL)"]
async fn sessions_insert_save_load_expire_and_delete() {
    let pool = pool().await;
    let id = unique("s");
    let later = NOW + 60_000;

    assert!(
        sessions::insert_new(&pool, &id, b"one", later)
            .await
            .unwrap()
    );
    assert!(
        !sessions::insert_new(&pool, &id, b"two", later)
            .await
            .unwrap()
    );
    assert_eq!(
        sessions::load(&pool, &id, NOW).await.unwrap().as_deref(),
        Some(&b"one"[..])
    );
    sessions::save(&pool, &id, b"three", later).await.unwrap();
    assert_eq!(
        sessions::load(&pool, &id, NOW).await.unwrap().as_deref(),
        Some(&b"three"[..])
    );
    // Expired at `later`: no longer readable, and cleaned up.
    assert!(sessions::load(&pool, &id, later).await.unwrap().is_none());
    assert!(sessions::delete_expired(&pool, later).await.unwrap() >= 1);
    assert!(sessions::load(&pool, &id, NOW).await.unwrap().is_none());

    let id2 = unique("s");
    sessions::save(&pool, &id2, b"x", later).await.unwrap();
    sessions::delete(&pool, &id2).await.unwrap();
    assert!(sessions::load(&pool, &id2, NOW).await.unwrap().is_none());
    sessions::delete(&pool, &id2).await.unwrap();
}

#[tokio::test]
#[ignore = "needs Postgres (DATABASE_URL)"]
async fn search_results_store_availability_and_membership() {
    let pool = pool().await;
    let tenant = TenantId::new(&unique("t-")).unwrap();
    let repo = PgSearches::new(pool.clone(), Arc::new(FixedClock(NOW)));
    let id = searches::create_search(&pool, tenant.as_str(), "pan", NOW)
        .await
        .unwrap();
    let row = |sku: &str, availability: &str, membership_required| SearchResultRow {
        retailer: "costco_mx".to_owned(),
        retailer_sku: sku.to_owned(),
        title: "Pan".to_owned(),
        url: "https://www.costco.com.mx/p/1".to_owned(),
        price_mxn: 5_000,
        availability: availability.to_owned(),
        membership_required,
        asin: None,
        match_status: None,
        confidence_bp: None,
        amazon_price_mxn: None,
        fees_mxn: None,
        profit_centavos: None,
        roi_bp: None,
        margin_bp: None,
    };
    searches::upsert_result(&pool, id, &row("1", "in_stock", true))
        .await
        .unwrap();
    searches::upsert_result(&pool, id, &row("2", "out_of_stock", false))
        .await
        .unwrap();
    // Retries overwrite the new columns too.
    searches::upsert_result(&pool, id, &row("2", "pre_order", false))
        .await
        .unwrap();
    assert!(
        searches::upsert_result(&pool, id, &row("3", "maybe", false))
            .await
            .is_err()
    );

    let view = repo.load(&tenant, SearchId(id)).await.unwrap().unwrap();
    assert_eq!(view.hidden_results, 0);
    let got: Vec<_> = view
        .results
        .iter()
        .map(|r| {
            (
                r.retailer_sku.as_str(),
                r.availability,
                r.membership_required,
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            ("1", Availability::InStock, true),
            ("2", Availability::PreOrder, false)
        ]
    );
}
