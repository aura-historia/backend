use application::transaction::{Transaction, UnitOfWork};
use aura_historia_jobs::{
    DomainJob, DomainJobPayload, IdempotencyKey, NotificationDeliveryCreatedJob, OrderingKey,
    WorkerQueue, encode,
};
use aura_historia_worker::scopes::notification_delivery::{
    NotificationDeliveryJobs, compose_delivery_use_case,
};
use aws_sdk_sesv2::Client as SesClient;
use content_notice_core::content_notice::ContentNotice;
use domain_primitives::event_id::EventId;
use listing_source_core::ListingSourceId;
use notification_core::{
    external_email::{
        ContentNoticeReceipt, ExternalEmailContent, ExternalEmailOrigin, ExternalEmailRecipient,
    },
    notification_delivery_id::NotificationDeliveryId,
    notification_id::NotificationId,
};
use notification_email_aws::{EmailDeliveryConfig, SesNotificationChannelSender};
use notification_postgres::{
    SqlxEmailDeliveryTargetReader, SqlxExternalEmailOutboxFactory,
    SqlxNotificationDeliveryRepository,
};
use notification_service::{
    ports::{
        external_email_outbox::{
            ExternalEmailOutbox, ExternalEmailOutboxFactory, ExternalEmailRequest,
        },
        notification_channel_sender::{NotificationChannelSender, NotificationDeliveryDispatcher},
        notification_delivery_repository::{
            ClaimNotificationDeliveryOutcome, NotificationDeliveryError,
            NotificationDeliveryRepository,
        },
    },
    use_cases::commands::deliver_notification::DeliverNotificationHandler,
};
use platform_postgres_queue::{Disposition, JobId};
use platform_worker::{Job, JobHandler, error_class};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use test_api::{
    IntegrationTestService, Postgres, Ses, aura_integration_test, get_postgres_client,
    get_sent_emails,
};
use time::OffsetDateTime;
use user_core::user_id::UserId;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[aura_integration_test(services = [BUSINESS_SCHEMA, Ses()], isolated = true)]
async fn active_postgres_lease_retains_work_then_redelivery_sends_once_and_duplicate_is_noop() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let handler = composition(pool.clone(), false).await?;
        let (delivery, recipient) = seed_delivery(&pool, true).await?;
        let body = job_body(delivery)?;

        assert_eq!(
            Disposition::Retry(error_class("already_claimed")),
            handler.handle(&claimed(1, body.clone())).await
        );
        let active = snapshot(&pool, delivery).await?;
        assert_eq!("PROCESSING", active["status"]);
        assert_eq!(4, active["attempt_count"]);
        assert!(active["lease_token"].is_string());
        assert_eq!(0, email_count(&recipient).await);

        sqlx::query("UPDATE notification_deliveries SET lease_expires_at = now() - interval '1 second' WHERE notification_delivery_id = $1")
            .bind(delivery.as_uuid()).execute(&pool).await?;
        assert_eq!(Disposition::Complete, handler.handle(&claimed(2, body.clone())).await);
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
        assert_eq!(
            format!(
                "\"Watchlist Notification | Aura Historia\" <{}>",
                test_api::TestContext::current().ok_or("delivery test context")?.sender_email()?
            ),
            email.source
        );
        assert!(email.body.html_part.as_deref().is_some_and(|html|
            html.contains("Delivery test source") && html.contains("In stock")
        ));

        assert_eq!(Disposition::Complete, handler.handle(&claimed(1, body)).await);
        assert_eq!(committed, snapshot(&pool, delivery).await?);
        assert_eq!(1, email_count(&recipient).await);
        Ok(())
    }.await;
    assert!(
        result.is_ok(),
        "real notification delivery lease test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, Ses()], isolated = true)]
async fn unrenderable_template_finalizes_permanent_failure_without_email_or_resend() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let handler = composition(pool.clone(), true).await?;
        let (delivery, recipient) = seed_delivery(&pool, false).await?;
        let body = job_body(delivery)?;
        assert_eq!(
            Disposition::Complete,
            handler.handle(&claimed(1, body.clone())).await
        );
        let failed = snapshot(&pool, delivery).await?;
        assert_eq!("FAILED", failed["status"]);
        assert_eq!(1, failed["attempt_count"]);
        assert_eq!("TEMPLATE_RENDER_FAILED", failed["last_error_code"]);
        assert!(failed["provider_message_id"].is_null());
        assert!(failed["lease_token"].is_null());
        assert_eq!(0, email_count(&recipient).await);
        assert_eq!(
            Disposition::Complete,
            handler.handle(&claimed(1, body)).await
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

#[aura_integration_test(services = [BUSINESS_SCHEMA, Ses()], isolated = true)]
async fn finalization_retry_keeps_the_original_ses_receipt_without_resending() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let (ses, email) = provider(false).await?;
        let fault = Arc::new(FinalizationFault::default());
        let dispatcher =
            NotificationDeliveryDispatcher::new(vec![Arc::new(SesNotificationChannelSender::new(
                ses,
                email,
                Arc::new(SqlxEmailDeliveryTargetReader::new(pool.clone())),
            )?)
                as Arc<dyn NotificationChannelSender>])?;
        let handler = NotificationDeliveryJobs::new(Arc::new(DeliverNotificationHandler::new(
            FailFirstFinalization {
                inner: SqlxNotificationDeliveryRepository::new(pool.clone()),
                fault: fault.clone(),
            },
            dispatcher,
        )));
        let (delivery, recipient) = seed_delivery(&pool, false).await?;
        let body = job_body(delivery)?;
        assert_eq!(
            Disposition::Complete,
            handler.handle(&claimed(1, body.clone())).await
        );
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
        assert_eq!(
            Disposition::Complete,
            handler.handle(&claimed(1, body)).await
        );
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

#[aura_integration_test(services = [BUSINESS_SCHEMA, Ses()], isolated = true)]
async fn content_notice_receipt_is_claimed_rendered_and_delivered_to_the_notifier_without_a_user() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let handler = composition(pool.clone(), false).await?;
        let notice = content_notice()?;
        let recipient = notice
            .submitter()
            .email
            .as_ref()
            .expect("fixture email")
            .as_str()
            .to_owned();
        let mut tx = platform_postgres::SqlxUnitOfWork::new(pool.clone()).begin().await?;
        let message = SqlxExternalEmailOutboxFactory::new()
            .in_transaction(&mut tx)
            .enqueue(ExternalEmailRequest {
                recipient: ExternalEmailRecipient::try_from(recipient.as_str())?,
                language: notice.submitter().language,
                content: ExternalEmailContent::ContentNoticeReceipt(
                    ContentNoticeReceipt::for_notice(
                        &notice,
                        time::macros::datetime!(2026-11-14 12:00 UTC),
                    ),
                ),
                origin: ExternalEmailOrigin::ContentNotice(notice.id()),
            })
            .await?
            .external_email_message_id();
        tx.commit().await?;
        let delivery = NotificationDeliveryId::try_from(
            sqlx::query_scalar::<_, uuid::Uuid>("SELECT notification_delivery_id FROM notification_deliveries WHERE external_email_message_id = $1")
                .bind(message.into_uuid())
                .fetch_one(&pool)
                .await?,
        )?;

        assert_eq!(0, sqlx::query_scalar::<_, i64>("SELECT count(*) FROM users").fetch_one(&pool).await?);

        let (logs, guard) = test_api::CapturedLogs::start();
        let disposition = handler.handle(&claimed(1, job_body(delivery)?)).await;
        drop(guard);

        assert_eq!(Disposition::Complete, disposition);
        assert_eq!(
            1,
            logs.events_where("message", "notification delivery claimed")
                .iter()
                .filter(|fields| fields["source_type"] == "external")
                .count()
        );
        let stored = snapshot(&pool, delivery).await?;
        assert_eq!("DELIVERED", stored["status"]);
        assert_eq!(1, stored["attempt_count"]);
        for private in [recipient.as_str(), "MarkerName", "MarkerExplanation"] {
            assert!(!logs.text().contains(private), "{private} must never be logged");
        }
        let email = get_sent_emails().await.into_iter().find(|email|
            email.destination.to_addresses.contains(&recipient)
        ).ok_or("SES delivery not captured")?;
        assert_eq!("Abbiamo ricevuto la sua segnalazione", email.subject);
        assert_eq!(
            format!(
                "\"Content Notice | Aura Historia\" <{}>",
                test_api::TestContext::current().ok_or("delivery test context")?.sender_email()?
            ),
            email.source
        );
        let html = email.body.html_part.ok_or("receipt has no HTML part")?;
        for expected in [
            notice.id().to_string().as_str(),
            "Antik Shop",
            "Jugendstil-Vase",
            "14 novembre 2026 alle 12:00 UTC",
            "Violazione di diritti",
            "https://stage.aura-historia.com/it",
            "Se non ha inviato questa segnalazione",
        ] {
            assert!(html.contains(expected), "receipt lacks {expected:?}");
        }
        for marker in ["MarkerName", "MarkerExplanation", "marker-url.example"] {
            assert!(!html.contains(marker), "{marker} reached the receipt");
        }
        Ok(())
    }
    .await;
    assert!(
        result.is_ok(),
        "real content notice receipt delivery test failed: {result:?}"
    );
}

/// A listing notice in Italian whose submitter-typed fields carry marker text.
fn content_notice() -> Result<ContentNotice, Box<dyn std::error::Error + Send + Sync>> {
    use content_notice_core::{
        additional_url::{AdditionalUrl, AdditionalUrls},
        content_notice::NewContentNotice,
        content_notice_id::ContentNoticeId,
        content_notice_target::{
            ContentNoticeTarget, ListingSourceSnapshot, ProductListingSnapshot,
            ProductListingTitleSnapshot, TargetSnapshot,
        },
        good_faith_statement_version::GoodFaithStatementVersion,
        notice_explanation::NoticeExplanation,
        reason_category::ReasonCategory,
        submitter::{Submitter, SubmitterEmail, SubmitterName},
        submitter_role::SubmitterRole,
    };
    use listing_source_core::ListingSourceName;
    use product_listing_core::product_listing_id::ProductListingId;

    let id = ContentNoticeId::new();
    Ok(ContentNotice::submit(NewContentNotice {
        id,
        target: ContentNoticeTarget::ProductListing {
            product_listing_id: ProductListingId::new(),
            listing_source_id: ListingSourceId::new(),
        },
        target_snapshot: TargetSnapshot {
            listing_source: ListingSourceSnapshot {
                name: ListingSourceName::try_from("Antik Shop")?,
                url: None,
                partner: false,
            },
            product_listing: Some(ProductListingSnapshot {
                title: Some(ProductListingTitleSnapshot::try_from("Jugendstil-Vase")?),
                url: url::Url::parse("https://antik.example/vase")?,
            }),
        },
        submitter_role: SubmitterRole::RightsHolder,
        reason_category: ReasonCategory::RightsInfringement,
        explanation: Some(NoticeExplanation::try_from("MarkerExplanation")?),
        additional_urls: AdditionalUrls::try_from(vec![AdditionalUrl::try_from(
            "https://marker-url.example/photo",
        )?])?,
        submitter: Submitter {
            name: Some(SubmitterName::try_from("MarkerName")?),
            email: Some(SubmitterEmail::try_from(format!(
                "notifier-{}@example.test",
                id.as_uuid().simple()
            ))?),
            language: localization::Language::It,
        },
        good_faith_statement_version: GoodFaithStatementVersion::GoodFaithV1,
    })?)
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
    unrenderable_template: bool,
) -> Result<NotificationDeliveryJobs, Box<dyn std::error::Error + Send + Sync>> {
    let (ses, email) = provider(unrenderable_template).await?;
    Ok(NotificationDeliveryJobs::new(compose_delivery_use_case(
        pool, ses, email,
    )?))
}

async fn provider(
    unrenderable_template: bool,
) -> Result<(SesClient, EmailDeliveryConfig), Box<dyn std::error::Error + Send + Sync>> {
    let aws = test_api::aws::get_aws_config().await;
    let context = test_api::TestContext::current().expect("delivery test context");
    let ses = SesClient::new(aws);
    let sender = context.sender_email()?;
    test_api::verify_sender_identity(&sender).await;
    let configuration_set = context.configuration_set_name("delivery").await?;
    ses.create_configuration_set()
        .configuration_set_name(&configuration_set)
        .send()
        .await?;
    let templates = test_api::mail_templates::mjml_template_root()?;
    if unrenderable_template {
        // Compiles at startup but fails when rendered: `lookup` needs two parameters.
        std::fs::write(
            templates.join("watchlist/product-update/availability/en.html"),
            "{{lookup}}",
        )?;
    }
    Ok((
        ses,
        EmailDeliveryConfig::new(
            templates,
            sender,
            "contact@aura-historia.test",
            configuration_set,
            "https://stage.aura-historia.com",
        )?,
    ))
}

async fn seed_delivery(
    pool: &sqlx::PgPool,
    active: bool,
) -> Result<(NotificationDeliveryId, String), Box<dyn std::error::Error + Send + Sync>> {
    let user = UserId::new();
    let notification = NotificationId::new();
    let delivery = NotificationDeliveryId::new();
    let recipient = format!("worker-delivery-{delivery}@example.test");
    let mut tx = pool.begin().await?;
    sqlx::query("INSERT INTO users (user_id, email, language, show_unassessed_or_sensitive_content, tier, role) VALUES ($1, $2, 'en', false, 'ULTIMATE', 'USER')")
        .bind(user.as_uuid()).bind(&recipient).execute(&mut *tx).await?;
    let payload = json!({
        "type": "WATCHLIST",
        "snapshot": {
            "listing_source_id": ListingSourceId::new().as_uuid().to_string(),
            "source_listing_id": "lambda-notification-product",
            "listing_source_slug_id": "worker-delivery-source",
            "product_listing_title_slug_id": "worker-delivery-product-abcdef",
            "listing_source_name": "Delivery test source",
            "title": null, "image": null, "content_policy": null,
            "url": "https://example.test/product",
            "view_url": "https://aura-historia.com/product-listings/worker-delivery-product-abcdef"
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
        payload: DomainJobPayload::NotificationDeliveryCreated(NotificationDeliveryCreatedJob {
            notification_delivery_id: delivery,
        }),
    })
}

fn claimed(attempt: u32, body: String) -> Job {
    Job::new(JobId::from_uuid(uuid::Uuid::now_v7()), attempt, None, body)
}
