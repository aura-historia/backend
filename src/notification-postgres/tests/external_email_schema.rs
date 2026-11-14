use sqlx::PgPool;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use uuid::Uuid;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

async fn insert_message(
    pool: &PgPool,
    template: &str,
    origin_id: Uuid,
) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO external_email_messages (external_email_message_id, recipient_email, language, template, payload_version, payload, origin_type, origin_id) VALUES ($1, 'notifier@example.test', 'en', $2, 1, '{\"reference\": \"probe-1\"}', 'TEST_PROBE', $3)")
        .bind(id)
        .bind(template)
        .bind(origin_id)
        .execute(pool)
        .await?;
    Ok(id)
}

async fn insert_notification(pool: &PgPool) -> Uuid {
    let user_id = Uuid::now_v7();
    let notification_id = Uuid::now_v7();
    sqlx::query("INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'FREE', 'USER')")
        .bind(user_id)
        .bind(format!("{user_id}@example.test"))
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("seed user: {error}"));
    sqlx::query("INSERT INTO notifications (notification_id, user_id, kind, partnership_application_id, payload_version, payload) VALUES ($1, $2, 'PARTNERSHIP_APPLICATION_APPROVED', $3, 1, $4)")
        .bind(notification_id)
        .bind(user_id)
        .bind(Uuid::now_v7())
        .bind(serde_json::json!({"type": "PARTNERSHIP_APPLICATION", "snapshot": {"party_name": "Test Party", "listing_source_name": "Test Source", "image": null}}))
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("seed notification: {error}"));
    notification_id
}

async fn insert_delivery(
    pool: &PgPool,
    notification_id: Option<Uuid>,
    external_email_message_id: Option<Uuid>,
    target_key: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO notification_deliveries (notification_delivery_id, notification_id, external_email_message_id, channel, target_key) VALUES ($1, $2, $3, 'EMAIL', $4)")
        .bind(Uuid::now_v7())
        .bind(notification_id)
        .bind(external_email_message_id)
        .bind(target_key)
        .execute(pool)
        .await
        .map(|_| ())
}

fn violated_constraint(result: Result<impl std::fmt::Debug, sqlx::Error>) -> String {
    match result {
        Err(sqlx::Error::Database(error)) => error
            .constraint()
            .unwrap_or_else(|| panic!("database error without constraint: {error}"))
            .to_owned(),
        other => panic!("expected a constraint violation, got {other:?}"),
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_accept_exactly_one_source_per_delivery() {
    let pool = get_postgres_client().await;
    let notification_id = insert_notification(&pool).await;
    let message_id = insert_message(&pool, "TEST_PROBE", Uuid::now_v7())
        .await
        .unwrap_or_else(|error| panic!("seed message: {error}"));

    insert_delivery(&pool, Some(notification_id), None, "PRIMARY")
        .await
        .unwrap_or_else(|error| panic!("notification delivery remains valid: {error}"));
    insert_delivery(&pool, None, Some(message_id), "RECIPIENT")
        .await
        .unwrap_or_else(|error| panic!("external delivery is valid: {error}"));

    let other_message_id = insert_message(&pool, "TEST_PROBE", Uuid::now_v7())
        .await
        .unwrap_or_else(|error| panic!("seed message: {error}"));
    assert_eq!(
        "notification_delivery_source_check",
        violated_constraint(
            insert_delivery(
                &pool,
                Some(notification_id),
                Some(other_message_id),
                "RECIPIENT"
            )
            .await
        )
    );
    assert_eq!(
        "notification_delivery_source_check",
        violated_constraint(insert_delivery(&pool, None, None, "RECIPIENT").await)
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_require_the_recipient_target_for_external_deliveries() {
    let pool = get_postgres_client().await;
    let message_id = insert_message(&pool, "TEST_PROBE", Uuid::now_v7())
        .await
        .unwrap_or_else(|error| panic!("seed message: {error}"));

    assert_eq!(
        "notification_deliveries_external_email_target_check",
        violated_constraint(insert_delivery(&pool, None, Some(message_id), "PRIMARY").await)
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_reject_unsupported_templates_and_a_second_email_per_origin_and_template() {
    let pool = get_postgres_client().await;
    let origin_id = Uuid::now_v7();
    insert_message(&pool, "TEST_PROBE", origin_id)
        .await
        .unwrap_or_else(|error| panic!("seed message: {error}"));

    assert_eq!(
        "external_email_messages_template_check",
        violated_constraint(insert_message(&pool, "CONTENT_NOTICE_RECEIPT", Uuid::now_v7()).await)
    );
    assert_eq!(
        "external_email_messages_origin_template_unique",
        violated_constraint(insert_message(&pool, "TEST_PROBE", origin_id).await)
    );
}
