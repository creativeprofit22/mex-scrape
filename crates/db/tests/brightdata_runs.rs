//! Postgres tests for `brightdata_runs` and `safety_settings`. Ignored by
//! default (they need a database); run with
//! `cargo test -p db --test brightdata_runs -- --ignored` and `DATABASE_URL`
//! set. Each test uses its own tenant, so tests share no rows.

use std::time::{SystemTime, UNIX_EPOCH};

use db::DatabaseUrl;
use db::brightdata_runs::{self, InsertRunError, NewRunRow, RunCaps, RunPatch, SafetySettings};
use sqlx::postgres::PgPool;

const CAPS: RunCaps = RunCaps {
    runs_per_day: 20,
    items_per_day: 500,
    items_per_run: 50,
};

fn now_ms() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
}

fn tenant(name: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("test-{name}-{nanos}")
}

async fn pool() -> PgPool {
    let url = DatabaseUrl::from_env()
        .expect("DATABASE_URL is valid")
        .expect("DATABASE_URL is set");
    let pool = db::connect(&url).await.expect("connect");
    db::migrate(&pool).await.expect("migrate");
    pool
}

fn new_run<'a>(tenant_id: &'a str, job: &'a str, requested: i32, now: i64) -> NewRunRow<'a> {
    NewRunRow {
        tenant_id,
        job,
        mode: "dataset",
        input_json: r#"{"inputs":[{"url":"https://www.amazon.com.mx/dp/B0001"}]}"#,
        requested_count: requested,
        now_ms: now,
    }
}

fn day_start(now: i64) -> i64 {
    now - now.rem_euclid(86_400_000)
}

fn is_check_violation(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .and_then(|database| database.code())
        .is_some_and(|code| code == "23514")
}

#[tokio::test]
#[ignore = "needs Postgres (DATABASE_URL)"]
async fn migration_seeds_safety_settings_closed() {
    let pool = pool().await;
    // Fresh databases (CI) start with the connector off; an operator may
    // have changed a local one, so only the row's presence is asserted
    // there.
    let settings = brightdata_runs::safety_settings(&pool)
        .await
        .unwrap()
        .expect("settings row is seeded");
    if std::env::var_os("CI").is_some() {
        assert_eq!(
            settings,
            SafetySettings {
                global_kill_switch: false,
                brightdata_enabled: false,
            }
        );
    }
    let duplicate = sqlx::query("INSERT INTO safety_settings (id) VALUES (2)")
        .execute(&pool)
        .await
        .unwrap_err();
    assert!(is_check_violation(&duplicate), "{duplicate}");
}

#[tokio::test]
#[ignore = "needs Postgres (DATABASE_URL)"]
async fn one_active_run_per_scope() {
    let pool = pool().await;
    let tenant = tenant("active");
    let now = now_ms();
    let first = brightdata_runs::insert_run_capped(
        &pool,
        &new_run(&tenant, "amazon", 1, now),
        day_start(now),
        &CAPS,
    )
    .await
    .unwrap();
    let second = brightdata_runs::insert_run_capped(
        &pool,
        &new_run(&tenant, "amazon", 1, now),
        day_start(now),
        &CAPS,
    )
    .await;
    assert!(
        matches!(second, Err(InsertRunError::ActiveRun)),
        "{second:?}"
    );
    // The raw index rejects it too, without the app's checks.
    let raw = sqlx::query(
        "INSERT INTO brightdata_runs (tenant_id, job, mode, input_json, status) \
         VALUES ($1, 'amazon', 'dataset', '{}', 'ready')",
    )
    .bind(&tenant)
    .execute(&pool)
    .await
    .unwrap_err();
    assert!(
        raw.as_database_error()
            .is_some_and(|database| database.is_unique_violation()),
        "{raw}"
    );
    brightdata_runs::insert_run_capped(
        &pool,
        &new_run(&tenant, "liverpool", 1, now),
        day_start(now),
        &CAPS,
    )
    .await
    .unwrap();
    let done = RunPatch {
        status: Some("imported"),
        ..RunPatch::default()
    };
    assert!(
        brightdata_runs::update_if_active(&pool, first, &done, now)
            .await
            .unwrap()
    );
    brightdata_runs::insert_run_capped(
        &pool,
        &new_run(&tenant, "amazon", 1, now),
        day_start(now),
        &CAPS,
    )
    .await
    .unwrap();
}

#[tokio::test]
#[ignore = "needs Postgres (DATABASE_URL)"]
async fn checks_reject_bad_values() {
    let pool = pool().await;
    let tenant = tenant("checks");
    for sql in [
        "INSERT INTO brightdata_runs (tenant_id, job, mode, input_json, status) \
         VALUES ($1, 'checks', 'unlocker', '{}', 'paused')",
        "INSERT INTO brightdata_runs (tenant_id, job, mode, input_json) \
         VALUES ($1, 'checks', 'browser', '{}')",
        "INSERT INTO brightdata_runs (tenant_id, job, mode, input_json, snapshot_id) \
         VALUES ($1, 'checks', 'dataset', '{}', '../x')",
        "INSERT INTO brightdata_runs (tenant_id, job, mode, input_json, error_message) \
         VALUES ($1, 'checks', 'unlocker', '{}', repeat('e', 1001))",
        "INSERT INTO brightdata_runs (tenant_id, job, mode, input_json, requested_count) \
         VALUES ($1, 'checks', 'unlocker', '{}', -1)",
        "INSERT INTO brightdata_runs (tenant_id, job, mode, input_json) \
         VALUES ($1, 'checks', 'unlocker', jsonb_build_object('pad', repeat('x', 40000)))",
    ] {
        let error = sqlx::query(sql)
            .bind(&tenant)
            .execute(&pool)
            .await
            .unwrap_err();
        assert!(is_check_violation(&error), "{sql}: {error}");
    }
    let bad_tenant = sqlx::query(
        "INSERT INTO brightdata_runs (tenant_id, job, mode, input_json) \
         VALUES ('a/b', 'checks', 'unlocker', '{}')",
    )
    .execute(&pool)
    .await
    .unwrap_err();
    assert!(is_check_violation(&bad_tenant), "{bad_tenant}");

    let now = now_ms();
    let id = brightdata_runs::insert_run_capped(
        &pool,
        &new_run(&tenant, "checks", 1, now),
        day_start(now),
        &CAPS,
    )
    .await
    .unwrap();
    let bad_snapshot = RunPatch {
        snapshot_id: Some("a?b"),
        ..RunPatch::default()
    };
    let error = brightdata_runs::update_if_active(&pool, id, &bad_snapshot, now)
        .await
        .unwrap_err();
    assert!(is_check_violation(&error), "{error}");
}

#[tokio::test]
#[ignore = "needs Postgres (DATABASE_URL)"]
async fn capped_insert_enforces_caps_in_gate_order() {
    let pool = pool().await;
    let tenant = tenant("caps");
    let caps = RunCaps {
        runs_per_day: 2,
        items_per_day: 5,
        items_per_run: 3,
    };
    let now = now_ms();
    let today = day_start(now);
    let insert = |job: &'static str, requested| {
        let pool = pool.clone();
        let tenant = tenant.clone();
        async move {
            brightdata_runs::insert_run_capped(
                &pool,
                &new_run(&tenant, job, requested, now),
                today,
                &caps,
            )
            .await
        }
    };
    assert!(matches!(
        insert("a", 4).await,
        Err(InsertRunError::ItemsPerRunCap)
    ));
    insert("a", 3).await.unwrap();
    assert!(matches!(
        insert("b", 3).await,
        Err(InsertRunError::ItemsPerDayCap)
    ));
    insert("b", 2).await.unwrap();
    assert!(matches!(
        insert("c", 1).await,
        Err(InsertRunError::RunsPerDayCap)
    ));
    let counts = brightdata_runs::gate_counts(&pool, &tenant, "a", today)
        .await
        .unwrap();
    assert_eq!((counts.runs_today, counts.items_today), (2, 5));
    assert!(counts.active_run);
    // Tomorrow's window starts empty.
    let tomorrow = brightdata_runs::gate_counts(&pool, &tenant, "c", today + 86_400_000)
        .await
        .unwrap();
    assert_eq!((tomorrow.runs_today, tomorrow.items_today), (0, 0));
    assert!(!tomorrow.active_run);
}

#[tokio::test]
#[ignore = "needs Postgres (DATABASE_URL)"]
async fn updates_never_overwrite_a_terminal_status() {
    let pool = pool().await;
    let tenant = tenant("terminal");
    let now = now_ms();
    let id = brightdata_runs::insert_run_capped(
        &pool,
        &new_run(&tenant, "amazon", 1, now),
        day_start(now),
        &CAPS,
    )
    .await
    .unwrap();
    let running = RunPatch {
        status: Some("running"),
        snapshot_id: Some("sd_db0000000001"),
        ..RunPatch::default()
    };
    assert!(
        brightdata_runs::update_if_active(&pool, id, &running, now + 1)
            .await
            .unwrap()
    );
    let cancel = RunPatch {
        status: Some("cancelled"),
        error_message: Some("Cancelled by request"),
        ..RunPatch::default()
    };
    assert!(
        brightdata_runs::update_if_active(&pool, id, &cancel, now + 2)
            .await
            .unwrap()
    );
    let late = RunPatch {
        status: Some("imported"),
        row_count: Some(3),
        ..RunPatch::default()
    };
    assert!(
        !brightdata_runs::update_if_active(&pool, id, &late, now + 3)
            .await
            .unwrap()
    );
    let run = brightdata_runs::load_run(&pool, id).await.unwrap().unwrap();
    assert_eq!(run.status, "cancelled");
    assert_eq!(run.row_count, 0);
    assert_eq!(run.snapshot_id.as_deref(), Some("sd_db0000000001"));
    assert_eq!(run.error_message, "Cancelled by request");
    assert_eq!(run.created_at_ms, now);
    assert_eq!(run.updated_at_ms, now + 2);
    let input: serde_json::Value = serde_json::from_str(&run.input_json).unwrap();
    assert_eq!(
        input["inputs"][0]["url"],
        "https://www.amazon.com.mx/dp/B0001"
    );
    let active = brightdata_runs::active_runs(&pool).await.unwrap();
    assert!(active.iter().all(|row| row.id != id));
}

#[tokio::test]
#[ignore = "needs Postgres (DATABASE_URL)"]
async fn driver_lock_is_exclusive_until_released() {
    let pool = pool().await;
    let held = brightdata_runs::try_driver_lock(&pool)
        .await
        .unwrap()
        .expect("lock is free");
    assert!(
        brightdata_runs::try_driver_lock(&pool)
            .await
            .unwrap()
            .is_none()
    );
    held.release().await.unwrap();
    let again = brightdata_runs::try_driver_lock(&pool).await.unwrap();
    assert!(again.is_some());
}

#[tokio::test]
#[ignore = "needs Postgres (DATABASE_URL)"]
async fn access_events_count_by_host_and_status_widened() {
    let pool = pool().await;
    let tenant_id = tenant("access");
    let now = now_ms();
    let run = brightdata_runs::insert_run_capped(
        &pool,
        &new_run(&tenant_id, "j", 1, now),
        day_start(now),
        &CAPS,
    )
    .await
    .unwrap();
    let host = format!("{tenant_id}.example");
    for outcome in [
        "ok",
        "blocked_needs_kyc",
        "blocked_needs_kyc",
        "skipped_robots",
    ] {
        brightdata_runs::insert_access_event(&pool, run, &host, outcome)
            .await
            .unwrap();
    }
    assert!(
        brightdata_runs::insert_access_event(&pool, run, &host, "bogus")
            .await
            .is_err()
    );
    let counts = brightdata_runs::access_counts_by_host(&pool).await.unwrap();
    let mine = counts.iter().find(|c| c.host == host).unwrap();
    assert_eq!(
        (
            mine.ok,
            mine.blocked_needs_kyc,
            mine.skipped_robots,
            mine.failed
        ),
        (1, 2, 1, 0)
    );
    let widened =
        sqlx::query("UPDATE brightdata_runs SET status = 'blocked_needs_kyc' WHERE id = $1")
            .bind(run)
            .execute(&pool)
            .await;
    assert!(widened.is_ok());
}
