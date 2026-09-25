use aura_historia_jobs::{
    DomainJob, DomainJobPayload, IdempotencyKey, OrderingKey, SearchFilterMatchCreatedJob,
    WorkerQueue, encode,
};
use aws_lambda_events::sqs::{SqsBatchResponse, SqsEvent, SqsMessage};
use domain_primitives::event_id::EventId;
use lambda_runtime::{Context, LambdaEvent};
use listing_source_core::ListingSourceId;
use product_listing_core::product_listing_id::ProductListingId;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use search_filter_match_notification_lambda::{
    compose_search_filter_match_notification_use_case, handler,
};
use serde_json::{Value, json};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use user_core::user_id::UserId;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn missing_match_is_retried_then_committed_match_creates_one_notification_and_intent() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let user = seed_user(&pool).await?;
        let filter = seed_filter(&pool, user).await?;
        let (product, source) = seed_product(&pool).await?;
        let body = job_body(user, filter, product, source)?;
        let use_case = compose_search_filter_match_notification_use_case(pool.clone());

        assert_eq!(
            failure_ids(handler(event("missing", body.clone()), use_case.as_ref()).await?),
            ["missing"]
        );
        assert_eq!(0, notification_count(&pool, user, filter, source).await?);
        seed_match(&pool, user, filter, product, source).await?;
        assert!(
            failure_ids(handler(event("retry", body.clone()), use_case.as_ref()).await?).is_empty()
        );
        assert!(
            failure_ids(handler(event("duplicate", body), use_case.as_ref()).await?).is_empty()
        );
        assert_notification(&pool, user, filter, source).await?;
        Ok(())
    }
    .await;
    assert!(
        result.is_ok(),
        "match notification missing-source/redelivery composition failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn overlapping_invocations_insert_only_one_match_notification_and_delivery_intent() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let user = seed_user(&pool).await?;
        let filter = seed_filter(&pool, user).await?;
        let (product, source) = seed_product(&pool).await?;
        seed_match(&pool, user, filter, product, source).await?;
        let body = job_body(user, filter, product, source)?;
        let use_case = compose_search_filter_match_notification_use_case(pool.clone());

        let mut barrier = pool.begin().await?;
        sqlx::query("LOCK TABLE notifications IN ACCESS EXCLUSIVE MODE")
            .execute(&mut *barrier)
            .await?;
        let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *barrier)
            .await?;
        let first = tokio::spawn({
            let use_case = use_case.clone();
            let body = body.clone();
            async move { handler(event("first", body), use_case.as_ref()).await }
        });
        wait_for_blocked(&pool, blocker, 1).await?;
        let second = tokio::spawn({
            let use_case = use_case.clone();
            async move { handler(event("second", body), use_case.as_ref()).await }
        });
        wait_for_blocked(&pool, blocker, 2).await?;
        barrier.commit().await?;
        assert!(failure_ids(first.await??).is_empty());
        assert!(failure_ids(second.await??).is_empty());
        assert_notification(&pool, user, filter, source).await?;
        Ok(())
    }
    .await;
    assert!(
        result.is_ok(),
        "match notification concurrent insert composition failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn withdrawn_listing_suppresses_historical_match_notification() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let user = seed_user(&pool).await?;
        let filter = seed_filter(&pool, user).await?;
        let (product, source) = seed_product(&pool).await?;
        seed_match(&pool, user, filter, product, source).await?;
        sqlx::query("UPDATE product_listings SET lifecycle = 'WITHDRAWN', availability = NULL WHERE product_listing_id = $1")
            .bind(product.as_uuid()).execute(&pool).await?;

        let use_case = compose_search_filter_match_notification_use_case(pool.clone());
        let body = job_body(user, filter, product, source)?;
        assert!(failure_ids(handler(event("withdrawn", body), use_case.as_ref()).await?).is_empty());
        assert_eq!(0, notification_count(&pool, user, filter, source).await?);
        Ok(())
    }.await;
    assert!(
        result.is_ok(),
        "match notification withdrawn-listing composition failed: {result:?}"
    );
}

async fn wait_for_blocked(pool: &sqlx::PgPool, blocker: i32, expected: i64) -> TestResult {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let blocked: i64 = sqlx::query_scalar("WITH RECURSIVE blocked(pid) AS (SELECT pid FROM pg_stat_activity WHERE $1 = ANY(pg_blocking_pids(pid)) UNION SELECT a.pid FROM pg_stat_activity a JOIN blocked b ON b.pid = ANY(pg_blocking_pids(a.pid))) SELECT count(*) FROM blocked")
                .bind(blocker).fetch_one(pool).await?;
            if blocked >= expected { return Ok::<(), sqlx::Error>(()); }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }).await??;
    Ok(())
}

fn event(id: &str, body: String) -> LambdaEvent<SqsEvent> {
    let mut message = SqsMessage::default();
    message.message_id = Some(id.to_owned());
    message.body = Some(body);
    let mut payload = SqsEvent::default();
    payload.records = vec![message];
    let mut context = Context::default();
    context.deadline = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        + 60_000;
    LambdaEvent::new(payload, context)
}

fn failure_ids(response: SqsBatchResponse) -> Vec<String> {
    response
        .batch_item_failures
        .into_iter()
        .map(|failure| failure.item_identifier)
        .collect()
}

fn job_body(
    user: UserId,
    filter: UserSearchFilterId,
    product: ProductListingId,
    source: EventId,
) -> Result<String, aura_historia_jobs::WireError> {
    encode(&DomainJob {
        target_queue: WorkerQueue::SearchFilterMatchNotification,
        idempotency_key: IdempotencyKey::new(format!(
            "search-filter-match:{user}:{filter}:{product}:{source}"
        )),
        ordering_key: OrderingKey::new(format!("user:{user}")),
        payload:
            DomainJobPayload::<aura_historia_jobs::SearchFilterOperation>::SearchFilterMatchCreated(
                SearchFilterMatchCreatedJob {
                    user_id: user,
                    user_search_filter_id: filter,
                    product_listing_id: product,
                    origin_event_id: source,
                },
            ),
    })
}

async fn seed_user(pool: &sqlx::PgPool) -> Result<UserId, sqlx::Error> {
    let user = UserId::new();
    sqlx::query(
        "INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'ULTIMATE', 'USER')",
    )
    .bind(user.as_uuid())
    .bind(format!("lambda-match-{user}@example.test"))
    .execute(pool)
    .await?;
    Ok(user)
}

async fn seed_filter(pool: &sqlx::PgPool, user: UserId) -> Result<UserSearchFilterId, sqlx::Error> {
    let filter = UserSearchFilterId::new();
    sqlx::query("INSERT INTO search_filters (user_search_filter_id, user_id, name, notifications, state, search, language, currency) VALUES ($1, $2, 'Fixture filter', true, 'ACTIVE', $3, 'en', 'EUR')")
        .bind(filter.as_uuid()).bind(user.as_uuid()).bind(json!({})).execute(pool).await?;
    Ok(filter)
}

async fn seed_product(pool: &sqlx::PgPool) -> Result<(ProductListingId, EventId), sqlx::Error> {
    let product = ProductListingId::new();
    let source = EventId::new();
    let listing_source = ListingSourceId::new();
    let mut tx = pool.begin().await?;
    sqlx::query("WITH operator AS (INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, 'Fixture operator') RETURNING party_id) INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) SELECT $3, $4, 'Match notification lambda source', party_id FROM operator")
        .bind(uuid::Uuid::now_v7()).bind(format!("match-operator-{}", listing_source.as_uuid()))
        .bind(listing_source.as_uuid()).bind(format!("match-source-{}", listing_source.as_uuid()))
        .execute(&mut *tx).await?;
    sqlx::query("INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, title_text, title_language, availability, lifecycle, url, product_images) VALUES ($1, $2, $3, $3, $3, $4, $5, 'Match notification lambda product', 'en', 'AVAILABLE', 'ACTIVE', 'https://example.test/product', '[]')")
        .bind(product.as_uuid()).bind(format!("match-product-{}", &product.as_uuid().simple().to_string()[26..]))
        .bind(source.as_uuid()).bind(listing_source.as_uuid()).bind(product.to_string())
        .execute(&mut *tx).await?;
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_CHANGED', 'DOMAIN', 1, $3, now())")
        .bind(source.as_uuid()).bind(product.as_uuid())
        .bind(json!({"images": {"previousCount": 0, "currentCount": 0}})).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok((product, source))
}

async fn seed_match(
    pool: &sqlx::PgPool,
    user: UserId,
    filter: UserSearchFilterId,
    product: ProductListingId,
    source: EventId,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO search_filter_matches (user_id, user_search_filter_id, product_listing_id, origin_event_id, user_search_filter_name) VALUES ($1, $2, $3, $4, 'Fixture filter')")
        .bind(user.as_uuid()).bind(filter.as_uuid()).bind(product.as_uuid()).bind(source.as_uuid())
        .execute(pool).await?;
    Ok(())
}

async fn notification_count(
    pool: &sqlx::PgPool,
    user: UserId,
    filter: UserSearchFilterId,
    source: EventId,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT count(*) FROM notifications WHERE user_id = $1 AND user_search_filter_id = $2 AND origin_event_id = $3")
        .bind(user.as_uuid()).bind(filter.as_uuid()).bind(source.as_uuid()).fetch_one(pool).await
}

async fn assert_notification(
    pool: &sqlx::PgPool,
    user: UserId,
    filter: UserSearchFilterId,
    source: EventId,
) -> TestResult {
    let rows: Vec<(String, Value, i64)> = sqlx::query_as("SELECT n.kind, n.payload, (SELECT count(*) FROM notification_deliveries d WHERE d.notification_id = n.notification_id) FROM notifications n WHERE n.user_id = $1 AND n.user_search_filter_id = $2 AND n.origin_event_id = $3")
        .bind(user.as_uuid()).bind(filter.as_uuid()).bind(source.as_uuid()).fetch_all(pool).await?;
    assert_eq!(1, rows.len());
    assert_eq!("SEARCH_FILTER_MATCH", rows[0].0);
    assert_eq!(
        Some("SEARCH_FILTER"),
        rows[0].1.get("type").and_then(Value::as_str)
    );
    assert_eq!(
        Some("Fixture filter"),
        rows[0]
            .1
            .get("user_search_filter_name")
            .and_then(Value::as_str)
    );
    assert_eq!(1, rows[0].2);
    Ok(())
}
