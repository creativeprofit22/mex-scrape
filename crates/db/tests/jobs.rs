//! Postgres tests for the job tables and pgmq queues (migrations 0005 and
//! 0006). Ignored by default (they need a database); run with
//! `cargo test -p db --test jobs -- --ignored` and `DATABASE_URL` set.
//!
//! Tests run in parallel, so each one owns its rows: the queue test owns
//! `refresh_jobs`, the search test owns `search_jobs`, and the budget test
//! is the only one touching the shared `safety_settings` row.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use app::{BudgetError, Clock, RequestBudget, SearchId, SearchRepo};
use db::DatabaseUrl;
use db::queue::{self, Queue};
use db::searches::{self, PgSearches, RetailerOutcomeRow};
use db::tracking;
use db::usage::{self, ConsumeOutcome, PgRequestBudget};
use domain::{RetailerSearchOutcome, SearchQuery, SearchStatus, TenantId, UnixMillis};
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

async fn purge(pool: &PgPool, q: Queue) {
    sqlx::query("SELECT pgmq.purge_queue($1::text)")
        .bind(q.as_str())
        .execute(pool)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "needs Postgres (DATABASE_URL)"]
async fn queue_send_read_backoff_delete_and_dead_letter() {
    let pool = pool().await;
    purge(&pool, Queue::RefreshJobs).await;
    purge(&pool, Queue::RefreshJobsDlq).await;

    let a = queue::send(&pool, Queue::RefreshJobs, r#"{"kind":"a"}"#)
        .await
        .unwrap();
    let b = queue::send(&pool, Queue::RefreshJobs, r#"{"kind":"b"}"#)
        .await
        .unwrap();
    assert_eq!(queue::length(&pool, Queue::RefreshJobs).await.unwrap(), 2);

    // Read hides the message; a second read sees only the other one.
    let first = queue::read(&pool, Queue::RefreshJobs, 30, 1).await.unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].msg_id, a);
    assert_eq!(first[0].read_ct, 1);
    assert_eq!(first[0].message, r#"{"kind": "a"}"#);
    let second = queue::read(&pool, Queue::RefreshJobs, 30, 5).await.unwrap();
    assert_eq!(second.iter().map(|m| m.msg_id).collect::<Vec<_>>(), vec![b]);
    assert!(
        queue::read(&pool, Queue::RefreshJobs, 30, 5)
            .await
            .unwrap()
            .is_empty()
    );

    // Backoff of 0 s makes it visible again, with a higher read count.
    assert!(
        queue::set_vt(&pool, Queue::RefreshJobs, a, 0)
            .await
            .unwrap()
    );
    let again = queue::read(&pool, Queue::RefreshJobs, 30, 5).await.unwrap();
    assert_eq!(again.len(), 1);
    assert_eq!((again[0].msg_id, again[0].read_ct), (a, 2));

    // Dead-letter moves `a` atomically; delete finishes `b`.
    let dead = queue::dead_letter(
        &pool,
        Queue::RefreshJobs,
        a,
        r#"{"error_code":"transient","attempts":2}"#,
    )
    .await
    .unwrap();
    assert!(queue::delete(&pool, Queue::RefreshJobs, b).await.unwrap());
    assert!(!queue::delete(&pool, Queue::RefreshJobs, b).await.unwrap());
    assert_eq!(queue::length(&pool, Queue::RefreshJobs).await.unwrap(), 0);
    let dlq = queue::read(&pool, Queue::RefreshJobsDlq, 30, 5)
        .await
        .unwrap();
    assert_eq!(dlq.len(), 1);
    assert_eq!(dlq[0].msg_id, dead);
    assert!(dlq[0].message.contains("\"attempts\": 2"));

    // A dead-letter queue has no dead-letter queue of its own.
    assert!(matches!(
        queue::dead_letter(&pool, Queue::RefreshJobsDlq, dead, "{}").await,
        Err(queue::DeadLetterError::NoDeadLetterQueue)
    ));
    // Invalid JSON is refused by the database.
    assert!(
        queue::send(&pool, Queue::RefreshJobs, "not json")
            .await
            .is_err()
    );
    purge(&pool, Queue::RefreshJobsDlq).await;
}

#[tokio::test]
#[ignore = "needs Postgres (DATABASE_URL)"]
async fn search_is_stored_and_enqueued_together_and_read_per_tenant() {
    let pool = pool().await;
    purge(&pool, Queue::SearchJobs).await;
    let repo = PgSearches::new(pool.clone(), Arc::new(FixedClock(NOW)));
    let me = TenantId::new(&unique("t")).unwrap();
    let other = TenantId::new(&unique("u")).unwrap();
    let query = SearchQuery::parse("  taladro   inalámbrico ").unwrap();

    let id = repo.create_and_enqueue(&me, &query).await.unwrap();
    let msgs = queue::read(&pool, Queue::SearchJobs, 30, 10).await.unwrap();
    assert_eq!(msgs.len(), 1);
    let body: serde_json::Value = serde_json::from_str(&msgs[0].message).unwrap();
    assert_eq!(body["kind"], "search");
    assert_eq!(body["search_id"], id.0);
    assert_eq!(body["tenant_id"], me.as_str());

    let view = repo.load(&me, id).await.unwrap().unwrap();
    assert_eq!(view.status, SearchStatus::Queued);
    assert_eq!(view.query, "taladro inalámbrico");
    assert!(repo.load(&other, id).await.unwrap().is_none());
    assert!(repo.load(&me, SearchId(i64::MAX)).await.unwrap().is_none());

    // Lifecycle: running, outcome, terminal; terminal never changes.
    assert!(searches::mark_running(&pool, id.0, NOW + 1).await.unwrap());
    searches::upsert_retailer_outcome(
        &pool,
        id.0,
        &RetailerOutcomeRow {
            retailer: "sears_mx".to_owned(),
            outcome: "category_only".to_owned(),
            offers: 0,
        },
    )
    .await
    .unwrap();
    assert!(
        searches::finish(&pool, id.0, "done", "", NOW + 2)
            .await
            .unwrap()
    );
    assert!(
        !searches::finish(&pool, id.0, "failed", "x", NOW + 3)
            .await
            .unwrap()
    );
    assert!(!searches::mark_running(&pool, id.0, NOW + 4).await.unwrap());
    let view = repo.load(&me, id).await.unwrap().unwrap();
    assert_eq!(view.status, SearchStatus::Done);
    assert_eq!(view.retailers.len(), 1);
    assert_eq!(
        view.retailers[0].outcome,
        RetailerSearchOutcome::CategoryOnly
    );

    // The insert and the enqueue share one transaction: a CHECK failure
    // leaves neither a row nor a message.
    let before = queue::length(&pool, Queue::SearchJobs).await.unwrap();
    assert!(
        searches::create_search(&pool, "bad tenant", "q", NOW)
            .await
            .is_err()
    );
    assert_eq!(
        queue::length(&pool, Queue::SearchJobs).await.unwrap(),
        before
    );
    purge(&pool, Queue::SearchJobs).await;
}

#[tokio::test]
#[ignore = "needs Postgres (DATABASE_URL)"]
async fn tracked_products_come_due_oldest_first() {
    let pool = pool().await;
    let retailer = "test_tracking";
    let (a, b, c) = (unique("a"), unique("b"), unique("c"));
    assert!(tracking::track(&pool, retailer, &a, NOW).await.unwrap());
    assert!(!tracking::track(&pool, retailer, &a, NOW).await.unwrap());
    assert!(tracking::track(&pool, retailer, &b, NOW).await.unwrap());
    assert!(tracking::track(&pool, retailer, &c, NOW).await.unwrap());
    assert!(
        tracking::mark_refreshed(&pool, retailer, &a, NOW + 10)
            .await
            .unwrap()
    );
    assert!(
        !tracking::mark_refreshed(&pool, retailer, "missing", NOW)
            .await
            .unwrap()
    );

    // `c` was attempted but never refreshed (blocked, delisted, unparseable).
    assert_eq!(
        tracking::mark_attempted(&pool, retailer, &[c.as_str(), "missing"], NOW + 20)
            .await
            .unwrap(),
        1
    );

    let due: Vec<_> = tracking::due(&pool, NOW + 1_000, 10_000)
        .await
        .unwrap()
        .into_iter()
        .filter(|(r, _)| r == retailer)
        .map(|(_, s)| s)
        .collect();
    let pos = |s: &str| due.iter().position(|d| d == s).unwrap();
    assert!(pos(&b) < pos(&a), "never-tried first");
    assert!(
        pos(&a) < pos(&c),
        "attempted-but-unrefreshed rotates behind"
    );

    let not_yet: Vec<_> = tracking::due(&pool, NOW + 5, 10_000)
        .await
        .unwrap()
        .into_iter()
        .filter(|(r, _)| r == retailer)
        .map(|(_, s)| s)
        .collect();
    assert!(not_yet.contains(&b));
    assert!(!not_yet.contains(&a), "refreshed after the cutoff");
    assert!(!not_yet.contains(&c), "attempted after the cutoff");
}

/// The only test touching `safety_settings`; it restores the row.
#[tokio::test]
#[ignore = "needs Postgres (DATABASE_URL)"]
async fn budget_honours_kill_switch_and_never_exceeds_cap_under_races() {
    let pool = pool().await;
    let tenant = TenantId::new(&unique("budget")).unwrap();
    let day = usage::day_start(NOW);

    sqlx::query("UPDATE safety_settings SET global_kill_switch = TRUE WHERE id = 1")
        .execute(&pool)
        .await
        .unwrap();
    let killed = usage::try_consume(&pool, tenant.as_str(), day, 1, 10).await;
    let budget = PgRequestBudget::new(pool.clone(), Arc::new(FixedClock(NOW)), 10);
    let via_port = budget.consume(&tenant, 1).await;
    sqlx::query("UPDATE safety_settings SET global_kill_switch = FALSE WHERE id = 1")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(killed.unwrap(), ConsumeOutcome::KillSwitch);
    assert_eq!(via_port, Err(BudgetError::KillSwitch));
    assert_eq!(usage::used(&pool, tenant.as_str(), day).await.unwrap(), 0);

    // 40 concurrent single requests against a cap of 25.
    let mut tasks = Vec::new();
    for _ in 0..40 {
        let pool = pool.clone();
        let t = tenant.as_str().to_owned();
        tasks.push(tokio::spawn(async move {
            usage::try_consume(&pool, &t, day, 1, 25).await.unwrap()
        }));
    }
    let mut granted = 0;
    for t in tasks {
        if t.await.unwrap() == ConsumeOutcome::Granted {
            granted += 1;
        }
    }
    assert_eq!(granted, 25);
    assert_eq!(usage::used(&pool, tenant.as_str(), day).await.unwrap(), 25);

    // A request larger than the cap on a fresh day is refused outright,
    // and the next day starts from zero.
    let next = day + domain::DAY_MS;
    assert_eq!(
        usage::try_consume(&pool, tenant.as_str(), next, 26, 25)
            .await
            .unwrap(),
        ConsumeOutcome::CapReached
    );
    assert_eq!(
        usage::try_consume(&pool, tenant.as_str(), next, 25, 25)
            .await
            .unwrap(),
        ConsumeOutcome::Granted
    );
    let capped = PgRequestBudget::new(pool.clone(), Arc::new(FixedClock(next)), 25);
    assert_eq!(
        capped.consume(&tenant, 1).await,
        Err(BudgetError::CapReached)
    );
}
