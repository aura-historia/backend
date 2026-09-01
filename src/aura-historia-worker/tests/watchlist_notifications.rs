use aura_historia_worker::cdc::WorkerQueue;
use aura_historia_worker::watchlist_notifications::consume_watchlist_notification_queue;
use aura_historia_worker::{QueueConfig, WorkerRunError, WorkerRuntime, serve_with_runtime};

use domain_primitives::event_id::EventId;
use platform_postgres::SqlxUnitOfWork;
use product_listing_core::product_listing_id::ProductListingId;
use std::sync::Arc;
use std::time::{Duration, Instant};

use notification_postgres::{
    SqlxNotificationDeliveryIntentRepositoryFactory, SqlxNotificationRepositoryFactory,
};
use notification_service::{
    initial_external_delivery_plan_reader::InitialExternalDeliveryPlanReaderFactory,
    notification_creation::NotificationCreationCoordinatorFactory,
};
use product_listing_postgres::SqlxProductListingWatchlistNotificationSourceReaderFactory;

use product_listing_service::use_cases::{
    GenerateWatchlistNotificationsHandler, GenerateWatchlistNotificationsUseCase,
};
use serde_json::json;
use test_api::{
    IntegrationTestService, Postgres, Sequin, aura_integration_test, get_postgres_client,
    get_sequin_worker_webhook_bind_addr,
};
use time::OffsetDateTime;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use user_core::user_id::UserId;
use watchlist_postgres::SqlxWatchlistNotificationRecipientReaderFactory;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
const WORKER_SEQUIN: Sequin = Sequin::worker_webhook();
const POLL_INTERVAL: Duration = Duration::from_millis(200);
const POLL_ATTEMPTS: usize = 80;
const NO_NOTIFICATION_OBSERVATION: Duration = Duration::from_secs(2);

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_SEQUIN])]
async fn should_create_availability_notification_from_committed_product_event() {
    let result = create_availability_notification_from_committed_product_event().await;

    assert!(
        result.is_ok(),
        "availability notification acceptance test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_SEQUIN])]
async fn should_create_price_notifications_only_for_active_watchers() {
    let result = create_price_notifications_only_for_active_watchers().await;

    assert!(
        result.is_ok(),
        "price notification acceptance test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_SEQUIN])]
async fn should_not_notify_watcher_created_after_product_event() {
    let result = no_notification_for_watcher_created_after_product_event().await;

    assert!(
        result.is_ok(),
        "late watcher acceptance test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_SEQUIN])]
async fn should_preserve_one_notification_when_product_event_delivery_is_retried() {
    let result = preserve_one_notification_when_product_event_delivery_is_retried().await;

    assert!(
        result.is_ok(),
        "duplicate delivery acceptance test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_SEQUIN])]
async fn should_not_notify_for_rolled_back_or_unrouted_product_listing_events() {
    let result = not_notify_for_rolled_back_or_unrouted_product_listing_events().await;

    assert!(
        result.is_ok(),
        "non-notification event acceptance test failed: {result:?}"
    );
}

async fn create_availability_notification_from_committed_product_event()
-> Result<(), Box<dyn std::error::Error>> {
    let worker = WatchlistWorker::start().await?;
    let result = async {
        let user_id = seed_user(&worker.pool, "availability-recipient").await?;
        let event_id = EventId::new();
        let mut transaction = worker.pool.begin().await?;
        let product_listing_id = seed_product(&mut transaction, event_id).await?;
        seed_watchlist(
            &mut transaction,
            user_id,
            product_listing_id,
            true,
            "ACTIVE",
        )
        .await?;
        insert_product_event(
            &mut transaction,
            event_id,
            product_listing_id,
            "PRODUCT_LISTING_CHANGED",
            json!({"availability": {"previous": "AVAILABLE", "current": null}}),
        )
        .await?;
        transaction.commit().await?;

        let notifications = wait_for_notifications(&worker.pool, user_id, 1).await?;
        assert_eq!(uuid::Uuid::from(event_id), notifications[0].origin_event_id);
        assert_availability_change(&notifications[0], Some("AVAILABLE"), None)?;
        assert!(
            notifications[0]
                .payload
                .pointer("/change/new_availability")
                .is_some_and(serde_json::Value::is_null)
        );
        Ok(())
    }
    .await;

    worker.finish(result).await
}

async fn create_price_notifications_only_for_active_watchers()
-> Result<(), Box<dyn std::error::Error>> {
    let worker = WatchlistWorker::start().await?;
    let result = async {
        let email_recipient = seed_user(&worker.pool, "price-email").await?;
        let in_app_recipient = seed_user(&worker.pool, "price-in-app").await?;
        let inactive_recipient = seed_user(&worker.pool, "price-inactive").await?;
        let event_id = EventId::new();
        let mut transaction = worker.pool.begin().await?;
        let product_listing_id = seed_product(&mut transaction, event_id).await?;
        seed_watchlist(
            &mut transaction,
            email_recipient,
            product_listing_id,
            true,
            "ACTIVE",
        )
        .await?;
        seed_watchlist(
            &mut transaction,
            in_app_recipient,
            product_listing_id,
            false,
            "ACTIVE",
        )
        .await?;
        seed_watchlist(
            &mut transaction,
            inactive_recipient,
            product_listing_id,
            true,
            "INACTIVE_BY_USER",
        )
        .await?;
        insert_product_event(
            &mut transaction,
            event_id,
            product_listing_id,
            "PRODUCT_LISTING_CHANGED",
            json!({
                "pricing": {
                    "price": {
                        "previous": {"amount": 1200, "currency": "USD"},
                        "current": {"amount": 900, "currency": "USD"}
                    }
                }
            }),
        )
        .await?;
        transaction.commit().await?;

        let email_notifications = wait_for_notifications(&worker.pool, email_recipient, 1).await?;
        let _in_app_notifications =
            wait_for_notifications(&worker.pool, in_app_recipient, 1).await?;
        assert_eq!(
            uuid::Uuid::from(event_id),
            email_notifications[0].origin_event_id
        );
        assert_price_change(&email_notifications[0], "USD", 1200, 900)?;
        assert_no_notifications_for(
            &worker.pool,
            inactive_recipient,
            NO_NOTIFICATION_OBSERVATION,
        )
        .await
    }
    .await;

    worker.finish(result).await
}

async fn no_notification_for_watcher_created_after_product_event()
-> Result<(), Box<dyn std::error::Error>> {
    let worker = WatchlistWorker::start().await?;
    let result = async {
        let user_id = seed_user(&worker.pool, "late-watcher").await?;
        let event_id = EventId::new();
        let event_time = OffsetDateTime::now_utc();
        let mut transaction = worker.pool.begin().await?;
        let product_listing_id = seed_product(&mut transaction, event_id).await?;
        insert_product_event_at(
            &mut transaction,
            event_id,
            product_listing_id,
            "PRODUCT_LISTING_CHANGED",
            json!({"availability": {"previous": "AVAILABLE", "current": "SOLD_OUT"}}),
            event_time,
        )
        .await?;
        transaction.commit().await?;

        seed_watchlist_at(
            &worker.pool,
            user_id,
            product_listing_id,
            true,
            "ACTIVE",
            event_time + time::Duration::seconds(1),
        )
        .await?;
        let response = reqwest::Client::new()
            .post(format!(
                "http://{}/cdc/sequin",
                get_sequin_worker_webhook_bind_addr()
            ))
            .json(&json!({
                "record": {
                    "event_id": event_id.to_string(),
                    "product_listing_id": product_listing_id.to_string(),
                    "event_type": "PRODUCT_LISTING_CHANGED",
                    "event_group": "DOMAIN",
                    "event_type_schema_version": 1,
                    "payload": {"availability": {}}
                },
                "action": "insert",
                "metadata": {"table_schema": "public", "table_name": "product_listing_events"}
            }))
            .send()
            .await?;
        assert_eq!(reqwest::StatusCode::ACCEPTED, response.status());
        assert_no_notifications_for(&worker.pool, user_id, NO_NOTIFICATION_OBSERVATION).await
    }
    .await;

    worker.finish(result).await
}

async fn preserve_one_notification_when_product_event_delivery_is_retried()
-> Result<(), Box<dyn std::error::Error>> {
    let worker = WatchlistWorker::start().await?;
    let result = async {
        let user_id = seed_user(&worker.pool, "duplicate-recipient").await?;
        let event_id = EventId::new();
        let mut transaction = worker.pool.begin().await?;
        let product_listing_id = seed_product(&mut transaction, event_id).await?;
        seed_watchlist(
            &mut transaction,
            user_id,
            product_listing_id,
            true,
            "ACTIVE",
        )
        .await?;
        insert_product_event(
            &mut transaction,
            event_id,
            product_listing_id,
            "PRODUCT_LISTING_CHANGED",
            json!({"availability": {"previous": null, "current": "AVAILABLE"}}),
        )
        .await?;
        transaction.commit().await?;
        let _ = wait_for_notifications(&worker.pool, user_id, 1).await?;

        let response = reqwest::Client::new()
            .post(format!(
                "http://{}/cdc/sequin",
                get_sequin_worker_webhook_bind_addr()
            ))
            .json(&json!({
                "record": {
                    "event_id": event_id.to_string(),
                    "product_listing_id": product_listing_id.to_string(),
                    "event_type": "PRODUCT_LISTING_CHANGED",
                    "event_group": "DOMAIN",
                    "event_type_schema_version": 1,
                    "payload": {"availability": {}}
                },
                "action": "insert",
                "metadata": {"table_schema": "public", "table_name": "product_listing_events"}
            }))
            .send()
            .await?;
        assert_eq!(reqwest::StatusCode::ACCEPTED, response.status());
        assert_no_more_than_notifications(&worker.pool, user_id, 1, NO_NOTIFICATION_OBSERVATION)
            .await
    }
    .await;

    worker.finish(result).await
}

async fn not_notify_for_rolled_back_or_unrouted_product_listing_events()
-> Result<(), Box<dyn std::error::Error>> {
    let worker = WatchlistWorker::start().await?;
    let result = async {
        let user_id = seed_user(&worker.pool, "absence-recipient").await?;
        let rolled_back_event_id = EventId::new();
        let mut rolled_back_transaction = worker.pool.begin().await?;
        let rolled_back_product_listing_id =
            seed_product(&mut rolled_back_transaction, rolled_back_event_id).await?;
        seed_watchlist(
            &mut rolled_back_transaction,
            user_id,
            rolled_back_product_listing_id,
            true,
            "ACTIVE",
        )
        .await?;
        insert_product_event(
            &mut rolled_back_transaction,
            rolled_back_event_id,
            rolled_back_product_listing_id,
            "PRODUCT_LISTING_CHANGED",
            json!({"availability": {"previous": "AVAILABLE", "current": "SOLD_OUT"}}),
        )
        .await?;
        drop(rolled_back_transaction);

        let unrouted_event_id = EventId::new();
        let mut unrouted_transaction = worker.pool.begin().await?;
        let unrouted_product_listing_id =
            seed_product(&mut unrouted_transaction, unrouted_event_id).await?;
        seed_watchlist(
            &mut unrouted_transaction,
            user_id,
            unrouted_product_listing_id,
            true,
            "ACTIVE",
        )
        .await?;
        insert_product_event(
            &mut unrouted_transaction,
            unrouted_event_id,
            unrouted_product_listing_id,
            "PRODUCT_LISTING_CHANGED",
            json!({"url": {"previous": "https://example.test/old", "current": "https://example.test/new"}}),
        )
        .await?;
        unrouted_transaction.commit().await?;

        assert_no_notifications_for(&worker.pool, user_id, NO_NOTIFICATION_OBSERVATION).await
    }
    .await;

    worker.finish(result).await
}

struct WatchlistWorker {
    pool: sqlx::PgPool,
    consumer: JoinHandle<()>,
    shutdown_tx: oneshot::Sender<()>,
    server: JoinHandle<Result<(), WorkerRunError>>,
}

impl WatchlistWorker {
    async fn start() -> Result<Self, Box<dyn std::error::Error>> {
        let pool = get_postgres_client().await;
        let handler: Arc<dyn GenerateWatchlistNotificationsUseCase> =
            Arc::new(GenerateWatchlistNotificationsHandler::new(
                SqlxUnitOfWork::new(pool.clone()),
                SqlxProductListingWatchlistNotificationSourceReaderFactory::new(),
                SqlxWatchlistNotificationRecipientReaderFactory,
                NotificationCreationCoordinatorFactory::new(
                    SqlxNotificationRepositoryFactory::new(),
                    InitialExternalDeliveryPlanReaderFactory,
                    SqlxNotificationDeliveryIntentRepositoryFactory::new(),
                ),
            ));
        let (runtime, mut receivers) =
            WorkerRuntime::with_watchlist_notification_queue(QueueConfig::new(16))?;
        let receiver = receivers
            .take(WorkerQueue::WatchlistNotification)
            .ok_or_else(|| std::io::Error::other("watchlist notification queue is missing"))?;
        let consumer = tokio::spawn(consume_watchlist_notification_queue(receiver, handler));
        let listener = tokio::net::TcpListener::bind(get_sequin_worker_webhook_bind_addr()).await?;
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server = tokio::spawn(serve_with_runtime(listener, runtime, async move {
            let _ = shutdown_rx.await;
        }));

        Ok(Self {
            pool,
            consumer,
            shutdown_tx,
            server,
        })
    }

    async fn finish(
        self,
        result: Result<(), Box<dyn std::error::Error>>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let shutdown = self.shutdown().await;
        result?;
        shutdown
    }

    async fn shutdown(self) -> Result<(), Box<dyn std::error::Error>> {
        self.shutdown_tx
            .send(())
            .map_err(|_| std::io::Error::other("worker server shutdown channel closed"))?;
        self.server.await??;
        self.consumer.await?;
        Ok(())
    }
}

async fn seed_user(pool: &sqlx::PgPool, label: &str) -> Result<UserId, sqlx::Error> {
    let user_id = UserId::new();
    sqlx::query(
        "INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'ULTIMATE', 'USER')",
    )
    .bind(uuid::Uuid::from(user_id))
    .bind(format!("worker-watchlist-{label}-{user_id}@example.test"))
    .execute(pool)
    .await?;
    Ok(user_id)
}

async fn seed_product(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    event_id: EventId,
) -> Result<ProductListingId, sqlx::Error> {
    let product_listing_id = ProductListingId::new();
    let product_uuid = uuid::Uuid::from(product_listing_id);
    let listing_source_id = uuid::Uuid::new_v4();
    let product_slug_suffix = product_uuid.simple().to_string()[..6].to_owned();
    sqlx::query("WITH operator AS (INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, concat($2, '-operator'), concat($3, ' operator')) RETURNING party_id) INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) SELECT $1, $2, $3, party_id FROM operator")
        .bind(listing_source_id)
        .bind(format!("worker-watchlist-source-{listing_source_id}"))
        .bind("Worker watchlist source")
        .execute(&mut **transaction)
        .await?;
    sqlx::query("INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, title_text, title_language, availability, lifecycle, url, product_images) VALUES ($1, $2, $3, $3, $3, $4, $5, 'Worker watchlist product', 'en', 'AVAILABLE', 'ACTIVE', 'https://example.test/product', '[]')")
        .bind(product_uuid)
        .bind(format!("worker-watchlist-product-{product_slug_suffix}"))
        .bind(uuid::Uuid::from(event_id))
        .bind(listing_source_id)
        .bind(product_uuid.to_string())

        .execute(&mut **transaction)
        .await?;
    Ok(product_listing_id)
}

async fn seed_watchlist(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: UserId,
    product_listing_id: ProductListingId,
    notifications: bool,
    state: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO product_listing_watchlist (user_id, product_listing_id, notifications, state, active_since, notifications_enabled_since) VALUES ($1, $2, $3, $4, CASE WHEN $4 = 'ACTIVE' THEN now() ELSE NULL END, CASE WHEN $3 THEN now() ELSE NULL END)")
        .bind(uuid::Uuid::from(user_id))
        .bind(uuid::Uuid::from(product_listing_id))
        .bind(notifications)
        .bind(state)
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

async fn insert_product_event(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    event_id: EventId,
    product_listing_id: ProductListingId,
    event_type: &str,
    payload: serde_json::Value,
) -> Result<(), sqlx::Error> {
    insert_product_event_at(
        transaction,
        event_id,
        product_listing_id,
        event_type,
        payload,
        OffsetDateTime::now_utc(),
    )
    .await
}

async fn insert_product_event_at(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    event_id: EventId,
    product_listing_id: ProductListingId,
    event_type: &str,
    payload: serde_json::Value,
    event_time: OffsetDateTime,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, $3, 'DOMAIN', 1, $4, $5)")
        .bind(uuid::Uuid::from(event_id))
        .bind(uuid::Uuid::from(product_listing_id))
        .bind(event_type)
        .bind(payload)
        .bind(event_time)
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

async fn seed_watchlist_at(
    pool: &sqlx::PgPool,
    user_id: UserId,
    product_listing_id: ProductListingId,
    notifications: bool,
    state: &str,
    active_since: OffsetDateTime,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO product_listing_watchlist (user_id, product_listing_id, notifications, state, active_since, notifications_enabled_since, created, updated) VALUES ($1, $2, $3, $4, $5, CASE WHEN $3 THEN $5 ELSE NULL END, $5, $5)")
        .bind(uuid::Uuid::from(user_id))
        .bind(uuid::Uuid::from(product_listing_id))
        .bind(notifications)
        .bind(state)
        .bind(active_since)
        .execute(pool)
        .await?;
    Ok(())
}

#[derive(sqlx::FromRow)]
struct WatchlistNotificationRow {
    origin_event_id: uuid::Uuid,
    kind: String,
    payload: serde_json::Value,
}

async fn notifications_for_user(
    pool: &sqlx::PgPool,
    user_id: UserId,
) -> Result<Vec<WatchlistNotificationRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT origin_event_id, kind, payload FROM notifications \
         WHERE user_id = $1 ORDER BY created, notification_id",
    )
    .bind(uuid::Uuid::from(user_id))
    .fetch_all(pool)
    .await
}

async fn wait_for_notifications(
    pool: &sqlx::PgPool,
    user_id: UserId,
    expected: usize,
) -> Result<Vec<WatchlistNotificationRow>, Box<dyn std::error::Error>> {
    for _ in 0..POLL_ATTEMPTS {
        let notifications = notifications_for_user(pool, user_id).await?;
        if notifications.len() == expected {
            return Ok(notifications);
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err(std::io::Error::other(format!(
        "user {user_id} did not receive {expected} notifications"
    ))
    .into())
}

async fn assert_no_notifications_for(
    pool: &sqlx::PgPool,
    user_id: UserId,
    duration: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    assert_no_more_than_notifications(pool, user_id, 0, duration).await
}

async fn assert_no_more_than_notifications(
    pool: &sqlx::PgPool,
    user_id: UserId,
    maximum: usize,
    duration: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = Instant::now() + duration;
    loop {
        let notifications = notifications_for_user(pool, user_id).await?;
        if notifications.len() > maximum {
            return Err(std::io::Error::other(format!(
                "user {user_id} received {} notifications; expected at most {maximum}",
                notifications.len()
            ))
            .into());
        }
        if Instant::now() >= deadline {
            return Ok(());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

fn assert_availability_change(
    notification: &WatchlistNotificationRow,
    old_availability: Option<&str>,
    new_availability: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!("WATCHLIST_AVAILABILITY_CHANGED", notification.kind);
    assert_eq!(
        old_availability,
        notification
            .payload
            .pointer("/change/old_availability")
            .and_then(serde_json::Value::as_str)
    );
    assert_eq!(
        new_availability,
        notification
            .payload
            .pointer("/change/new_availability")
            .and_then(serde_json::Value::as_str)
    );
    Ok(())
}

fn assert_price_change(
    notification: &WatchlistNotificationRow,
    currency: &str,
    old_amount: u64,
    new_amount: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!("WATCHLIST_PRICE_CHANGED", notification.kind);
    assert_eq!(
        Some(currency),
        notification
            .payload
            .pointer("/change/old_price/currency")
            .and_then(serde_json::Value::as_str)
    );
    assert_eq!(
        Some(old_amount),
        notification
            .payload
            .pointer("/change/old_price/amount")
            .and_then(serde_json::Value::as_u64)
    );
    assert_eq!(
        Some(currency),
        notification
            .payload
            .pointer("/change/new_price/currency")
            .and_then(serde_json::Value::as_str)
    );
    assert_eq!(
        Some(new_amount),
        notification
            .payload
            .pointer("/change/new_price/amount")
            .and_then(serde_json::Value::as_u64)
    );
    Ok(())
}
