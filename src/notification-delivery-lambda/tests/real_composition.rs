use aura_historia_jobs::{
    DomainJob, DomainJobPayload, IdempotencyKey, NotificationDeliveryCreatedJob, OrderingKey,
    WorkerQueue, encode,
};
use aws_lambda_events::sqs::{SqsBatchResponse, SqsEvent, SqsMessage};
use aws_sdk_s3::{
    Client as S3Client,
    config::Builder as S3ConfigBuilder,
    types::{BucketLocationConstraint, CreateBucketConfiguration},
};
use aws_sdk_sesv2::Client as SesClient;
use domain_primitives::event_id::EventId;
use lambda_runtime::{Context, LambdaEvent};
use listing_source_core::ListingSourceId;
use notification_core::{
    notification_delivery_id::NotificationDeliveryId, notification_id::NotificationId,
};
use notification_delivery_lambda::{compose_delivery_use_case, handler};
use notification_email_aws::{EmailDeliveryConfig, SesNotificationChannelSender};
use notification_postgres::{SqlxEmailDeliveryTargetReader, SqlxNotificationDeliveryRepository};
use notification_service::{
    ports::{
        notification_channel_sender::{NotificationChannelSender, NotificationDeliveryDispatcher},
        notification_delivery_repository::{
            ClaimNotificationDeliveryOutcome, NotificationDeliveryError,
            NotificationDeliveryRepository,
        },
    },
    use_cases::commands::deliver_notification::{
        DeliverNotificationHandler, DeliverNotificationUseCase,
    },
};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};
use test_api::{
    IntegrationTestService, Postgres, S3, Ses, aura_integration_test, get_postgres_client,
    get_sent_emails,
};
use time::OffsetDateTime;
use user_core::user_id::UserId;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[aura_integration_test(services = [BUSINESS_SCHEMA, S3(), Ses()])]
async fn active_postgres_lease_retains_work_then_redelivery_sends_once_and_duplicate_is_noop() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let use_case = composition(pool.clone(), false).await?;
        let (delivery, recipient) = seed_delivery(&pool, true).await?;
        let body = job_body(delivery)?;

        let first = handler(event("active-lease", body.clone()), use_case.as_ref()).await?;
        assert_eq!(failure_ids(first), ["active-lease"]);
        let active = snapshot(&pool, delivery).await?;
        assert_eq!("PROCESSING", active["status"]);
        assert_eq!(4, active["attempt_count"]);
        assert!(active["lease_token"].is_string());
        assert_eq!(0, email_count(&recipient).await);

        sqlx::query("UPDATE notification_deliveries SET lease_expires_at = now() - interval '1 second' WHERE notification_delivery_id = $1")
            .bind(delivery.as_uuid()).execute(&pool).await?;
        assert!(failure_ids(handler(event("expired-lease", body.clone()), use_case.as_ref()).await?).is_empty());
        let committed = snapshot(&pool, delivery).await?;
        assert_eq!("DELIVERED", committed["status"]);
        assert_eq!(5, committed["attempt_count"]);
        assert!(committed["provider_message_id"].is_string());
        assert!(committed["delivered_at"].is_string());
        assert!(committed["lease_token"].is_null());
        assert_eq!(1, email_count(&recipient).await);
        let email = get_sent_emails().await.into_iter().find(|email|
            email.destination.to_addresses.contains(&recipient)
        ).ok_or("SES delivery not captured")?;
        assert_eq!("Your watchlist item's availability changed", email.subject);
        assert!(email.body.html_part.as_deref().is_some_and(|html|
            html.contains("Delivery test source") && html.contains("In stock")
        ));

        assert!(failure_ids(handler(event("duplicate", body), use_case.as_ref()).await?).is_empty());
        assert_eq!(committed, snapshot(&pool, delivery).await?);
        assert_eq!(1, email_count(&recipient).await);
        Ok(())
    }.await;
    assert!(
        result.is_ok(),
        "real notification delivery lease test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, S3(), Ses()])]
async fn invalid_s3_template_finalizes_permanent_failure_without_email_or_resend() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let use_case = composition(pool.clone(), true).await?;
        let (delivery, recipient) = seed_delivery(&pool, false).await?;
        let body = job_body(delivery)?;
        assert!(
            failure_ids(handler(event("invalid-template", body.clone()), use_case.as_ref()).await?)
                .is_empty()
        );
        let failed = snapshot(&pool, delivery).await?;
        assert_eq!("FAILED", failed["status"]);
        assert_eq!(1, failed["attempt_count"]);
        assert_eq!("S3_TEMPLATE_INVALID_UTF8", failed["last_error_code"]);
        assert!(failed["provider_message_id"].is_null());
        assert!(failed["lease_token"].is_null());
        assert_eq!(0, email_count(&recipient).await);
        assert!(
            failure_ids(handler(event("duplicate", body), use_case.as_ref()).await?).is_empty()
        );
        assert_eq!(failed, snapshot(&pool, delivery).await?);
        assert_eq!(0, email_count(&recipient).await);
        Ok(())
    }
    .await;
    assert!(
        result.is_ok(),
        "real permanent delivery failure test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, S3(), Ses()])]
async fn finalization_retry_keeps_the_original_ses_receipt_without_resending() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let (s3, ses, email) = provider(false).await?;
        let fault = Arc::new(FinalizationFault::default());
        let dispatcher =
            NotificationDeliveryDispatcher::new(vec![Arc::new(SesNotificationChannelSender::new(
                s3,
                ses,
                email,
                Arc::new(SqlxEmailDeliveryTargetReader::new(pool.clone())),
            ))
                as Arc<dyn NotificationChannelSender>])?;
        let use_case = DeliverNotificationHandler::new(
            FailFirstFinalization {
                inner: SqlxNotificationDeliveryRepository::new(pool.clone()),
                fault: fault.clone(),
            },
            dispatcher,
        );
        let (delivery, recipient) = seed_delivery(&pool, false).await?;
        let body = job_body(delivery)?;
        assert!(failure_ids(handler(event("initial", body.clone()), &use_case).await?).is_empty());
        let stored = snapshot(&pool, delivery).await?;
        assert_eq!("DELIVERED", stored["status"]);
        assert_eq!(1, stored["attempt_count"]);
        let calls = fault.calls.lock().unwrap().clone();
        assert_eq!(
            2,
            calls.len(),
            "failed finalization must retry with the same completion"
        );
        assert_eq!(calls[0], calls[1]);
        assert_eq!(json!(calls[0].1), stored["provider_message_id"]);
        assert_eq!(1, email_count(&recipient).await);
        assert!(failure_ids(handler(event("duplicate", body), &use_case).await?).is_empty());
        assert_eq!(stored, snapshot(&pool, delivery).await?);
        assert_eq!(1, email_count(&recipient).await);
        Ok(())
    }
    .await;
    assert!(
        result.is_ok(),
        "real finalization retry test failed: {result:?}"
    );
}

#[derive(Default)]
struct FinalizationFault {
    failed: AtomicBool,
    calls: Mutex<Vec<(uuid::Uuid, String, OffsetDateTime)>>,
}

struct FailFirstFinalization {
    inner: SqlxNotificationDeliveryRepository,
    fault: Arc<FinalizationFault>,
}

#[async_trait::async_trait]
impl NotificationDeliveryRepository for FailFirstFinalization {
    async fn claim_and_load_source(
        &self,
        id: NotificationDeliveryId,
        now: OffsetDateTime,
        expires: OffsetDateTime,
        token: uuid::Uuid,
    ) -> Result<ClaimNotificationDeliveryOutcome, NotificationDeliveryError> {
        self.inner
            .claim_and_load_source(id, now, expires, token)
            .await
    }

    async fn mark_delivered(
        &self,
        id: NotificationDeliveryId,
        token: uuid::Uuid,
        provider_message_id: &str,
        completed_at: OffsetDateTime,
    ) -> Result<bool, NotificationDeliveryError> {
        self.fault.calls.lock().unwrap().push((
            token,
            provider_message_id.to_owned(),
            completed_at,
        ));
        if !self.fault.failed.swap(true, Ordering::SeqCst) {
            return Err(NotificationDeliveryError::OperationFailed {
                source: Box::new(std::io::Error::other("test-only finalization failure")),
            });
        }
        self.inner
            .mark_delivered(id, token, provider_message_id, completed_at)
            .await
    }

    async fn mark_retryable_failure(
        &self,
        id: NotificationDeliveryId,
        token: uuid::Uuid,
        error_code: &str,
        completed_at: OffsetDateTime,
    ) -> Result<bool, NotificationDeliveryError> {
        self.inner
            .mark_retryable_failure(id, token, error_code, completed_at)
            .await
    }

    async fn mark_permanent_failure(
        &self,
        id: NotificationDeliveryId,
        token: uuid::Uuid,
        error_code: &str,
        completed_at: OffsetDateTime,
    ) -> Result<bool, NotificationDeliveryError> {
        self.inner
            .mark_permanent_failure(id, token, error_code, completed_at)
            .await
    }
}

async fn composition(
    pool: sqlx::PgPool,
    invalid_template: bool,
) -> Result<Arc<dyn DeliverNotificationUseCase>, Box<dyn std::error::Error + Send + Sync>> {
    let (s3, ses, email) = provider(invalid_template).await?;
    compose_delivery_use_case(pool, s3, ses, email)
}

async fn provider(
    invalid_template: bool,
) -> Result<(S3Client, SesClient, EmailDeliveryConfig), Box<dyn std::error::Error + Send + Sync>> {
    let aws = test_api::localstack::get_aws_config().await;
    let s3 = S3Client::from_conf(S3ConfigBuilder::from(aws).force_path_style(true).build());
    let bucket = format!("lambda-delivery-{}", uuid::Uuid::new_v4());
    let stage = "test";
    let commit = uuid::Uuid::new_v4().simple().to_string();
    s3.create_bucket()
        .bucket(&bucket)
        .create_bucket_configuration(
            CreateBucketConfiguration::builder()
                .location_constraint(BucketLocationConstraint::EuCentral1)
                .build(),
        )
        .send()
        .await?;
    let template = if invalid_template {
        vec![0xff]
    } else {
        b"<html><body>{{listing_source_name}} {{first_name}} {{old_availability}} {{new_availability}} <a href=\"{{view_url}}\">View</a></body></html>".to_vec()
    };
    s3.put_object()
        .bucket(&bucket)
        .key(format!(
            "{stage}/{commit}/mjml/watchlist/product-update/availability/en.html"
        ))
        .body(template.into())
        .send()
        .await?;
    Ok((
        s3,
        SesClient::new(aws),
        EmailDeliveryConfig::new(
            bucket,
            "no-reply@notify.aura-historia.test",
            "contact@aura-historia.test",
            stage.to_owned(),
            commit,
        ),
    ))
}

async fn seed_delivery(
    pool: &sqlx::PgPool,
    active: bool,
) -> Result<(NotificationDeliveryId, String), Box<dyn std::error::Error + Send + Sync>> {
    let user = UserId::new();
    let notification = NotificationId::new();
    let delivery = NotificationDeliveryId::new();
    let recipient = format!("lambda-delivery-{delivery}@example.test");
    let mut tx = pool.begin().await?;
    sqlx::query("INSERT INTO users (user_id, email, language, show_unassessed_or_sensitive_content, tier, role) VALUES ($1, $2, 'en', false, 'ULTIMATE', 'USER')")
        .bind(user.as_uuid()).bind(&recipient).execute(&mut *tx).await?;
    let payload = json!({
        "type": "WATCHLIST",
        "snapshot": {
            "listing_source_id": ListingSourceId::new().as_uuid().to_string(),
            "source_listing_id": "lambda-notification-product",
            "listing_source_slug_id": "lambda-delivery-source",
            "product_listing_title_slug_id": "lambda-delivery-product-abcdef",
            "listing_source_name": "Delivery test source",
            "title": null, "image": null, "content_policy": null,
            "url": "https://example.test/product",
            "view_url": "https://aura-historia.com/product-listings/lambda-delivery-product-abcdef"
        },
        "change": {"type": "AVAILABILITY_CHANGE", "old_availability": "AVAILABLE", "new_availability": "IN_STOCK"}
    });
    sqlx::query("INSERT INTO notifications (notification_id, user_id, kind, origin_event_id, product_listing_id, payload_version, payload, seen) VALUES ($1, $2, 'WATCHLIST_AVAILABILITY_CHANGED', $3, $4, 1, $5, false)")
        .bind(notification.as_uuid()).bind(user.as_uuid()).bind(EventId::new().as_uuid())
        .bind(uuid::Uuid::now_v7()).bind(payload).execute(&mut *tx).await?;
    if active {
        sqlx::query("INSERT INTO notification_deliveries (notification_delivery_id, notification_id, channel, target_key, status, attempt_count, lease_token, lease_expires_at) VALUES ($1, $2, 'EMAIL', 'PRIMARY', 'PROCESSING', 4, $3, now() + interval '1 hour')")
            .bind(delivery.as_uuid()).bind(notification.as_uuid()).bind(uuid::Uuid::new_v4())
            .execute(&mut *tx).await?;
    } else {
        sqlx::query("INSERT INTO notification_deliveries (notification_delivery_id, notification_id, channel, target_key) VALUES ($1, $2, 'EMAIL', 'PRIMARY')")
            .bind(delivery.as_uuid()).bind(notification.as_uuid()).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok((delivery, recipient))
}

async fn snapshot(
    pool: &sqlx::PgPool,
    delivery: NotificationDeliveryId,
) -> Result<Value, sqlx::Error> {
    sqlx::query_scalar("SELECT to_jsonb(d) || jsonb_build_object('tuple_version', xmin::text) FROM notification_deliveries d WHERE notification_delivery_id = $1")
        .bind(delivery.as_uuid()).fetch_one(pool).await
}

async fn email_count(recipient: &str) -> usize {
    get_sent_emails()
        .await
        .iter()
        .filter(|email| {
            email
                .destination
                .to_addresses
                .iter()
                .any(|address| address == recipient)
        })
        .count()
}

fn job_body(delivery: NotificationDeliveryId) -> Result<String, aura_historia_jobs::WireError> {
    let key = format!("notification-delivery:{delivery}");
    encode(&DomainJob {
        target_queue: WorkerQueue::NotificationDelivery,
        idempotency_key: IdempotencyKey::new(&key),
        ordering_key: OrderingKey::new(key),
        payload: DomainJobPayload::<aura_historia_jobs::SearchFilterOperation>::NotificationDeliveryCreated(
            NotificationDeliveryCreatedJob { notification_delivery_id: delivery }
        ),
    })
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
