//! The receipt email intent commits or rolls back with its notice (`docs/content-notices.md`,
//! Receipt email).

mod support;

use application::{
    error::static_error,
    operation_context::{CorrelationId, OperationContext, Principal, RequestId},
};
use content_notice_core::{
    additional_url::{AdditionalUrl, AdditionalUrls},
    good_faith_statement_version::GoodFaithStatementVersion,
    notice_explanation::NoticeExplanation,
    reason_category::ReasonCategory,
    submitter::{Submitter, SubmitterEmail, SubmitterName},
    submitter_role::SubmitterRole,
};
use content_notice_postgres::{
    SqlxContentNoticeRepositoryFactory, SqlxContentNoticeSubmissionHistoryFactory,
    SqlxContentNoticeTargetReaderFactory,
};
use content_notice_service::{
    ports::ContentNoticeClock,
    use_cases::commands::submit_content_notice::{
        ContentNoticeTargetReference, SubmitContentNoticeCommand, SubmitContentNoticeError,
        SubmitContentNoticeHandler, SubmitContentNoticeResult, SubmitContentNoticeUseCase,
    },
};
use localization::Language;
use notification_core::external_email::ExternalEmailOrigin;
use notification_postgres::SqlxExternalEmailOutboxFactory;
use notification_service::ports::external_email_outbox::{
    ExternalEmailEnqueueOutcome, ExternalEmailErasure, ExternalEmailOutbox,
    ExternalEmailOutboxError, ExternalEmailOutboxFactory, ExternalEmailRequest,
};
use platform_postgres::{SqlxTransaction, SqlxUnitOfWork};
use product_listing_core::listing_lifecycle::ListingLifecycle;
use serde_json::{Value, json};
use sqlx::PgPool;
use support::{seed_listing_source, seed_product_listing};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::{OffsetDateTime, macros::datetime};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

const NOW: OffsetDateTime = datetime!(2026-11-14 12:00:00.25 UTC);

struct FixedClock;

impl ContentNoticeClock for FixedClock {
    fn now_utc(&self) -> OffsetDateTime {
        NOW
    }
}

fn handler<O>(pool: &PgPool, receipts: O) -> impl SubmitContentNoticeUseCase
where
    O: ExternalEmailOutboxFactory<SqlxTransaction> + 'static,
{
    SubmitContentNoticeHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxContentNoticeTargetReaderFactory::new(),
        SqlxContentNoticeRepositoryFactory::new(),
        SqlxContentNoticeSubmissionHistoryFactory::new(),
        receipts,
        FixedClock,
    )
}

fn context() -> OperationContext {
    OperationContext {
        principal: Principal::Anonymous,
        request_id: RequestId::new("request"),
        correlation_id: CorrelationId::new("correlation"),
    }
}

/// A notice whose submitter-typed fields carry marker text that must never reach a receipt.
fn command(target: ContentNoticeTargetReference) -> SubmitContentNoticeCommand {
    SubmitContentNoticeCommand {
        target,
        submitter_role: SubmitterRole::RightsHolder,
        reason_category: ReasonCategory::RightsInfringement,
        explanation: Some(NoticeExplanation::try_from("MarkerExplanation").unwrap()),
        additional_urls: AdditionalUrls::try_from(vec![
            AdditionalUrl::try_from("https://marker-url.example/photo").unwrap(),
        ])
        .unwrap(),
        submitter: Submitter {
            name: SubmitterName::try_from("MarkerName").unwrap(),
            email: SubmitterEmail::try_from("Erika@Example.com").unwrap(),
            language: Language::Fr,
        },
        good_faith_statement_version: GoodFaithStatementVersion::GoodFaithV1,
    }
}

/// Every external email row in the database, so a test sees stray rows too.
async fn external_emails(pool: &PgPool) -> Vec<Value> {
    sqlx::query_scalar(
        "SELECT jsonb_build_object(
             'recipient_email', m.recipient_email, 'language', m.language,
             'template', m.template, 'payload_version', m.payload_version, 'payload', m.payload,
             'origin_type', m.origin_type, 'origin_id', m.origin_id,
             'channel', d.channel, 'target_key', d.target_key, 'status', d.status,
             'attempt_count', d.attempt_count)
         FROM external_email_messages m
         LEFT JOIN notification_deliveries d
             ON d.external_email_message_id = m.external_email_message_id
         ORDER BY m.created",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn stored_notices(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM content_notices")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_commit_a_stored_notice_with_exactly_one_pending_receipt_of_our_own_data() {
    let pool = get_postgres_client().await;
    let source = seed_listing_source(&pool, "Antik Shop", None, None).await;
    let listing = seed_product_listing(
        &pool,
        source,
        ListingLifecycle::Active,
        Some("Jugendstil-Vase"),
    )
    .await;

    let result = handler(&pool, SqlxExternalEmailOutboxFactory::new())
        .execute(
            &context(),
            command(ContentNoticeTargetReference::ProductListing {
                product_listing_id: listing,
            }),
        )
        .await
        .unwrap();

    let SubmitContentNoticeResult::Stored { content_notice_id } = result else {
        panic!("expected a stored notice, got {result:?}");
    };
    let emails = external_emails(&pool).await;
    assert_eq!(
        vec![json!({
            "recipient_email": "Erika@Example.com",
            "language": "fr",
            "template": "CONTENT_NOTICE_RECEIPT",
            "payload_version": 1,
            "payload": {
                "content_notice_id": content_notice_id.into_uuid(),
                "received_at": "2026-11-14T12:00:00.25Z",
                "provider_name": "Antik Shop",
                "listing_title": "Jugendstil-Vase",
                "reason_category": "RIGHTS_INFRINGEMENT",
            },
            "origin_type": "CONTENT_NOTICE",
            "origin_id": content_notice_id.into_uuid(),
            "channel": "EMAIL",
            "target_key": "RECIPIENT",
            "status": "PENDING",
            "attempt_count": 0,
        })],
        emails
    );
    let stored = emails[0].to_string();
    for marker in ["MarkerName", "MarkerExplanation", "marker-url.example"] {
        assert!(!stored.contains(marker), "{marker} reached the receipt");
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_enqueue_a_listing_source_receipt_without_a_listing_title() {
    let pool = get_postgres_client().await;
    let source = seed_listing_source(&pool, "Antik Shop", None, None).await;

    handler(&pool, SqlxExternalEmailOutboxFactory::new())
        .execute(
            &context(),
            command(ContentNoticeTargetReference::ListingSource {
                listing_source_id: source,
            }),
        )
        .await
        .unwrap();

    let emails = external_emails(&pool).await;
    assert_eq!(1, emails.len());
    assert_eq!(Value::Null, emails[0]["payload"]["listing_title"]);
}

/// Writes the receipt like the real outbox, then fails, as a lost connection would after the
/// insert.
struct FailingAfterEnqueue(SqlxExternalEmailOutboxFactory);

struct FailingOutbox<O>(O);

impl ExternalEmailOutboxFactory<SqlxTransaction> for FailingAfterEnqueue {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl ExternalEmailOutbox + 'tx {
        FailingOutbox(self.0.in_transaction(tx))
    }
}

#[async_trait::async_trait]
impl<O: ExternalEmailOutbox> ExternalEmailOutbox for FailingOutbox<O> {
    async fn enqueue(
        &mut self,
        request: ExternalEmailRequest,
    ) -> Result<ExternalEmailEnqueueOutcome, ExternalEmailOutboxError> {
        self.0.enqueue(request).await?;
        Err(ExternalEmailOutboxError::OperationFailed {
            source: static_error("enqueue failed after the insert"),
        })
    }

    async fn erase_origin(
        &mut self,
        origin: ExternalEmailOrigin,
    ) -> Result<ExternalEmailErasure, ExternalEmailOutboxError> {
        self.0.erase_origin(origin).await
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_roll_back_the_notice_and_its_receipt_when_the_enqueue_fails() {
    let pool = get_postgres_client().await;
    let source = seed_listing_source(&pool, "Antik Shop", None, None).await;

    let result = handler(
        &pool,
        FailingAfterEnqueue(SqlxExternalEmailOutboxFactory::new()),
    )
    .execute(
        &context(),
        command(ContentNoticeTargetReference::ListingSource {
            listing_source_id: source,
        }),
    )
    .await;

    assert!(
        matches!(
            result,
            Err(SubmitContentNoticeError::TemporarilyUnavailable { .. })
        ),
        "{result:?}"
    );
    assert_eq!(0, stored_notices(&pool).await);
    assert!(external_emails(&pool).await.is_empty());
    let deliveries: i64 = sqlx::query_scalar("SELECT count(*) FROM notification_deliveries")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(0, deliveries);
}
