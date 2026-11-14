use application::transaction::{Transaction, UnitOfWork};
use content_notice_core::content_notice_id::ContentNoticeId;
use content_notice_core::reason_category::ReasonCategory;
use listing_source_core::ListingSourceName;
use localization::Language;
use notification_core::external_email::{
    ContentNoticeReceipt, ContentNoticeReceiptTarget, ExternalEmailContent, ExternalEmailOrigin,
    ExternalEmailRecipient,
};
use notification_postgres::SqlxExternalEmailOutboxFactory;
use notification_service::ports::external_email_outbox::{
    ExternalEmailEnqueueOutcome, ExternalEmailErasure, ExternalEmailOutbox,
    ExternalEmailOutboxFactory, ExternalEmailRequest,
};
use platform_postgres::SqlxUnitOfWork;
use sqlx::PgPool;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use uuid::Uuid;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

fn origin() -> ExternalEmailOrigin {
    ExternalEmailOrigin::ContentNotice(ContentNoticeId::new())
}

/// The receipt of the notice `origin` names, about the whole provider `provider_name`.
fn request(origin: ExternalEmailOrigin, provider_name: &str) -> ExternalEmailRequest {
    let ExternalEmailOrigin::ContentNotice(content_notice_id) = origin;
    ExternalEmailRequest {
        recipient: ExternalEmailRecipient::try_from("notifier@example.test")
            .unwrap_or_else(|error| panic!("invalid test recipient: {error}")),
        language: Language::It,
        content: ExternalEmailContent::ContentNoticeReceipt(ContentNoticeReceipt {
            content_notice_id,
            received_at: time::OffsetDateTime::UNIX_EPOCH,
            provider_name: ListingSourceName::try_from(provider_name)
                .unwrap_or_else(|error| panic!("invalid test provider name: {error}")),
            target: ContentNoticeReceiptTarget::ListingSource,
            reason_category: ReasonCategory::Other,
        }),
        origin,
    }
}

/// The stored payload of [`request`].
fn payload(origin: ExternalEmailOrigin, provider_name: &str) -> serde_json::Value {
    serde_json::json!({
        "content_notice_id": origin.origin_id(),
        "received_at": "1970-01-01T00:00:00Z",
        "provider_name": provider_name,
        "target_type": "LISTING_SOURCE",
        "listing_title": null,
        "reason_category": "OTHER",
    })
}

async fn enqueue(pool: &PgPool, request: ExternalEmailRequest) -> ExternalEmailEnqueueOutcome {
    let mut tx = SqlxUnitOfWork::new(pool.clone())
        .begin()
        .await
        .unwrap_or_else(|error| panic!("begin transaction: {error}"));
    let outcome = SqlxExternalEmailOutboxFactory::new()
        .in_transaction(&mut tx)
        .enqueue(request)
        .await
        .unwrap_or_else(|error| panic!("enqueue external email: {error}"));
    tx.commit()
        .await
        .unwrap_or_else(|error| panic!("commit transaction: {error}"));
    outcome
}

async fn count(pool: &PgPool, sql: &'static str) -> i64 {
    sqlx::query_scalar(sql)
        .fetch_one(pool)
        .await
        .unwrap_or_else(|error| panic!("count rows: {error}"))
}

#[derive(Debug, PartialEq, sqlx::FromRow)]
struct StoredMessage {
    recipient_email: String,
    language: String,
    template: String,
    payload_version: i32,
    payload: serde_json::Value,
    origin_type: String,
    origin_id: Uuid,
    status: String,
    channel: String,
    target_key: String,
    attempt_count: i32,
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_commit_message_and_pending_delivery_in_the_callers_transaction() {
    let pool = get_postgres_client().await;
    let origin = origin();

    let outcome = enqueue(&pool, request(origin, "Antik Shop")).await;

    let ExternalEmailEnqueueOutcome::Enqueued(id) = outcome else {
        panic!("expected a new external email, got {outcome:?}");
    };
    let stored = sqlx::query_as::<_, StoredMessage>(
        "SELECT m.recipient_email, m.language, m.template, m.payload_version, m.payload, m.origin_type, m.origin_id, d.status, d.channel, d.target_key, d.attempt_count FROM external_email_messages m JOIN notification_deliveries d USING (external_email_message_id) WHERE m.external_email_message_id = $1",
    )
    .bind(id.into_uuid())
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("load stored external email: {error}"));
    assert_eq!(
        StoredMessage {
            recipient_email: "notifier@example.test".to_owned(),
            language: "it".to_owned(),
            template: "CONTENT_NOTICE_RECEIPT".to_owned(),
            payload_version: 1,
            payload: payload(origin, "Antik Shop"),
            origin_type: "CONTENT_NOTICE".to_owned(),
            origin_id: origin.origin_id(),
            status: "PENDING".to_owned(),
            channel: "EMAIL".to_owned(),
            target_key: "RECIPIENT".to_owned(),
            attempt_count: 0,
        },
        stored
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_leave_nothing_behind_when_the_callers_transaction_rolls_back() {
    let pool = get_postgres_client().await;
    let mut tx = SqlxUnitOfWork::new(pool.clone())
        .begin()
        .await
        .unwrap_or_else(|error| panic!("begin transaction: {error}"));
    SqlxExternalEmailOutboxFactory::new()
        .in_transaction(&mut tx)
        .enqueue(request(origin(), "Antik Shop"))
        .await
        .unwrap_or_else(|error| panic!("enqueue external email: {error}"));

    drop(tx);

    assert_eq!(
        0,
        count(&pool, "SELECT count(*) FROM external_email_messages").await
    );
    assert_eq!(
        0,
        count(&pool, "SELECT count(*) FROM notification_deliveries").await
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_return_the_existing_message_when_an_origin_enqueues_the_same_template_again() {
    let pool = get_postgres_client().await;
    let origin = origin();

    let first = enqueue(&pool, request(origin, "Antik Shop")).await;
    let repeated = enqueue(&pool, request(origin, "Kunsthandel")).await;
    let other_origin = enqueue(&pool, request(self::origin(), "Antik Shop")).await;

    assert!(matches!(first, ExternalEmailEnqueueOutcome::Enqueued(_)));
    assert_eq!(
        ExternalEmailEnqueueOutcome::AlreadyEnqueued(first.external_email_message_id()),
        repeated
    );
    assert!(matches!(
        other_origin,
        ExternalEmailEnqueueOutcome::Enqueued(id) if id != first.external_email_message_id()
    ));
    assert_eq!(
        2,
        count(&pool, "SELECT count(*) FROM external_email_messages").await
    );
    assert_eq!(
        2,
        count(&pool, "SELECT count(*) FROM notification_deliveries").await
    );
    assert_eq!(
        payload(origin, "Antik Shop"),
        sqlx::query_scalar::<_, serde_json::Value>(
            "SELECT payload FROM external_email_messages WHERE external_email_message_id = $1"
        )
        .bind(first.external_email_message_id().into_uuid())
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("load payload: {error}"))
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_wait_for_a_concurrent_enqueue_and_follow_its_outcome() {
    let pool = get_postgres_client().await;
    for commit_first in [true, false] {
        let origin = origin();
        let factory = SqlxExternalEmailOutboxFactory::new();
        let unit_of_work = SqlxUnitOfWork::new(pool.clone());
        let mut first_tx = unit_of_work
            .begin()
            .await
            .unwrap_or_else(|error| panic!("begin transaction: {error}"));
        let first = factory
            .in_transaction(&mut first_tx)
            .enqueue(request(origin, "Antik Shop"))
            .await
            .unwrap_or_else(|error| panic!("enqueue external email: {error}"));

        let second = tokio::spawn({
            let pool = pool.clone();
            async move { enqueue(&pool, request(origin, "Antik Shop")).await }
        });
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert!(
            !second.is_finished(),
            "the second enqueue must wait for the first transaction"
        );
        if commit_first {
            first_tx
                .commit()
                .await
                .unwrap_or_else(|error| panic!("commit transaction: {error}"));
        } else {
            drop(first_tx);
        }
        let second = second
            .await
            .unwrap_or_else(|error| panic!("second enqueue task: {error}"));

        if commit_first {
            assert_eq!(
                ExternalEmailEnqueueOutcome::AlreadyEnqueued(first.external_email_message_id()),
                second
            );
        } else {
            assert!(matches!(
                second,
                ExternalEmailEnqueueOutcome::Enqueued(id) if id != first.external_email_message_id()
            ));
        }
    }
    assert_eq!(
        2,
        count(&pool, "SELECT count(*) FROM notification_deliveries").await
    );
}

async fn erase(pool: &PgPool, origin: ExternalEmailOrigin) -> ExternalEmailErasure {
    let mut tx = SqlxUnitOfWork::new(pool.clone())
        .begin()
        .await
        .unwrap_or_else(|error| panic!("begin transaction: {error}"));
    let erasure = SqlxExternalEmailOutboxFactory::new()
        .in_transaction(&mut tx)
        .erase_origin(origin)
        .await
        .unwrap_or_else(|error| panic!("erase origin: {error}"));
    tx.commit()
        .await
        .unwrap_or_else(|error| panic!("commit transaction: {error}"));
    erasure
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_erase_only_finished_messages_of_an_origin_with_their_deliveries() {
    let pool = get_postgres_client().await;
    let finished = origin();
    let pending = origin();
    let finished_id = enqueue(&pool, request(finished, "Antik Shop"))
        .await
        .external_email_message_id();
    let pending_id = enqueue(&pool, request(pending, "Kunsthandel"))
        .await
        .external_email_message_id();
    sqlx::query("UPDATE notification_deliveries SET status = 'DELIVERED', provider_message_id = 'receipt', delivered_at = now() WHERE external_email_message_id = $1")
        .bind(finished_id.into_uuid())
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("finish delivery: {error}"));

    assert_eq!(
        ExternalEmailErasure {
            erased: 1,
            in_flight: 0
        },
        erase(&pool, finished).await
    );
    assert_eq!(
        ExternalEmailErasure {
            erased: 0,
            in_flight: 1
        },
        erase(&pool, pending).await
    );
    assert_eq!(
        vec![pending_id.into_uuid()],
        sqlx::query_scalar::<_, Uuid>(
            "SELECT external_email_message_id FROM notification_deliveries"
        )
        .fetch_all(&pool)
        .await
        .unwrap_or_else(|error| panic!("load deliveries: {error}"))
    );
    assert_eq!(
        1,
        count(&pool, "SELECT count(*) FROM external_email_messages").await
    );
}
