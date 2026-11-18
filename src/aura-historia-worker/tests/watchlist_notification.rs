use aura_historia_jobs::{
    DomainJob, DomainJobPayload, IdempotencyKey, OrderingKey, ProductListingEventJob, WorkerQueue,
    encode,
};
use aura_historia_worker::scopes::watchlist_notification::{
    WatchlistNotificationJobs, compose_watchlist_notification_use_case,
};
use domain_primitives::event_id::EventId;
use listing_source_core::ListingSourceId;
use platform_postgres_queue::{Disposition, JobId};
use platform_worker::{Job, JobHandler, error_class};
use product_listing_core::product_listing_id::ProductListingId;
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::OffsetDateTime;
use user_core::user_id::UserId;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn missing_event_is_retried_then_committed_event_creates_one_notification_and_intent() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let user = seed_user(&pool).await?;
        let source = EventId::new();
        let product = seed_product(&pool).await?;
        seed_watchlist(&pool, user, product).await?;
        let body = job_body(product, source)?;
        let handler = watchlist_jobs(&pool);

        assert_eq!(
            Disposition::Retry(error_class("missing_source")),
            handler.handle(&claimed(1, body.clone())).await
        );
        assert_eq!(0, notification_count(&pool, user, source).await?);
        insert_event(&pool, product, source).await?;
        assert_eq!(
            Disposition::Complete,
            handler.handle(&claimed(2, body.clone())).await
        );
        assert_eq!(
            Disposition::Complete,
            handler.handle(&claimed(1, body)).await
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

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn overlapping_invocations_insert_only_one_notification_and_delivery_intent() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let user = seed_user(&pool).await?;
        let source = EventId::new();
        let product = seed_product(&pool).await?;
        seed_watchlist(&pool, user, product).await?;
        insert_event(&pool, product, source).await?;
        let body = job_body(product, source)?;
        let handler = watchlist_jobs(&pool);

        // Hold the target table until both invocations have reached the write path.
        let mut barrier = pool.begin().await?;
        sqlx::query("LOCK TABLE notifications IN ACCESS EXCLUSIVE MODE")
            .execute(&mut *barrier)
            .await?;
        let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *barrier)
            .await?;
        let first = test_api::spawn_owned({
            let handler = handler.clone();
            let body = body.clone();
            async move { handler.handle(&claimed(1, body)).await }
        });
        wait_for_blocked(&pool, blocker, 1).await?;
        let second = test_api::spawn_owned({
            let handler = handler.clone();
            async move { handler.handle(&claimed(1, body)).await }
        });
        wait_for_blocked(&pool, blocker, 2).await?;
        barrier.commit().await?;
        assert_eq!(Disposition::Complete, first.await?);
        assert_eq!(Disposition::Complete, second.await?);
        assert_notification(&pool, user, source).await?;
        Ok(())
    }
    .await;
    assert!(
        result.is_ok(),
        "watchlist concurrent insert composition failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
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

        let body = job_body(product, source)?;
        assert_eq!(Disposition::Complete, watchlist_jobs(&pool).handle(&claimed(1, body)).await);
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

fn watchlist_jobs(pool: &sqlx::PgPool) -> Arc<WatchlistNotificationJobs> {
    Arc::new(WatchlistNotificationJobs::new(
        compose_watchlist_notification_use_case(pool.clone()),
    ))
}

fn claimed(attempt: u32, body: String) -> Job {
    Job::new(JobId::from_uuid(uuid::Uuid::now_v7()), attempt, None, body)
}

fn job_body(
    product: ProductListingId,
    source: EventId,
) -> Result<String, aura_historia_jobs::WireError> {
    encode(&DomainJob {
        target_queue: WorkerQueue::WatchlistNotification,
        idempotency_key: IdempotencyKey::new(format!("product-event:{source}")),
        ordering_key: OrderingKey::new(format!("product:{product}")),
        payload: DomainJobPayload::ProductListingEvent(ProductListingEventJob {
            event_id: source,
            product_listing_id: product,
        }),
    })
}

async fn seed_user(pool: &sqlx::PgPool) -> Result<UserId, sqlx::Error> {
    let user = UserId::new();
    sqlx::query(
        "INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'ULTIMATE', 'USER')",
    )
    .bind(user.as_uuid())
    .bind(format!("worker-watchlist-{user}@example.test"))
    .execute(pool)
    .await?;
    Ok(user)
}

async fn seed_product(pool: &sqlx::PgPool) -> Result<ProductListingId, sqlx::Error> {
    let product = ProductListingId::new();
    let initial_event = EventId::new();
    let listing_source = ListingSourceId::new();
    let mut tx = pool.begin().await?;
    sqlx::query("WITH operator AS (INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, 'Fixture operator') RETURNING party_id) INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id, listing_source_type) SELECT $3, $4, 'Watchlist lambda source', party_id, 'DEALER' FROM operator")
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
