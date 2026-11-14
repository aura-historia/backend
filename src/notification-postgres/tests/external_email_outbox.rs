use application::transaction::{Transaction, UnitOfWork};
use localization::Language;
use notification_core::external_email::{
    ExternalEmailContent, ExternalEmailOrigin, ExternalEmailTestProbe,
};
use notification_postgres::SqlxExternalEmailOutboxFactory;
use notification_service::ports::external_email_outbox::{
    ExternalEmailEnqueueOutcome, ExternalEmailOutbox, ExternalEmailOutboxFactory,
    ExternalEmailRequest,
};
use platform_postgres::SqlxUnitOfWork;
use serde_email::Email;
use sqlx::PgPool;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use uuid::Uuid;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

fn request(origin: ExternalEmailOrigin, reference: &str) -> ExternalEmailRequest {
    ExternalEmailRequest {
        recipient: Email::new("notifier@example.test")
            .unwrap_or_else(|error| panic!("invalid test recipient: {error}")),
        language: Language::It,
        content: ExternalEmailContent::TestProbe(
            ExternalEmailTestProbe::new(reference)
                .unwrap_or_else(|error| panic!("invalid test probe: {error}")),
        ),
        origin,
    }
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
    let origin_id = Uuid::now_v7();

    let outcome = enqueue(
        &pool,
        request(ExternalEmailOrigin::TestProbe(origin_id), "probe-1"),
    )
    .await;

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
            template: "TEST_PROBE".to_owned(),
            payload_version: 1,
            payload: serde_json::json!({ "reference": "probe-1" }),
            origin_type: "TEST_PROBE".to_owned(),
            origin_id,
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
        .enqueue(request(
            ExternalEmailOrigin::TestProbe(Uuid::now_v7()),
            "probe-1",
        ))
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
    let origin = ExternalEmailOrigin::TestProbe(Uuid::now_v7());

    let first = enqueue(&pool, request(origin, "probe-1")).await;
    let repeated = enqueue(&pool, request(origin, "probe-2")).await;
    let other_origin = enqueue(
        &pool,
        request(ExternalEmailOrigin::TestProbe(Uuid::now_v7()), "probe-1"),
    )
    .await;

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
        serde_json::json!({ "reference": "probe-1" }),
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
async fn should_enqueue_an_email_once_when_one_origin_enqueues_concurrently() {
    let pool = get_postgres_client().await;
    let origin = ExternalEmailOrigin::TestProbe(Uuid::now_v7());

    let (left, right) = tokio::join!(
        enqueue(&pool, request(origin, "probe-1")),
        enqueue(&pool, request(origin, "probe-1")),
    );

    assert_eq!(
        left.external_email_message_id(),
        right.external_email_message_id()
    );
    assert_eq!(
        1,
        [left, right]
            .iter()
            .filter(|outcome| matches!(outcome, ExternalEmailEnqueueOutcome::Enqueued(_)))
            .count()
    );
    assert_eq!(
        1,
        count(&pool, "SELECT count(*) FROM notification_deliveries").await
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_erase_an_origins_messages_with_their_deliveries() {
    let pool = get_postgres_client().await;
    let erased = ExternalEmailOrigin::TestProbe(Uuid::now_v7());
    enqueue(&pool, request(erased, "probe-1")).await;
    let kept = enqueue(
        &pool,
        request(ExternalEmailOrigin::TestProbe(Uuid::now_v7()), "probe-2"),
    )
    .await;
    let mut tx = SqlxUnitOfWork::new(pool.clone())
        .begin()
        .await
        .unwrap_or_else(|error| panic!("begin transaction: {error}"));

    let erased_count = SqlxExternalEmailOutboxFactory::new()
        .in_transaction(&mut tx)
        .erase_origin(erased)
        .await
        .unwrap_or_else(|error| panic!("erase origin: {error}"));
    tx.commit()
        .await
        .unwrap_or_else(|error| panic!("commit transaction: {error}"));

    assert_eq!(1, erased_count);
    assert_eq!(
        vec![kept.external_email_message_id().into_uuid()],
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
