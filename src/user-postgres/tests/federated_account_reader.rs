use serde_email::Email;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use user_core::user_id::UserId;
use user_postgres::SqlxFederatedAccountReader;
use user_service::ports::FederatedAccountReader;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_read_federated_account_by_exact_email_and_optional_binding() {
    let pool = get_postgres_client().await;
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let bound_email = format!("Federated-{suffix}@Example.test");
    let case_variant_email = bound_email.to_ascii_lowercase();
    let unbound_email = format!("unbound-{suffix}@example.test");
    let absent_email = format!("absent-{suffix}@example.test");
    let bound_user_id = UserId::new();
    let case_variant_user_id = UserId::new();
    let unbound_user_id = UserId::new();

    for (user_id, email) in [
        (bound_user_id, bound_email.as_str()),
        (case_variant_user_id, case_variant_email.as_str()),
        (unbound_user_id, unbound_email.as_str()),
    ] {
        sqlx::query(
            "INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'FREE', 'USER')",
        )
        .bind(user_id.into_uuid())
        .bind(email)
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to seed user: {error}"));
    }

    let issuer = "https://cognito-idp.eu-central-1.amazonaws.com/eu-central-1_test-pool";
    let subject = format!("google|{suffix}");
    sqlx::query(
        "INSERT INTO user_cognito_identities (issuer, subject, user_id) VALUES ($1, $2, $3)",
    )
    .bind(issuer)
    .bind(&subject)
    .bind(bound_user_id.into_uuid())
    .execute(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to seed Cognito binding: {error}"));

    let reader = SqlxFederatedAccountReader::new(pool.clone());
    let bound = reader
        .find_by_email(&email(&bound_email))
        .await
        .unwrap_or_else(|error| panic!("failed to read bound account: {error}"))
        .unwrap_or_else(|| panic!("exact bound email was not found"));
    let identity = bound
        .cognito_identity
        .unwrap_or_else(|| panic!("expected Cognito binding"));
    assert_eq!(bound_email, bound.email.as_ref());
    assert_eq!(issuer, identity.issuer.as_str());
    assert_eq!(subject, identity.subject.as_str());

    let case_variant = reader
        .find_by_email(&email(&case_variant_email))
        .await
        .unwrap_or_else(|error| panic!("failed to read case-variant account: {error}"))
        .unwrap_or_else(|| panic!("distinct case-variant email was not found"));
    assert_eq!(case_variant_email, case_variant.email.as_ref());
    assert!(case_variant.cognito_identity.is_none());

    let unbound = reader
        .find_by_email(&email(&unbound_email))
        .await
        .unwrap_or_else(|error| panic!("failed to read unbound account: {error}"))
        .unwrap_or_else(|| panic!("existing unbound account was not found"));
    assert_eq!(unbound_email, unbound.email.as_ref());
    assert!(unbound.cognito_identity.is_none());

    assert!(
        reader
            .find_by_email(&email(&absent_email))
            .await
            .unwrap_or_else(|error| panic!("failed to read absent account: {error}"))
            .is_none()
    );

    let duplicate_email = sqlx::query(
        "INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'FREE', 'USER')",
    )
    .bind(UserId::new().into_uuid())
    .bind(&bound_email)
    .execute(&pool)
    .await;
    assert!(matches!(
        duplicate_email,
        Err(sqlx::Error::Database(error)) if error.is_unique_violation()
    ));
}

fn email(value: &str) -> Email {
    Email::try_from(value).unwrap_or_else(|error| panic!("invalid test email: {error}"))
}
