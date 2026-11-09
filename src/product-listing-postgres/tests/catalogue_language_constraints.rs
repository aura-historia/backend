use localization::Language;
use strum::IntoEnumIterator;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

/// A language added to `Language` without a forward migration is rejected by the database
/// only when a listing in that language is written, so every persisted language column must
/// admit the whole catalogue.
#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn every_language_check_constraint_admits_every_catalogue_language() {
    let pool = get_postgres_client().await;

    let constraints: Vec<(String, String)> = sqlx::query_as(
        "SELECT conname::text, pg_get_constraintdef(oid) \
         FROM pg_constraint \
         WHERE contype = 'c' AND conname LIKE '%\\_language\\_check' \
         ORDER BY conname",
    )
    .fetch_all(&pool)
    .await
    .expect("read language constraints");

    assert!(
        !constraints.is_empty(),
        "language CHECK constraints follow the *_language_check naming convention"
    );
    for (name, definition) in constraints {
        for language in Language::iter() {
            assert!(
                definition.contains(&format!("'{}'", language.as_str())),
                "{name} rejects the catalogue language {}: {definition}",
                language.as_str()
            );
        }
    }
}
