use aura_historia_worker::cdc::WorkerQueue;
use aura_historia_worker::watchlist_notifications::consume_watchlist_notification_queue;
use aura_historia_worker::{QueueConfig, WorkerRunError, WorkerRuntime, serve_with_runtime};
use common::currency::domain::Currency;
use common::event_id::EventId;
use common::postgres::SqlxUnitOfWork;
use common::product_id::ProductId;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::user_id::UserId;
use notification_core::notification::{NotificationPayload, NotificationWatchlistPayload};
use notification_dynamodb::{
    all_notifications_reader::DynamoDbAllNotificationsReader,
    conditional_writer::ConditionalDynamoDbNotificationWriter,
};
use notification_service::ports::all_notifications_reader::{
    AllNotificationsReadItem, AllNotificationsReader,
};
use notification_service::use_cases::commands::create_notification::CreateNotificationHandler;
use product_postgres::SqlxProductWatchlistNotificationSourceReaderFactory;
use product_service::use_cases::{
    GenerateWatchlistNotificationsHandler, GenerateWatchlistNotificationsUseCase,
};
use serde_json::json;
use test_api::{
    DynamoDB, IntegrationTestService, Postgres, Sequin, aura_integration_test, get_dynamodb_client,
    get_or_start_worker_webhook_sequin, get_postgres_client, get_sequin_worker_webhook_bind_addr,
};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use watchlist_postgres::SqlxWatchlistNotificationRecipientReaderFactory;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
const POLL_INTERVAL: Duration = Duration::from_millis(200);
const POLL_ATTEMPTS: usize = 80;
const NO_NOTIFICATION_OBSERVATION: Duration = Duration::from_secs(2);

#[aura_integration_test(services = [BUSINESS_SCHEMA, DynamoDB(), Sequin::worker_webhook()])]
async fn should_create_state_notification_from_committed_product_event() {
    let result = create_state_notification_from_committed_product_event().await;

    assert!(
        result.is_ok(),
        "state notification acceptance test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DynamoDB(), Sequin::worker_webhook()])]
async fn should_create_price_notifications_only_for_active_watchers() {
    let result = create_price_notifications_only_for_active_watchers().await;

    assert!(
        result.is_ok(),
        "price notification acceptance test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DynamoDB(), Sequin::worker_webhook()])]
async fn should_preserve_one_notification_when_product_event_delivery_is_retried() {
    let result = preserve_one_notification_when_product_event_delivery_is_retried().await;

    assert!(
        result.is_ok(),
        "duplicate delivery acceptance test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DynamoDB(), Sequin::worker_webhook()])]
async fn should_not_notify_for_rolled_back_or_unrouted_product_events() {
    let result = not_notify_for_rolled_back_or_unrouted_product_events().await;

    assert!(
        result.is_ok(),
        "non-notification event acceptance test failed: {result:?}"
    );
}

async fn create_state_notification_from_committed_product_event()
-> Result<(), Box<dyn std::error::Error>> {
    let worker = WatchlistWorker::start().await?;
    let result = async {
        let user_id = seed_user(&worker.pool, "state-recipient").await?;
        let product_id = seed_product(&worker.pool).await?;
        seed_watchlist(&worker.pool, user_id, product_id, true, "Active").await?;
        let event_id = insert_product_event(
            &worker.pool,
            product_id,
            "PRODUCT_STATE_CHANGED",
            json!({"oldState": "Available", "newState": "Sold"}),
        )
        .await?;

        let notifications = wait_for_notifications(&worker.notifications, user_id, 1).await?;
        assert_eq!(event_id, notifications[0].origin_event_id);
        assert!(notifications[0].external);
        assert_state_change(&notifications[0], "Available", "Sold")
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
        let product_id = seed_product(&worker.pool).await?;
        seed_watchlist(&worker.pool, email_recipient, product_id, true, "Active").await?;
        seed_watchlist(&worker.pool, in_app_recipient, product_id, false, "Active").await?;
        seed_watchlist(
            &worker.pool,
            inactive_recipient,
            product_id,
            true,
            "InactiveByUser",
        )
        .await?;
        let event_id = insert_product_event(
            &worker.pool,
            product_id,
            "PRODUCT_PRICE_CHANGED",
            json!({
                "oldPricing": {"price": {"amount": 1200, "currency": "EUR"}},
                "newPricing": {"price": {"amount": 900, "currency": "EUR"}}
            }),
        )
        .await?;

        let email_notifications =
            wait_for_notifications(&worker.notifications, email_recipient, 1).await?;
        let in_app_notifications =
            wait_for_notifications(&worker.notifications, in_app_recipient, 1).await?;
        assert_eq!(event_id, email_notifications[0].origin_event_id);
        assert!(email_notifications[0].external);
        assert!(!in_app_notifications[0].external);
        assert_price_change(&email_notifications[0], 1200, 900)?;
        assert_no_notifications_for(
            &worker.notifications,
            inactive_recipient,
            NO_NOTIFICATION_OBSERVATION,
        )
        .await
    }
    .await;

    worker.finish(result).await
}

async fn preserve_one_notification_when_product_event_delivery_is_retried()
-> Result<(), Box<dyn std::error::Error>> {
    let worker = WatchlistWorker::start().await?;
    let result = async {
        let user_id = seed_user(&worker.pool, "duplicate-recipient").await?;
        let product_id = seed_product(&worker.pool).await?;
        seed_watchlist(&worker.pool, user_id, product_id, true, "Active").await?;
        let event_id = insert_product_event(
            &worker.pool,
            product_id,
            "PRODUCT_STATE_CHANGED",
            json!({"oldState": "Listed", "newState": "Available"}),
        )
        .await?;
        let _ = wait_for_notifications(&worker.notifications, user_id, 1).await?;

        let response = reqwest::Client::new()
            .post(format!(
                "http://{}/cdc/sequin",
                get_sequin_worker_webhook_bind_addr()
            ))
            .json(&json!({
                "record": {
                    "event_id": event_id.to_string(),
                    "product_id": product_id.to_string(),
                    "event_type": "PRODUCT_STATE_CHANGED",
                    "event_group": "DOMAIN"
                },
                "action": "insert",
                "metadata": {"table_schema": "public", "table_name": "product_events"}
            }))
            .send()
            .await?;
        assert_eq!(reqwest::StatusCode::ACCEPTED, response.status());
        assert_no_more_than_notifications(
            &worker.notifications,
            user_id,
            1,
            NO_NOTIFICATION_OBSERVATION,
        )
        .await
    }
    .await;

    worker.finish(result).await
}

async fn not_notify_for_rolled_back_or_unrouted_product_events()
-> Result<(), Box<dyn std::error::Error>> {
    let worker = WatchlistWorker::start().await?;
    let result = async {
        let user_id = seed_user(&worker.pool, "absence-recipient").await?;
        let product_id = seed_product(&worker.pool).await?;
        seed_watchlist(&worker.pool, user_id, product_id, true, "Active").await?;
        insert_product_event_then_rollback(
            &worker.pool,
            product_id,
            "PRODUCT_STATE_CHANGED",
            json!({"oldState": "Available", "newState": "Sold"}),
        )
        .await?;
        insert_product_event(
            &worker.pool,
            product_id,
            "PRODUCT_URL_CHANGED",
            json!({"oldUrl": "https://example.test/old", "newUrl": "https://example.test/new"}),
        )
        .await?;

        assert_no_notifications_for(&worker.notifications, user_id, NO_NOTIFICATION_OBSERVATION)
            .await
    }
    .await;

    worker.finish(result).await
}

struct WatchlistWorker {
    pool: sqlx::PgPool,
    notifications: DynamoDbAllNotificationsReader<'static>,
    consumer: JoinHandle<()>,
    shutdown_tx: oneshot::Sender<()>,
    server: JoinHandle<Result<(), WorkerRunError>>,
}

impl WatchlistWorker {
    async fn start() -> Result<Self, Box<dyn std::error::Error>> {
        let pool = get_postgres_client().await;
        let dynamodb = get_dynamodb_client().await;
        let handler: Arc<dyn GenerateWatchlistNotificationsUseCase> =
            Arc::new(GenerateWatchlistNotificationsHandler::new(
                SqlxUnitOfWork::new(pool.clone()),
                SqlxProductWatchlistNotificationSourceReaderFactory::new(),
                SqlxWatchlistNotificationRecipientReaderFactory,
                CreateNotificationHandler::new(ConditionalDynamoDbNotificationWriter::new(
                    dynamodb.clone(),
                    "table_1",
                )),
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
        let _sequin = get_or_start_worker_webhook_sequin().await;

        Ok(Self {
            pool,
            notifications: DynamoDbAllNotificationsReader::new(dynamodb, "table_1"),
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
        "INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'Ultimate', 'User')",
    )
    .bind(uuid::Uuid::from(user_id))
    .bind(format!("worker-watchlist-{label}-{user_id}@example.test"))
    .execute(pool)
    .await?;
    Ok(user_id)
}

async fn seed_product(pool: &sqlx::PgPool) -> Result<ProductId, sqlx::Error> {
    let product_id = ProductId::new();
    let product_uuid = uuid::Uuid::from(product_id);
    let event_id = EventId::new();
    let shop_id = uuid::Uuid::new_v4();
    let product_slug_suffix = product_uuid.simple().to_string()[..6].to_owned();
    let mut tx = pool.begin().await?;
    sqlx::query("INSERT INTO shops (shop_id, shop_slug_id, name, shop_type, partner_status, shop_domains) VALUES ($1, $2, $3, 'COMMERCIAL_DEALER', 'SCRAPED', '{}')")
        .bind(shop_id)
        .bind(format!("worker-watchlist-shop-{shop_id}"))
        .bind("Worker watchlist shop")
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO products (product_id, product_slug_id, event_id, shop_id, seller_id, shops_product_id, title_text, title_language, state, lifecycle, url, product_images) VALUES ($1, $2, $3, $4, $4, $5, 'Worker watchlist product', 'en', 'Listed', 'Active', 'https://example.test/product', '[]')")
        .bind(product_uuid)
        .bind(format!("worker-watchlist-product-{product_slug_suffix}"))
        .bind(uuid::Uuid::from(event_id))
        .bind(shop_id)
        .bind(shop_id)
        .bind(product_uuid.to_string())
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO product_events (event_id, product_id, event_type, event_group, payload, event_time) VALUES ($1, $2, 'PRODUCT_CREATED', 'DOMAIN', '{}', now())")
        .bind(uuid::Uuid::from(event_id))
        .bind(product_uuid)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(product_id)
}

async fn seed_watchlist(
    pool: &sqlx::PgPool,
    user_id: UserId,
    product_id: ProductId,
    notifications: bool,
    state: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO product_watchlist (user_id, product_id, notifications, state) VALUES ($1, $2, $3, $4)")
        .bind(uuid::Uuid::from(user_id))
        .bind(uuid::Uuid::from(product_id))
        .bind(notifications)
        .bind(state)
        .execute(pool)
        .await?;
    Ok(())
}

async fn insert_product_event(
    pool: &sqlx::PgPool,
    product_id: ProductId,
    event_type: &str,
    payload: serde_json::Value,
) -> Result<EventId, sqlx::Error> {
    let event_id = EventId::new();
    sqlx::query("INSERT INTO product_events (event_id, product_id, event_type, event_group, payload, event_time) VALUES ($1, $2, $3, 'DOMAIN', $4, now())")
        .bind(uuid::Uuid::from(event_id))
        .bind(uuid::Uuid::from(product_id))
        .bind(event_type)
        .bind(payload)
        .execute(pool)
        .await?;
    Ok(event_id)
}

async fn insert_product_event_then_rollback(
    pool: &sqlx::PgPool,
    product_id: ProductId,
    event_type: &str,
    payload: serde_json::Value,
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    let event_id = EventId::new();
    sqlx::query("INSERT INTO product_events (event_id, product_id, event_type, event_group, payload, event_time) VALUES ($1, $2, $3, 'DOMAIN', $4, now())")
        .bind(uuid::Uuid::from(event_id))
        .bind(uuid::Uuid::from(product_id))
        .bind(event_type)
        .bind(payload)
        .execute(&mut *tx)
        .await?;
    drop(tx);
    Ok(())
}

async fn wait_for_notifications(
    reader: &DynamoDbAllNotificationsReader<'_>,
    user_id: UserId,
    expected: usize,
) -> Result<Vec<AllNotificationsReadItem>, Box<dyn std::error::Error>> {
    for _ in 0..POLL_ATTEMPTS {
        let notifications = reader.list_all_by_user(&user_id).await?;
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
    reader: &DynamoDbAllNotificationsReader<'_>,
    user_id: UserId,
    duration: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    assert_no_more_than_notifications(reader, user_id, 0, duration).await
}

async fn assert_no_more_than_notifications(
    reader: &DynamoDbAllNotificationsReader<'_>,
    user_id: UserId,
    maximum: usize,
    duration: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = Instant::now() + duration;
    loop {
        let notifications = reader.list_all_by_user(&user_id).await?;
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

fn assert_state_change(
    notification: &AllNotificationsReadItem,
    old_state: &str,
    new_state: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let NotificationPayload::Watchlist {
        watchlist_payload:
            NotificationWatchlistPayload::StateChange {
                old_state: actual_old,
                new_state: actual_new,
            },
        ..
    } = &notification.notification_payload
    else {
        return Err(std::io::Error::other("notification was not a watchlist state change").into());
    };
    assert_eq!(old_state, format!("{actual_old:?}"));
    assert_eq!(new_state, format!("{actual_new:?}"));
    Ok(())
}

fn assert_price_change(
    notification: &AllNotificationsReadItem,
    old_amount: u64,
    new_amount: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let NotificationPayload::Watchlist {
        watchlist_payload:
            NotificationWatchlistPayload::PriceChange {
                old_price,
                new_price,
            },
        ..
    } = &notification.notification_payload
    else {
        return Err(std::io::Error::other("notification was not a watchlist price change").into());
    };
    assert_eq!(
        Some(&common::price::domain::MonetaryAmount::from(old_amount)),
        old_price.get(&Currency::Eur)
    );
    assert_eq!(
        Some(&common::price::domain::MonetaryAmount::from(new_amount)),
        new_price.get(&Currency::Eur)
    );
    Ok(())
}
