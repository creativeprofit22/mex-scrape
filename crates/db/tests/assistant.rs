//! Postgres tests for the assistant tables (migration 0009) and recent
//! search results. Ignored by default (they need a database); run with
//! `cargo test -p db --test assistant -- --ignored` and `DATABASE_URL` set.
//!
//! Tests run in parallel, so each one uses its own accounts and tenants.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use app::{
    AgentUsageRepo, ChatId, Clock, MatchDecisionRepo, NewDecision, NewSuggestion,
    RecentResultsFilter, SearchId, SearchRepo, StoredDecision, SuggestionId, UserRecord, UserRepo,
};
use db::DatabaseUrl;
use db::assistant::{PgAgentUsage, PgMatchDecisions};
use db::searches::{self, PgSearches, SearchResultRow};
use db::users::PgUsers;
use domain::{
    Asin, Email, Locale, MatchDecision, RetailerId, RetailerSku, SearchQuery, SuggestionReason,
    TenantId, TokenCount, TokenUsage, UnixMillis,
};
use sqlx::postgres::PgPool;

const NOW: i64 = 1_760_000_000_000;
const DAY: i64 = 86_400_000;

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

async fn user(pool: &PgPool, name: &str) -> UserRecord {
    let email = Email::parse(&format!("{}@example.mx", unique(name))).unwrap();
    PgUsers::new(pool.clone(), Arc::new(FixedClock(NOW)))
        .create(&email, "$argon2id$fake", Locale::Es)
        .await
        .unwrap()
}

fn result(sku: &str, title: &str, status: Option<&str>) -> SearchResultRow {
    SearchResultRow {
        retailer: "costco_mx".to_owned(),
        retailer_sku: sku.to_owned(),
        title: title.to_owned(),
        url: format!("https://example.com/{sku}"),
        price_mxn: 100_000,
        availability: "in_stock".to_owned(),
        membership_required: false,
        asin: status.map(|_| "B0E2ETEST1".to_owned()),
        match_status: status.map(str::to_owned),
        confidence_bp: status.map(|_| 7_000),
        amazon_price_mxn: None,
        fees_mxn: None,
        profit_centavos: None,
        roi_bp: None,
        margin_bp: None,
    }
}

fn listing(sku: &str) -> (RetailerId, RetailerSku) {
    (
        RetailerId::new("costco_mx").unwrap(),
        RetailerSku::new(sku).unwrap(),
    )
}

#[tokio::test]
#[ignore = "needs Postgres (DATABASE_URL)"]
async fn token_usage_adds_atomically_per_tenant_and_day() {
    let pool = pool().await;
    let repo = PgAgentUsage::new(pool.clone());
    let a = TenantId::new(&unique("agent-a-")).unwrap();
    let b = TenantId::new(&unique("agent-b-")).unwrap();
    let day = NOW / DAY;
    let usage = |i, o| TokenUsage {
        input: TokenCount::new(i),
        output: TokenCount::new(o),
    };

    assert_eq!(repo.used_on(&a, day).await.unwrap(), TokenUsage::default());
    for _ in 0..10 {
        repo.add(&a, day, usage(3, 2)).await.unwrap();
    }
    assert_eq!(repo.used_on(&a, day).await.unwrap(), usage(30, 20));
    assert_eq!(
        repo.add(&a, day + 1, usage(1, 1)).await.unwrap(),
        usage(1, 1)
    );
    assert_eq!(repo.used_on(&a, day).await.unwrap(), usage(30, 20));
    assert_eq!(repo.used_on(&b, day).await.unwrap(), TokenUsage::default());
}

#[tokio::test]
#[ignore = "needs Postgres (DATABASE_URL)"]
async fn concurrent_usage_adds_never_lose_updates() {
    let pool = pool().await;
    let tenant = TenantId::new(&unique("agent-c-")).unwrap();
    let day = NOW / DAY;
    let tasks: Vec<_> = (0..8)
        .map(|_| {
            let repo = PgAgentUsage::new(pool.clone());
            let tenant = tenant.clone();
            tokio::spawn(async move {
                repo.add(
                    &tenant,
                    day,
                    TokenUsage {
                        input: TokenCount::new(5),
                        output: TokenCount::new(1),
                    },
                )
                .await
                .unwrap()
            })
        })
        .collect();
    for t in tasks {
        t.await.unwrap();
    }
    let total = PgAgentUsage::new(pool).used_on(&tenant, day).await.unwrap();
    assert_eq!(total.total(), TokenCount::new(48));
}

#[tokio::test]
#[ignore = "needs Postgres (DATABASE_URL)"]
async fn suggestions_and_decisions_are_scoped_to_the_tenant() {
    let pool = pool().await;
    let repo = PgMatchDecisions::new(pool.clone());
    let ana = user(&pool, "ana").await;
    let bob = user(&pool, "bob").await;
    let asin = Asin::new("B0E2ETEST1").unwrap();
    let chat = ChatId::parse(&unique("chat-")).unwrap();
    let (retailer, sku) = listing(&unique("sku"));

    let new = NewSuggestion {
        tenant: ana.tenant.clone(),
        chat_id: chat.clone(),
        retailer: retailer.clone(),
        retailer_sku: sku.clone(),
        asin: asin.clone(),
        title: "Taladro".to_owned(),
        decision: MatchDecision::Accept,
        reason: SuggestionReason::parse("Mismo modelo y marca").unwrap(),
        created_at: UnixMillis(NOW),
    };
    let id = repo.create_suggestion(&new).await.unwrap();
    let mine = repo.suggestions_for_chat(&ana.tenant, &chat).await.unwrap();
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].id, id);
    assert_eq!(mine[0].reason, "Mismo modelo y marca");
    assert_eq!(mine[0].created_at, UnixMillis(NOW));
    assert!(
        repo.suggestions_for_chat(&bob.tenant, &chat)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(repo.suggestion(&bob.tenant, id).await.unwrap().is_none());
    assert_eq!(
        repo.suggestion(&ana.tenant, id)
            .await
            .unwrap()
            .unwrap()
            .title,
        "Taladro"
    );
    assert!(
        repo.suggestion(&ana.tenant, SuggestionId(i64::MAX))
            .await
            .unwrap()
            .is_none()
    );

    let decision = |tenant: &TenantId, by: &UserRecord, d, s| NewDecision {
        tenant: tenant.clone(),
        retailer: retailer.clone(),
        retailer_sku: sku.clone(),
        asin: asin.clone(),
        decision: d,
        decided_by: by.id,
        suggestion_id: s,
        decided_at: UnixMillis(NOW),
    };
    // Bob can't confirm Ana's suggestion (foreign key on tenant + id).
    assert!(
        repo.decide(&decision(
            &bob.tenant,
            &bob,
            MatchDecision::Accept,
            Some(id)
        ))
        .await
        .is_err()
    );
    repo.decide(&decision(
        &ana.tenant,
        &ana,
        MatchDecision::Accept,
        Some(id),
    ))
    .await
    .unwrap();
    // A later verdict replaces the earlier one.
    repo.decide(&decision(&ana.tenant, &ana, MatchDecision::Reject, None))
        .await
        .unwrap();
    let expected = vec![StoredDecision {
        retailer: retailer.clone(),
        retailer_sku: sku.clone(),
        asin: asin.clone(),
        decision: MatchDecision::Reject,
    }];
    assert_eq!(
        repo.decisions_for_listings(&ana.tenant, &[(retailer.clone(), sku.clone())])
            .await
            .unwrap(),
        expected
    );
    assert!(
        repo.decisions_for_asin(&ana.tenant, &asin)
            .await
            .unwrap()
            .contains(&expected[0])
    );
    assert!(
        repo.decisions_for_listings(&bob.tenant, &[(retailer.clone(), sku.clone())])
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        repo.decisions_for_listings(&ana.tenant, &[])
            .await
            .unwrap()
            .is_empty()
    );

    // Who decided is recorded, and offer_matches is untouched.
    let by: i64 = sqlx::query_scalar(
        "SELECT decided_by_user_id FROM match_decisions \
         WHERE tenant_id = $1 AND retailer = $2 AND retailer_sku = $3",
    )
    .bind(ana.tenant.as_str())
    .bind(retailer.as_str())
    .bind(sku.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(by, ana.id.get());
    let global: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM offer_matches WHERE retailer = $1 AND retailer_sku = $2",
    )
    .bind(retailer.as_str())
    .bind(sku.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(global, 0);
}

#[tokio::test]
#[ignore = "needs Postgres (DATABASE_URL)"]
async fn recent_results_are_tenant_scoped_deduplicated_and_filtered() {
    let pool = pool().await;
    let repo = PgSearches::new(pool.clone(), Arc::new(FixedClock(NOW)));
    let tenant = TenantId::new(&unique("recent-")).unwrap();
    let other = TenantId::new(&unique("recent-o-")).unwrap();
    let sku = unique("r");

    let old = searches::create_search(&pool, tenant.as_str(), "pan", NOW - 100 * DAY)
        .await
        .unwrap();
    let older = searches::create_search(&pool, tenant.as_str(), "pan", NOW - 2 * DAY)
        .await
        .unwrap();
    let newer = searches::create_search(&pool, tenant.as_str(), "pan", NOW - DAY)
        .await
        .unwrap();
    let foreign = searches::create_search(&pool, other.as_str(), "pan", NOW)
        .await
        .unwrap();
    for (id, s, title, status) in [
        (old, "x", "Pan viejo", None),
        (older, sku.as_str(), "Pan 100% Integral", None),
        (older, "y", "Taladro", Some("accepted")),
        (
            newer,
            sku.as_str(),
            "Pan 100% Integral",
            Some("needs_review"),
        ),
        (foreign, "z", "Pan ajeno", None),
    ] {
        let key = if s == "x" || s == "y" || s == "z" {
            format!("{s}{sku}")
        } else {
            s.to_owned()
        };
        searches::upsert_result(&pool, id, &result(&key, title, status))
            .await
            .unwrap();
    }

    let since = UnixMillis(NOW - 90 * DAY);
    let all = repo
        .recent_results(&tenant, since, &RecentResultsFilter::default(), 50)
        .await
        .unwrap();
    // The 100-day-old search is out of range; the repeated listing appears
    // once, from its newest search; newest first.
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].search_id, SearchId(newer));
    assert_eq!(all[0].result.retailer_sku.as_str(), sku);
    assert_eq!(
        all[0].result.amazon.as_ref().unwrap().status,
        domain::MatchStatus::NeedsReview
    );
    assert_eq!(all[0].seen_at, UnixMillis(NOW - DAY));
    assert_eq!(all[1].search_id, SearchId(older));

    // '%' is plain text, not a wildcard; matching ignores case.
    let filter = RecentResultsFilter {
        title_contains: Some(SearchQuery::parse("100% integral").unwrap()),
        listing: None,
    };
    let hits = repo
        .recent_results(&tenant, since, &filter, 50)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    let filter = RecentResultsFilter {
        title_contains: Some(SearchQuery::parse("%").unwrap()),
        listing: None,
    };
    assert_eq!(
        repo.recent_results(&tenant, since, &filter, 50)
            .await
            .unwrap()
            .len(),
        1
    );
    let filter = RecentResultsFilter {
        title_contains: None,
        listing: Some(listing(&sku)),
    };
    let one = repo
        .recent_results(&tenant, since, &filter, 50)
        .await
        .unwrap();
    assert_eq!(one.len(), 1);
    assert_eq!(
        repo.recent_results(&tenant, since, &RecentResultsFilter::default(), 1)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        repo.recent_results(&other, since, &filter, 50)
            .await
            .unwrap()
            .is_empty()
    );
}
