//! Postgres tests for `amazon_products`. Ignored by default (they need a
//! database); run with `cargo test -p db --test amazon_products -- --ignored`
//! and `DATABASE_URL` set. Each test uses its own nanos-derived ASIN and
//! deletes it at the end, so tests share no rows.

use std::time::{SystemTime, UNIX_EPOCH};

use db::DatabaseUrl;
use db::amazon_products::{self, ProductRow};
use db::price_history::{self, MatchRow};
use sqlx::postgres::PgPool;

/// A fresh ASIN: `prefix` (one letter) then nine nanos digits.
fn asin(prefix: char) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{prefix}{:09}", nanos % 1_000_000_000)
}

async fn pool() -> PgPool {
    let url = DatabaseUrl::from_env()
        .expect("DATABASE_URL is valid")
        .expect("DATABASE_URL is set");
    let pool = db::connect(&url).await.expect("connect");
    db::migrate(&pool).await.expect("migrate");
    pool
}

fn product(asin: &str, title: &str, brand: Option<&str>, at: i64) -> ProductRow {
    ProductRow {
        asin: asin.to_owned(),
        title: title.to_owned(),
        brand: brand.map(str::to_owned),
        source: "sp_api".to_owned(),
        seen_at_ms: at,
    }
}

async fn cleanup(pool: &PgPool, asin: &str) {
    sqlx::query("DELETE FROM amazon_products WHERE asin = $1")
        .bind(asin)
        .execute(pool)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "needs Postgres (DATABASE_URL)"]
async fn newest_name_wins_and_brand_is_kept() {
    let pool = pool().await;
    let asin = asin('N');
    let t = 1_760_000_000_000;
    assert!(
        amazon_products::find_product(&pool, &asin)
            .await
            .unwrap()
            .is_none()
    );

    let first = product(&asin, "Licuadora 600 W", Some("Oster"), t);
    assert_eq!(
        amazon_products::upsert_product(&pool, &first)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        amazon_products::find_product(&pool, &asin).await.unwrap(),
        Some(first)
    );

    // A newer lookup without a brand refreshes the name, keeps the brand.
    let mut newer = product(&asin, "Licuadora Oster 600 W, 1.5 L", None, t + 1_000);
    newer.source = "bright_data".to_owned();
    assert_eq!(
        amazon_products::upsert_product(&pool, &newer)
            .await
            .unwrap(),
        1
    );
    let stored = amazon_products::find_product(&pool, &asin)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.title, "Licuadora Oster 600 W, 1.5 L");
    assert_eq!(stored.brand.as_deref(), Some("Oster"));
    assert_eq!(stored.source, "bright_data");
    assert_eq!(stored.seen_at_ms, t + 1_000);

    // An older lookup is ignored.
    let older = product(&asin, "Nombre viejo", Some("Otra"), t - 1_000);
    assert_eq!(
        amazon_products::upsert_product(&pool, &older)
            .await
            .unwrap(),
        0
    );
    let kept = amazon_products::find_product(&pool, &asin)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(kept.title, "Licuadora Oster 600 W, 1.5 L");
    assert_eq!(kept.brand.as_deref(), Some("Oster"));

    cleanup(&pool, &asin).await;
}

#[tokio::test]
#[ignore = "needs Postgres (DATABASE_URL)"]
async fn checks_refuse_bad_products() {
    let pool = pool().await;
    let asin = asin('C');
    let t = 1_760_000_000_000;
    for bad in [
        product(&asin, "", None, t),
        product(&asin, &"x".repeat(501), None, t),
        product(&asin, "Licuadora", Some(""), t),
        product("lowercase1", "Licuadora", None, t),
        ProductRow {
            source: "keepa".to_owned(),
            ..product(&asin, "Licuadora", None, t)
        },
    ] {
        assert!(
            amazon_products::upsert_product(&pool, &bad).await.is_err(),
            "{bad:?}"
        );
    }
    cleanup(&pool, &asin).await;
}

#[tokio::test]
#[ignore = "needs Postgres (DATABASE_URL)"]
async fn any_match_sees_every_status() {
    let pool = pool().await;
    let asin = asin('M');
    assert!(!price_history::any_match(&pool, &asin).await.unwrap());
    let sku = format!("any-match-{asin}");
    price_history::upsert_match(
        &pool,
        MatchRow {
            retailer: "test_retailer",
            retailer_sku: &sku,
            asin: &asin,
            method: "title",
            confidence_bp: 4_000,
            status: "needs_review",
            matched_at_ms: 1_760_000_000_000,
        },
    )
    .await
    .unwrap();
    assert!(price_history::any_match(&pool, &asin).await.unwrap());
    sqlx::query("DELETE FROM offer_matches WHERE retailer_sku = $1")
        .bind(&sku)
        .execute(&pool)
        .await
        .unwrap();
}
