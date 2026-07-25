use test_api::*;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
const BUSINESS_SCHEMA_WITH_RELATIONS: Postgres = Postgres::with_setup_script(
    "migrations",
    "src/test-api/tests/fixtures/business_schema_relations.sql",
);

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_apply_business_schema_migrations() {
    let pool = get_postgres_client().await;

    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT table_name \
         FROM information_schema.tables \
         WHERE table_schema = 'public' \
           AND table_type = 'BASE TABLE' \
         ORDER BY table_name",
    )
    .fetch_all(&pool)
    .await
    .unwrap();

    for expected in [
        "users",
        "shops",
        "user_partner_shops",
        "partner_shop_applications",
        "products",
        "product_events",
        "product_watchlist",
        "search_filters",
        "search_filter_matches",
    ] {
        assert!(tables.contains(&expected.to_string()));
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA_WITH_RELATIONS])]
async fn should_support_core_business_relations() {
    let pool = get_postgres_client().await;

    let match_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM search_filter_matches")
        .fetch_one(&pool)
        .await
        .unwrap();

    assert_eq!(1, match_count);
}
