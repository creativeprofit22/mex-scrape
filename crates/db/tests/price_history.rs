//! Postgres tests for `price_observations` and `offer_matches`. Ignored by
//! default (they need a database); run with
//! `cargo test -p db --test price_history -- --ignored` and `DATABASE_URL`
//! set. `price_observations` is append-only, so rows cannot be cleaned up:
//! each test uses its own nanos-suffixed SKUs and shares no rows.

use std::time::{SystemTime, UNIX_EPOCH};

use db::DatabaseUrl;
use db::price_history::{self, MatchRow, NewObservation};
use sqlx::postgres::PgPool;

const RETAILER: &str = "test_retailer";

fn sku(name: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{name}-{nanos}")
}

async fn pool() -> PgPool {
    let url = DatabaseUrl::from_env()
        .expect("DATABASE_URL is valid")
        .expect("DATABASE_URL is set");
    let pool = db::connect(&url).await.expect("connect");
    db::migrate(&pool).await.expect("migrate");
    pool
}

fn obs<'a>(sku: &'a str, price: i64, at: i64) -> NewObservation<'a> {
    NewObservation {
        retailer: RETAILER,
        retailer_sku: sku,
        price_mxn: price,
        list_price_mxn: None,
        availability: "in_stock",
        observed_at_ms: at,
    }
}

fn row<'a>(sku: &'a str, asin: &'a str, status: &'a str) -> MatchRow<'a> {
    MatchRow {
        retailer: RETAILER,
        retailer_sku: sku,
        asin,
        method: "title",
        confidence_bp: 9_000,
        status,
        matched_at_ms: 1_760_000_000_000,
    }
}

#[tokio::test]
#[ignore = "needs Postgres (DATABASE_URL)"]
async fn observations_read_back_in_time_order_and_filter_by_since() {
    let pool = pool().await;
    let sku = sku("order");
    let t = 1_760_000_000_123;
    for (price, at) in [(300, t + 2_000), (100, t), (200, t + 1_000)] {
        price_history::insert_observation(&pool, obs(&sku, price, at))
            .await
            .unwrap();
    }
    let mut with_list = obs(&sku, 400, t + 3_000);
    with_list.list_price_mxn = Some(500);
    with_list.availability = "out_of_stock";
    price_history::insert_observation(&pool, with_list)
        .await
        .unwrap();

    let all = price_history::observations_since(&pool, RETAILER, &sku, 0)
        .await
        .unwrap();
    let got: Vec<(i64, i64)> = all
        .iter()
        .map(|r| (r.price_mxn, r.observed_at_ms))
        .collect();
    assert_eq!(
        got,
        [
            (100, t),
            (200, t + 1_000),
            (300, t + 2_000),
            (400, t + 3_000)
        ]
    );
    assert_eq!(all[3].list_price_mxn, Some(500));
    assert_eq!(all[3].availability, "out_of_stock");

    let recent = price_history::observations_since(&pool, RETAILER, &sku, t + 1_000)
        .await
        .unwrap();
    assert_eq!(recent.len(), 3);
}

#[tokio::test]
#[ignore = "needs Postgres (DATABASE_URL)"]
async fn observations_are_append_only() {
    let pool = pool().await;
    let sku = sku("append");
    price_history::insert_observation(&pool, obs(&sku, 100, 1_760_000_000_000))
        .await
        .unwrap();
    let update = sqlx::query("UPDATE price_observations SET price_mxn = 1 WHERE retailer_sku = $1")
        .bind(&sku)
        .execute(&pool)
        .await;
    assert!(update.is_err(), "UPDATE must be refused");
    let delete = sqlx::query("DELETE FROM price_observations WHERE retailer_sku = $1")
        .bind(&sku)
        .execute(&pool)
        .await;
    assert!(delete.is_err(), "DELETE must be refused");
    let truncate = sqlx::query("TRUNCATE price_observations")
        .execute(&pool)
        .await;
    assert!(truncate.is_err(), "TRUNCATE must be refused");
    let rows = price_history::observations_since(&pool, RETAILER, &sku, 0)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].price_mxn, 100);
}

#[tokio::test]
#[ignore = "needs Postgres (DATABASE_URL)"]
async fn checks_refuse_bad_observations() {
    let pool = pool().await;
    let sku = sku("checks");
    let mut negative = obs(&sku, -1, 1);
    assert!(
        price_history::insert_observation(&pool, negative)
            .await
            .is_err()
    );
    negative.price_mxn = 100;
    negative.list_price_mxn = Some(100);
    assert!(
        price_history::insert_observation(&pool, negative)
            .await
            .is_err()
    );
    negative.list_price_mxn = None;
    negative.availability = "sold";
    assert!(
        price_history::insert_observation(&pool, negative)
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "needs Postgres (DATABASE_URL)"]
async fn upsert_replaces_review_but_keeps_rejection() {
    let pool = pool().await;
    let review = sku("review");
    assert_eq!(
        price_history::upsert_match(&pool, row(&review, "B000000001", "needs_review"))
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        price_history::upsert_match(&pool, row(&review, "B000000002", "accepted"))
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        price_history::match_status(&pool, RETAILER, &review)
            .await
            .unwrap(),
        Some(("B000000002".to_owned(), "accepted".to_owned()))
    );

    let rejected = sku("rejected");
    assert_eq!(
        price_history::upsert_match(&pool, row(&rejected, "B000000001", "rejected"))
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        price_history::upsert_match(&pool, row(&rejected, "B000000003", "accepted"))
            .await
            .unwrap(),
        0,
        "a kept rejection writes no row"
    );
    assert_eq!(
        price_history::match_status(&pool, RETAILER, &rejected)
            .await
            .unwrap(),
        Some(("B000000001".to_owned(), "rejected".to_owned()))
    );
}

#[tokio::test]
#[ignore = "needs Postgres (DATABASE_URL)"]
async fn accepted_listings_are_filtered_and_sorted() {
    let pool = pool().await;
    // A per-test ASIN, so other tests' rows never appear.
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let asin = format!("T{:09}", nanos % 1_000_000_000);
    let base = sku("listing");
    let (b, a, c) = (
        format!("{base}-b"),
        format!("{base}-a"),
        format!("{base}-c"),
    );
    for (s, status) in [(&b, "accepted"), (&a, "accepted"), (&c, "needs_review")] {
        price_history::upsert_match(&pool, row(s, &asin, status))
            .await
            .unwrap();
    }
    let mut other = row(&base, &asin, "accepted");
    other.retailer = "aaa_store";
    price_history::upsert_match(&pool, other).await.unwrap();

    let got = price_history::accepted_listings(&pool, &asin)
        .await
        .unwrap();
    let expected = [
        ("aaa_store".to_owned(), base.clone()),
        (RETAILER.to_owned(), a.clone()),
        (RETAILER.to_owned(), b.clone()),
    ];
    assert_eq!(got, expected);
}
