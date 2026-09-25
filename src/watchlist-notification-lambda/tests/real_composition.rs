use aura_historia_jobs::{
    DomainJob, DomainJobPayload, IdempotencyKey, OrderingKey, ProductListingEventJob, WorkerQueue,
    encode,
};
use aws_lambda_events::sqs::{SqsBatchResponse, SqsEvent, SqsMessage};
use domain_primitives::event_id::EventId;
use lambda_runtime::{Context, LambdaEvent};
use listing_source_core::ListingSourceId;
use product_listing_core::product_listing_id::ProductListingId;
use serde_json::{Value, json};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::OffsetDateTime;
use user_core::user_id::UserId;
use watchlist_notification_lambda::{compose_watchlist_notification_use_case, handler};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn missing_event_is_retried_then_committed_event_creates_one_notification_and_intent() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let user = seed_user(&pool).await?;
        let source = EventId::new();
        let product = seed_product(&pool).await?;
        seed_watchlist(&pool, user, product).await?;
        let body = job_body(product, source)?;
        let use_case = compose_watchlist_notification_use_case(pool.clone());

        assert_eq!(
            failure_ids(handler(event("missing", body.clone()), use_case.as_ref()).await?),
            ["missing"]
        );
        assert_eq!(0, notification_count(&pool, user, source).await?);
        insert_event(&pool, product, source).await?;
        assert!(
            failure_ids(handler(event("retry", body.clone()), use_case.as_ref()).await?).is_empty()
        );
        assert!(
            failure_ids(handler(event("duplicate", body), use_case.as_ref()).await?).is_empty()
        );
        assert_notification(&pool, user, source).await?;
        Ok(())
    }
    .await;
    assert!(
        result.is_ok(),
        "watchlist missing-source/redelivery composition failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn overlapping_invocations_insert_only_one_notification_and_delivery_intent() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let user = seed_user(&pool).await?;
        let source = EventId::new();
        let product = seed_product(&pool).await?;
        seed_watchlist(&pool, user, product).await?;
        insert_event(&pool, product, source).await?;
        let body = job_body(product, source)?;
        let use_case = compose_watchlist_notification_use_case(pool.clone());

        // Hold the target table until both invocations have reached the write path.
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
        assert_notification(&pool, user, source).await?;
        Ok(())
    }
    .await;
    assert!(
        result.is_ok(),
        "watchlist concurrent insert composition failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn withdrawn_listing_suppresses_historical_watchlist_notification() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let user = seed_user(&pool).await?;
        let source = EventId::new();
        let product = seed_product(&pool).await?;
        seed_watchlist(&pool, user, product).await?;
        insert_event(&pool, product, source).await?;
        sqlx::query("UPDATE product_listings SET lifecycle = 'WITHDRAWN', availability = NULL WHERE product_listing_id = $1")
            .bind(product.as_uuid()).execute(&pool).await?;

        let use_case = compose_watchlist_notification_use_case(pool.clone());
        let body = job_body(product, source)?;
        assert!(failure_ids(handler(event("withdrawn", body), use_case.as_ref()).await?).is_empty());
        assert_eq!(0, notification_count(&pool, user, source).await?);
        Ok(())
    }.await;
    assert!(
        result.is_ok(),
        "watchlist withdrawn-listing composition failed: {result:?}"
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
    product: ProductListingId,
    source: EventId,
) -> Result<String, aura_historia_jobs::WireError> {
    encode(&DomainJob {
        target_queue: WorkerQueue::WatchlistNotification,
        idempotency_key: IdempotencyKey::new(format!("product-event:{source}")),
        ordering_key: OrderingKey::new(format!("product:{product}")),
        payload: DomainJobPayload::<aura_historia_jobs::SearchFilterOperation>::ProductListingEvent(
            ProductListingEventJob {
                event_id: source,
                product_listing_id: product,
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
    .bind(format!("lambda-watchlist-{user}@example.test"))
    .execute(pool)
    .await?;
    Ok(user)
}

async fn seed_product(pool: &sqlx::PgPool) -> Result<ProductListingId, sqlx::Error> {
    let product = ProductListingId::new();
    let initial_event = EventId::new();
    let listing_source = ListingSourceId::new();
    let mut tx = pool.begin().await?;
    sqlx::query("WITH operator AS (INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, 'Fixture operator') RETURNING party_id) INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) SELECT $3, $4, 'Watchlist lambda source', party_id FROM operator")
        .bind(uuid::Uuid::now_v7()).bind(format!("watchlist-operator-{}", listing_source.as_uuid()))
        .bind(listing_source.as_uuid()).bind(format!("watchlist-source-{}", listing_source.as_uuid()))
        .execute(&mut *tx).await?;
    sqlx::query("INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, title_text, title_language, availability, lifecycle, url, product_images) VALUES ($1, $2, $3, $3, $3, $4, $5, 'Watchlist lambda product', 'en', 'AVAILABLE', 'ACTIVE', 'https://example.test/product', '[]')")
        .bind(product.as_uuid()).bind(format!("watchlist-product-{}", &product.as_uuid().simple().to_string()[26..]))
        .bind(initial_event.as_uuid()).bind(listing_source.as_uuid()).bind(product.to_string())
        .execute(&mut *tx).await?;
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_CHANGED', 'DOMAIN', 1, $3, now())")
        .bind(initial_event.as_uuid()).bind(product.as_uuid())
        .bind(json!({"images": {"previousCount": 0, "currentCount": 0}}))
        .execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(product)
}

async fn seed_watchlist(
    pool: &sqlx::PgPool,
    user: UserId,
    product: ProductListingId,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO product_listing_watchlist (user_id, product_listing_id, notifications, state, active_since, notifications_enabled_since) VALUES ($1, $2, true, 'ACTIVE', now() - interval '1 minute', now() - interval '1 minute')")
        .bind(user.as_uuid()).bind(product.as_uuid()).execute(pool).await?;
    Ok(())
}

async fn insert_event(
    pool: &sqlx::PgPool,
    product: ProductListingId,
    source: EventId,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_CHANGED', 'DOMAIN', 1, $3, $4)")
        .bind(source.as_uuid()).bind(product.as_uuid())
        .bind(json!({"availability": {"previous": "AVAILABLE", "current": "SOLD_OUT"}}))
        .bind(OffsetDateTime::now_utc()).execute(pool).await?;
    Ok(())
}

async fn notification_count(
    pool: &sqlx::PgPool,
    user: UserId,
    source: EventId,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE user_id = $1 AND origin_event_id = $2",
    )
    .bind(user.as_uuid())
    .bind(source.as_uuid())
    .fetch_one(pool)
    .await
}

async fn assert_notification(pool: &sqlx::PgPool, user: UserId, source: EventId) -> TestResult {
    let rows: Vec<(String, Value, i64)> = sqlx::query_as("SELECT n.kind, n.payload, (SELECT count(*) FROM notification_deliveries d WHERE d.notification_id = n.notification_id) FROM notifications n WHERE n.user_id = $1 AND n.origin_event_id = $2")
        .bind(user.as_uuid()).bind(source.as_uuid()).fetch_all(pool).await?;
    assert_eq!(1, rows.len());
    assert_eq!("WATCHLIST_AVAILABILITY_CHANGED", rows[0].0);
    assert_eq!(
        Some("AVAILABLE"),
        rows[0]
            .1
            .pointer("/change/old_availability")
            .and_then(Value::as_str)
    );
    assert_eq!(
        Some("SOLD_OUT"),
        rows[0]
            .1
            .pointer("/change/new_availability")
            .and_then(Value::as_str)
    );
    assert_eq!(1, rows[0].2);
    Ok(())
}
