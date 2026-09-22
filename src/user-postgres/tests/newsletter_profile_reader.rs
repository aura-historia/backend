use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use user_core::{first_name::FirstName, last_name::LastName, user_id::UserId};
use user_postgres::SqlxNewsletterProfileReader;
use user_service::ports::{NewsletterProfile, NewsletterProfileReader};

const BUSINESS_SCHEMA: Postgres = Postgres::new_schema_once("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_read_newsletter_profile_when_user_exists() {
    let pool = get_postgres_client().await;
    let user_id = UserId::new();
    sqlx::query(
            "INSERT INTO users (user_id, email, first_name, last_name, language, currency, tier, role) VALUES ($1, $2, $3, $4, $5, $6, 'FREE', 'USER')",
        )
        .bind(user_id.into_uuid())
        .bind(format!("{user_id}@example.test"))
        .bind("Ada")
        .bind("Lovelace")
        .bind("en")
        .bind("EUR")
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to seed newsletter profile: {error}"));

    let profile = SqlxNewsletterProfileReader::new(pool)
        .find_by_user_id(user_id)
        .await
        .unwrap_or_else(|error| panic!("failed to read newsletter profile: {error}"));

    assert_eq!(
        Some(NewsletterProfile {
            first_name: Some(FirstName::from("Ada")),
            last_name: Some(LastName::from("Lovelace")),
            language: Some(localization::Language::En),
            currency: Some(money::Currency::Eur),
        }),
        profile
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_return_none_when_user_does_not_exist() {
    let profile = SqlxNewsletterProfileReader::new(get_postgres_client().await)
        .find_by_user_id(UserId::new())
        .await
        .unwrap_or_else(|error| panic!("failed to read newsletter profile: {error}"));

    assert_eq!(None, profile);
}
