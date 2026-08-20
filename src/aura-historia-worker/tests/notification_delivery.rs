use aura_historia_worker::cdc::WorkerQueue;
use aura_historia_worker::notification_delivery::consume_notification_delivery_queue;
use aura_historia_worker::{QueueConfig, WorkerRunError, WorkerRuntime, serve_with_runtime};
use aws_sdk_s3::{
    Client as S3Client,
    config::Builder as S3ConfigBuilder,
    types::{BucketLocationConstraint, CreateBucketConfiguration},
};
use aws_sdk_sesv2::Client as SesClient;
use notification_core::{
    notification_delivery::NotificationDeliveryChannel,
    notification_delivery_id::NotificationDeliveryId,
};
use notification_email_aws::{EmailDeliveryConfig, SesNotificationChannelSender};
use notification_postgres::SqlxEmailDeliveryTargetReader;
use notification_postgres::SqlxNotificationDeliveryRepository;
use notification_service::{
    ports::notification_channel_sender::{
        NotificationChannelSendError, NotificationChannelSender, NotificationDeliveryDispatcher,
        SentNotificationDelivery,
    },
    use_cases::commands::deliver_notification::{
        DeliverNotificationHandler, DeliverNotificationUseCase,
    },
};
use serde_json::json;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use test_api::{
    IntegrationTestService, Postgres, S3, Sequin, Ses, aura_integration_test, get_postgres_client,
    get_sent_emails, get_sequin_worker_webhook_bind_addr,
};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
const WORKER_SEQUIN: Sequin = Sequin::worker_webhook();
const POLL_INTERVAL: Duration = Duration::from_millis(200);
const POLL_ATTEMPTS: usize = 80;
const NO_SIDE_EFFECT_OBSERVATION: Duration = Duration::from_secs(2);

#[aura_integration_test(services = [BUSINESS_SCHEMA, S3(), Ses(), WORKER_SEQUIN])]
async fn should_deliver_committed_notification_delivery_and_persist_result() {
    let result = deliver_committed_notification_delivery().await;

    assert!(
        result.is_ok(),
        "notification delivery insert acceptance test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, S3(), Ses(), WORKER_SEQUIN])]
async fn should_not_deliver_rolled_back_or_unsupported_cdc_changes() {
    let result = reject_rolled_back_or_unsupported_changes().await;

    assert!(
        result.is_ok(),
        "notification delivery rollback/filter acceptance test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, S3(), Ses(), WORKER_SEQUIN])]
async fn should_not_send_again_when_notification_delivery_is_redelivered() {
    let result = deduplicate_redelivered_notification_delivery().await;

    assert!(
        result.is_ok(),
        "notification delivery duplicate acceptance test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, S3(), Ses(), WORKER_SEQUIN])]
async fn should_deliver_after_expired_lease_is_redelivered() {
    let result = redeliver_after_expired_lease().await;

    assert!(
        result.is_ok(),
        "notification delivery lease/redelivery acceptance test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, S3(), Ses(), WORKER_SEQUIN])]
async fn should_persist_permanent_failure_when_template_is_invalid() {
    let result = persist_permanent_template_failure().await;

    assert!(
        result.is_ok(),
        "notification delivery failure acceptance test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, S3(), Ses(), WORKER_SEQUIN])]
async fn should_deliver_persisted_push_delivery_through_generic_cdc_flow() {
    let result = deliver_persisted_push_delivery().await;

    assert!(
        result.is_ok(),
        "notification PUSH delivery acceptance test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, S3(), Ses(), WORKER_SEQUIN])]
async fn should_clear_retry_failure_state_when_a_retry_succeeds() {
    let result = clear_retry_failure_state_after_successful_delivery().await;

    assert!(
        result.is_ok(),
        "notification delivery retry-success acceptance test failed: {result:?}"
    );
}

async fn deliver_committed_notification_delivery() -> Result<(), Box<dyn std::error::Error>> {
    let worker = NotificationDeliveryWorker::start(Template::Valid).await?;
    let result = async {
        let delivery = insert_delivery(&worker.pool, DeliveryState::Pending).await?;

        let persisted = wait_for_delivery(&worker.pool, delivery.delivery_id, "DELIVERED").await?;
        assert_eq!(1, persisted.attempt_count);
        assert!(persisted.provider_message_id.is_some());
        assert!(persisted.delivered_at.is_some());
        assert!(persisted.last_error_code.is_none());
        assert!(persisted.lease_token.is_none());
        assert!(persisted.lease_expires_at.is_none());

        let email = wait_for_email_to(&delivery.recipient_email).await?;
        assert_eq!(
            vec![delivery.recipient_email],
            email.destination.to_addresses
        );
        assert_eq!("Your watchlist item changed", email.subject);
        assert!(
            email
                .body
                .html_part
                .as_deref()
                .is_some_and(|body| body.contains("Delivery test shop"))
        );
        Ok(())
    }
    .await;

    worker.finish(result).await
}

async fn reject_rolled_back_or_unsupported_changes() -> Result<(), Box<dyn std::error::Error>> {
    let worker = NotificationDeliveryWorker::start(Template::Valid).await?;
    let result = async {
        let mut transaction = worker.pool.begin().await?;
        let rolled_back =
            insert_delivery_in_transaction(&mut transaction, DeliveryState::Pending).await?;
        drop(transaction);

        tokio::time::sleep(NO_SIDE_EFFECT_OBSERVATION).await;
        assert!(
            delivery_row(&worker.pool, rolled_back.delivery_id)
                .await?
                .is_none()
        );
        assert_no_email_to(&rolled_back.recipient_email).await?;

        let wrong_table = post_cdc_change(
            rolled_back.delivery_id,
            rolled_back.notification_id,
            "notifications",
            "insert",
        )
        .await?;
        assert_eq!(reqwest::StatusCode::SERVICE_UNAVAILABLE, wrong_table);

        let wrong_operation = post_cdc_change(
            rolled_back.delivery_id,
            rolled_back.notification_id,
            "notification_deliveries",
            "update",
        )
        .await?;
        assert_eq!(reqwest::StatusCode::SERVICE_UNAVAILABLE, wrong_operation);
        assert_no_email_to(&rolled_back.recipient_email).await
    }
    .await;

    worker.finish(result).await
}

async fn deduplicate_redelivered_notification_delivery() -> Result<(), Box<dyn std::error::Error>> {
    let worker = NotificationDeliveryWorker::start(Template::Valid).await?;
    let result = async {
        let delivery = insert_delivery(&worker.pool, DeliveryState::Pending).await?;
        let initial = wait_for_delivery(&worker.pool, delivery.delivery_id, "DELIVERED").await?;
        let initial_message_id = initial
            .provider_message_id
            .clone()
            .ok_or_else(|| std::io::Error::other("delivered row has no provider message id"))?;
        let _ = wait_for_email_to(&delivery.recipient_email).await?;

        let response = post_cdc_change(
            delivery.delivery_id,
            delivery.notification_id,
            "notification_deliveries",
            "insert",
        )
        .await?;
        assert_eq!(reqwest::StatusCode::ACCEPTED, response);

        tokio::time::sleep(NO_SIDE_EFFECT_OBSERVATION).await;
        let persisted = delivery_row(&worker.pool, delivery.delivery_id)
            .await?
            .ok_or_else(|| std::io::Error::other("delivered row disappeared"))?;
        assert_eq!("DELIVERED", persisted.status);
        assert_eq!(1, persisted.attempt_count);
        assert_eq!(Some(initial_message_id), persisted.provider_message_id);
        assert_email_count_for(&delivery.recipient_email, 1).await
    }
    .await;

    worker.finish(result).await
}

async fn redeliver_after_expired_lease() -> Result<(), Box<dyn std::error::Error>> {
    let worker = NotificationDeliveryWorker::start(Template::Valid).await?;
    let result = async {
        let delivery = insert_delivery(&worker.pool, DeliveryState::ActiveLease).await?;

        tokio::time::sleep(NO_SIDE_EFFECT_OBSERVATION).await;
        let claimed = delivery_row(&worker.pool, delivery.delivery_id)
            .await?
            .ok_or_else(|| std::io::Error::other("leased delivery row disappeared"))?;
        assert_eq!("PROCESSING", claimed.status);
        assert_eq!(4, claimed.attempt_count);
        assert!(claimed.lease_token.is_some());
        assert!(claimed.lease_expires_at.is_some());
        assert_no_email_to(&delivery.recipient_email).await?;

        sqlx::query(
            "UPDATE notification_deliveries SET lease_expires_at = now() - interval '1 second' WHERE notification_delivery_id = $1",
        )
        .bind(delivery.delivery_id)
        .execute(&worker.pool)
        .await?;

        let response = post_cdc_change(delivery.delivery_id, delivery.notification_id, "notification_deliveries", "insert").await?;
        assert_eq!(reqwest::StatusCode::ACCEPTED, response);

        let persisted = wait_for_delivery(&worker.pool, delivery.delivery_id, "DELIVERED").await?;
        assert_eq!(5, persisted.attempt_count);
        assert!(persisted.provider_message_id.is_some());
        assert_email_count_for(&delivery.recipient_email, 1).await
    }
    .await;

    worker.finish(result).await
}

async fn deliver_persisted_push_delivery() -> Result<(), Box<dyn std::error::Error>> {
    let (worker, sender) = NotificationDeliveryWorker::start_push().await?;
    let result = async {
        let delivery =
            insert_delivery_for_channel(&worker.pool, DeliveryState::Pending, "PUSH").await?;

        let persisted = wait_for_delivery(&worker.pool, delivery.delivery_id, "DELIVERED").await?;
        assert_eq!(1, persisted.attempt_count);
        assert_eq!(
            Some("push-message-1".to_owned()),
            persisted.provider_message_id
        );
        assert_eq!(
            vec![NotificationDeliveryId::from(delivery.delivery_id)],
            sender
                .sent_delivery_ids
                .lock()
                .map_err(|_| std::io::Error::other("push sender state lock poisoned"))?
                .clone()
        );
        Ok(())
    }
    .await;

    worker.finish(result).await
}

async fn clear_retry_failure_state_after_successful_delivery()
-> Result<(), Box<dyn std::error::Error>> {
    let worker = NotificationDeliveryWorker::start(Template::Valid).await?;
    let result = async {
        let delivery =
            insert_delivery(&worker.pool, DeliveryState::PendingWithRetryableFailure).await?;

        let persisted = wait_for_delivery(&worker.pool, delivery.delivery_id, "DELIVERED").await?;
        assert_eq!(1, persisted.attempt_count);
        assert!(persisted.provider_message_id.is_some());
        assert!(persisted.delivered_at.is_some());
        assert!(persisted.last_error_code.is_none());
        assert!(persisted.lease_token.is_none());
        assert!(persisted.lease_expires_at.is_none());
        Ok(())
    }
    .await;

    worker.finish(result).await
}

async fn persist_permanent_template_failure() -> Result<(), Box<dyn std::error::Error>> {
    let worker = NotificationDeliveryWorker::start(Template::InvalidUtf8).await?;
    let result = async {
        let delivery = insert_delivery(&worker.pool, DeliveryState::Pending).await?;

        let persisted = wait_for_delivery(&worker.pool, delivery.delivery_id, "FAILED").await?;
        assert_eq!(1, persisted.attempt_count);
        assert_eq!(
            Some("S3_TEMPLATE_INVALID_UTF8".to_owned()),
            persisted.last_error_code
        );
        assert!(persisted.provider_message_id.is_none());
        assert!(persisted.delivered_at.is_none());
        assert!(persisted.lease_token.is_none());
        assert!(persisted.lease_expires_at.is_none());
        assert_no_email_to(&delivery.recipient_email).await
    }
    .await;

    worker.finish(result).await
}

struct NotificationDeliveryWorker {
    pool: sqlx::PgPool,
    consumer: JoinHandle<()>,
    shutdown_tx: oneshot::Sender<()>,
    server: JoinHandle<Result<(), WorkerRunError>>,
}

impl NotificationDeliveryWorker {
    async fn start(template: Template) -> Result<Self, Box<dyn std::error::Error>> {
        let targets = DeliveryTargets::create(template).await?;
        let pool = get_postgres_client().await;
        let aws_config = test_api::localstack::get_aws_config().await;
        let s3 = S3Client::from_conf(
            S3ConfigBuilder::from(aws_config)
                .force_path_style(true)
                .build(),
        );
        let handler: Arc<dyn DeliverNotificationUseCase> =
            Arc::new(DeliverNotificationHandler::new(
                SqlxNotificationDeliveryRepository::new(pool.clone()),
                NotificationDeliveryDispatcher::new(vec![Arc::new(
                    SesNotificationChannelSender::new(
                        s3,
                        SesClient::new(aws_config),
                        EmailDeliveryConfig::new(
                            targets.bucket,
                            "no-reply@notify.aura-historia.test",
                            "contact@aura-historia.test",
                            targets.stage,
                            targets.commit_sha,
                        ),
                        Arc::new(SqlxEmailDeliveryTargetReader::new(pool.clone())),
                    ),
                )
                    as Arc<dyn NotificationChannelSender>])?,
            ));
        let (runtime, mut receivers) =
            WorkerRuntime::with_notification_delivery_queue(QueueConfig::new(16))?;
        let receiver = receivers
            .take(WorkerQueue::NotificationDelivery)
            .ok_or_else(|| std::io::Error::other("notification delivery queue is missing"))?;
        let consumer = tokio::spawn(consume_notification_delivery_queue(receiver, handler));
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

    async fn start_push() -> Result<(Self, Arc<RecordingPushSender>), Box<dyn std::error::Error>> {
        let pool = get_postgres_client().await;
        let sender = Arc::new(RecordingPushSender::default());
        let handler: Arc<dyn DeliverNotificationUseCase> =
            Arc::new(DeliverNotificationHandler::new(
                SqlxNotificationDeliveryRepository::new(pool.clone()),
                NotificationDeliveryDispatcher::new(vec![
                    sender.clone() as Arc<dyn NotificationChannelSender>
                ])?,
            ));
        let (runtime, mut receivers) =
            WorkerRuntime::with_notification_delivery_queue(QueueConfig::new(16))?;
        let receiver = receivers
            .take(WorkerQueue::NotificationDelivery)
            .ok_or_else(|| std::io::Error::other("notification delivery queue is missing"))?;
        let consumer = tokio::spawn(consume_notification_delivery_queue(receiver, handler));
        let listener = tokio::net::TcpListener::bind(get_sequin_worker_webhook_bind_addr()).await?;
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server = tokio::spawn(serve_with_runtime(listener, runtime, async move {
            let _ = shutdown_rx.await;
        }));

        Ok((
            Self {
                pool,
                consumer,
                shutdown_tx,
                server,
            },
            sender,
        ))
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

#[derive(Default)]
struct RecordingPushSender {
    sent_delivery_ids: Mutex<Vec<NotificationDeliveryId>>,
}

#[async_trait::async_trait]
impl NotificationChannelSender for RecordingPushSender {
    fn channel(&self) -> NotificationDeliveryChannel {
        NotificationDeliveryChannel::Push
    }

    async fn send(
        &self,
        source: &notification_service::ports::notification_delivery_repository::NotificationDeliverySource,
    ) -> Result<SentNotificationDelivery, NotificationChannelSendError> {
        self.sent_delivery_ids
            .lock()
            .map_err(|_| NotificationChannelSendError::Permanent {
                code: "TEST_PUSH_SENDER_LOCK_FAILED",
                source: common::error::boxed::box_error(std::io::Error::other(
                    "push sender state lock poisoned",
                )),
            })?
            .push(source.notification_delivery_id);
        Ok(SentNotificationDelivery {
            provider_message_id: "push-message-1".to_owned(),
        })
    }
}

struct DeliveryTargets {
    bucket: String,
    stage: String,
    commit_sha: String,
}

impl DeliveryTargets {
    async fn create(template: Template) -> Result<Self, Box<dyn std::error::Error>> {
        let aws_config = test_api::localstack::get_aws_config().await;
        let s3 = S3Client::from_conf(
            S3ConfigBuilder::from(aws_config)
                .force_path_style(true)
                .build(),
        );
        let bucket = format!("worker-delivery-{}", uuid::Uuid::new_v4());
        let stage = "test".to_owned();
        let commit_sha = uuid::Uuid::new_v4().simple().to_string();
        s3.create_bucket()
            .bucket(&bucket)
            .create_bucket_configuration(
                CreateBucketConfiguration::builder()
                    .location_constraint(BucketLocationConstraint::EuCentral1)
                    .build(),
            )
            .send()
            .await?;
        s3.put_object()
            .bucket(&bucket)
            .key(format!(
                "{stage}/{commit_sha}/mjml/watchlist/product-update/state/en.html"
            ))
            .body(template.bytes().into())
            .send()
            .await?;
        Ok(Self {
            bucket,
            stage,
            commit_sha,
        })
    }
}

#[derive(Clone, Copy)]
enum Template {
    Valid,
    InvalidUtf8,
}

impl Template {
    fn bytes(self) -> Vec<u8> {
        match self {
            Self::Valid => {
                b"<html><body>{{shop_name}} <a href=\"{{view_url}}\">View</a></body></html>"
                    .to_vec()
            }
            Self::InvalidUtf8 => vec![0xff],
        }
    }
}

#[derive(Clone, Copy)]
enum DeliveryState {
    Pending,
    PendingWithRetryableFailure,
    ActiveLease,
}

struct NotificationDeliveryFixture {
    delivery_id: uuid::Uuid,
    notification_id: uuid::Uuid,
    recipient_email: String,
}

async fn insert_delivery(
    pool: &sqlx::PgPool,
    state: DeliveryState,
) -> Result<NotificationDeliveryFixture, sqlx::Error> {
    insert_delivery_for_channel(pool, state, "EMAIL").await
}

async fn insert_delivery_for_channel(
    pool: &sqlx::PgPool,
    state: DeliveryState,
    channel: &str,
) -> Result<NotificationDeliveryFixture, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    let delivery =
        insert_delivery_in_transaction_for_channel(&mut transaction, state, channel).await?;
    transaction.commit().await?;
    Ok(delivery)
}

async fn insert_delivery_in_transaction(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    state: DeliveryState,
) -> Result<NotificationDeliveryFixture, sqlx::Error> {
    insert_delivery_in_transaction_for_channel(transaction, state, "EMAIL").await
}

async fn insert_delivery_in_transaction_for_channel(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    state: DeliveryState,
    channel: &str,
) -> Result<NotificationDeliveryFixture, sqlx::Error> {
    let user_id = uuid::Uuid::new_v4();
    let notification_id = uuid::Uuid::new_v4();
    let delivery_id = uuid::Uuid::new_v4();
    let origin_event_id = uuid::Uuid::new_v4();
    let product_id = uuid::Uuid::new_v4();
    let recipient_email = format!("notification-delivery-{delivery_id}@example.test");

    sqlx::query(
        "INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'ULTIMATE', 'USER')",
    )
    .bind(user_id)
    .bind(&recipient_email)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        "INSERT INTO notifications (notification_id, user_id, kind, origin_event_id, product_id, payload_version, payload, seen) VALUES ($1, $2, 'WATCHLIST_STATE_CHANGED', $3, $4, 1, $5, false)",
    )
    .bind(notification_id)
    .bind(user_id)
    .bind(origin_event_id)
    .bind(product_id)
    .bind(notification_payload())
    .execute(&mut **transaction)
    .await?;

    match state {
        DeliveryState::Pending => {
            sqlx::query(
                "INSERT INTO notification_deliveries (notification_delivery_id, notification_id, channel, target_key) VALUES ($1, $2, $3, 'PRIMARY')",
            )
            .bind(delivery_id)
            .bind(notification_id)
            .bind(channel)
            .execute(&mut **transaction)
            .await?;
        }
        DeliveryState::PendingWithRetryableFailure => {
            sqlx::query(
                "INSERT INTO notification_deliveries (notification_delivery_id, notification_id, channel, target_key, last_error_code) VALUES ($1, $2, $3, 'PRIMARY', 'S3_TEMPLATE_FETCH_RETRYABLE')",
            )
            .bind(delivery_id)
            .bind(notification_id)
            .bind(channel)
            .execute(&mut **transaction)
            .await?;
        }
        DeliveryState::ActiveLease => {
            sqlx::query(
                "INSERT INTO notification_deliveries (notification_delivery_id, notification_id, channel, target_key, status, attempt_count, lease_token, lease_expires_at) VALUES ($1, $2, $3, 'PRIMARY', 'PROCESSING', 4, $4, now() + interval '1 hour')",
            )
            .bind(delivery_id)
            .bind(notification_id)
            .bind(channel)
            .bind(uuid::Uuid::new_v4())
            .execute(&mut **transaction)
            .await?;
        }
    }

    Ok(NotificationDeliveryFixture {
        delivery_id,
        notification_id,
        recipient_email,
    })
}

fn notification_payload() -> serde_json::Value {
    json!({
        "type": "WATCHLIST",
        "snapshot": {
            "shop_id": uuid::Uuid::new_v4(),
            "shops_product_id": "worker-notification-delivery-product",
            "shop_slug_id": "worker-delivery-shop",
            "product_slug_id": "worker-delivery-product-abcdef",
            "shop_name": "Delivery test shop",
            "title": null,
            "image": null,
            "url": "https://example.test/products/delivery",
            "view_url": "https://aura-historia.test/products/delivery"
        },
        "change": {
            "type": "STATE_CHANGE",
            "old_state": "Listed",
            "new_state": "Available"
        }
    })
}

#[derive(sqlx::FromRow)]
struct DeliveryRow {
    status: String,
    attempt_count: i32,
    lease_token: Option<uuid::Uuid>,
    lease_expires_at: Option<time::OffsetDateTime>,
    provider_message_id: Option<String>,
    last_error_code: Option<String>,
    delivered_at: Option<time::OffsetDateTime>,
}

async fn wait_for_delivery(
    pool: &sqlx::PgPool,
    delivery_id: uuid::Uuid,
    expected_status: &str,
) -> Result<DeliveryRow, Box<dyn std::error::Error>> {
    for _ in 0..POLL_ATTEMPTS {
        if let Some(row) = delivery_row(pool, delivery_id).await?
            && row.status == expected_status
        {
            return Ok(row);
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err(std::io::Error::other(format!(
        "notification delivery {delivery_id} did not reach {expected_status}"
    ))
    .into())
}

async fn delivery_row(
    pool: &sqlx::PgPool,
    delivery_id: uuid::Uuid,
) -> Result<Option<DeliveryRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT status, attempt_count, lease_token, lease_expires_at, provider_message_id, last_error_code, delivered_at FROM notification_deliveries WHERE notification_delivery_id = $1",
    )
    .bind(delivery_id)
    .fetch_optional(pool)
    .await
}

async fn post_cdc_change(
    delivery_id: uuid::Uuid,
    notification_id: uuid::Uuid,
    table_name: &str,
    action: &str,
) -> Result<reqwest::StatusCode, reqwest::Error> {
    Ok(reqwest::Client::new()
        .post(format!(
            "http://{}/cdc/sequin",
            get_sequin_worker_webhook_bind_addr()
        ))
        .json(&json!({
            "record": {"notification_delivery_id": delivery_id, "notification_id": notification_id, "channel": "EMAIL", "status": "PENDING"},
            "action": action,
            "metadata": {"table_schema": "public", "table_name": table_name}
        }))
        .send()
        .await?
        .status())
}

async fn wait_for_email_to(
    recipient_email: &str,
) -> Result<test_api::SentEmail, Box<dyn std::error::Error>> {
    for _ in 0..POLL_ATTEMPTS {
        if let Some(email) = get_sent_emails().await.into_iter().find(|email| {
            email
                .destination
                .to_addresses
                .iter()
                .any(|address| address == recipient_email)
        }) {
            return Ok(email);
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err(std::io::Error::other(format!("no SES email sent to {recipient_email}")).into())
}

async fn assert_no_email_to(recipient_email: &str) -> Result<(), Box<dyn std::error::Error>> {
    if get_sent_emails().await.into_iter().any(|email| {
        email
            .destination
            .to_addresses
            .iter()
            .any(|address| address == recipient_email)
    }) {
        return Err(std::io::Error::other(format!(
            "unexpected SES email sent to {recipient_email}"
        ))
        .into());
    }
    Ok(())
}

async fn assert_email_count_for(
    recipient_email: &str,
    expected_count: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let actual_count = get_sent_emails()
        .await
        .into_iter()
        .filter(|email| {
            email
                .destination
                .to_addresses
                .iter()
                .any(|address| address == recipient_email)
        })
        .count();
    if actual_count != expected_count {
        return Err(std::io::Error::other(format!(
            "expected {expected_count} SES emails to {recipient_email}, found {actual_count}"
        ))
        .into());
    }
    Ok(())
}
