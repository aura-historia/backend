use sqlx::Executor;
use test_api::*;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

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
        "fx_rate_quotes",
        "fx_rates",
        "products",
        "product_events",
        "product_translations",
        "product_watchlist",
        "search_filters",
        "search_filter_matches",
    ] {
        assert!(tables.contains(&expected.to_string()));
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_support_core_business_relations() {
    let pool = get_postgres_client().await;
    pool.execute(sqlx::raw_sql(
        r#"
        INSERT INTO users (user_id, email, tier, role)
        VALUES ('10000000-0000-0000-0000-000000000001', 'user@example.com', 'FREE', 'USER');

        INSERT INTO shops (shop_id, shop_slug_id, name, shop_type, partner_status, shop_domains)
        VALUES (
            '20000000-0000-0000-0000-000000000001',
            'shop-one',
            'Shop One',
            'AUCTION_HOUSE',
            'PARTNERED',
            ARRAY['shop.example.com']
        );

        INSERT INTO user_partner_shops (user_id, shop_id)
        VALUES (
            '10000000-0000-0000-0000-000000000001',
            '20000000-0000-0000-0000-000000000001'
        );

        INSERT INTO partner_shop_applications (
            partner_shop_application_id,
            applicant_user_id,
            business_state,
            payload_type,
            shop_id
        )
        VALUES (
            '60000000-0000-0000-0000-000000000001',
            '10000000-0000-0000-0000-000000000001',
            'APPROVED',
            'EXISTING',
            '20000000-0000-0000-0000-000000000001'
        );

        BEGIN;

        INSERT INTO products (
            product_id,
            product_slug_id,
            event_id,
            shop_id,
            seller_id,
            shops_product_id,
            title_text,
            title_language,
            state,
            lifecycle,
            url,
            product_images
        )
        VALUES (
            '30000000-0000-0000-0000-000000000001',
            'product-one',
            '40000000-0000-0000-0000-000000000001',
            '20000000-0000-0000-0000-000000000001',
            '20000000-0000-0000-0000-000000000001',
            'external-1',
            'A vase',
            'en',
            'LISTED',
            'ACTIVE',
            'https://shop.example.com/products/external-1',
            '[{"url": "https://cdn.example.com/image.jpg", "prohibited_content": "NONE"}]'
        );

        INSERT INTO product_events (
            event_id,
            product_id,
            event_type,
            event_group,
            payload,
            event_time
        )
        VALUES (
            '40000000-0000-0000-0000-000000000001',
            '30000000-0000-0000-0000-000000000001',
            'PRODUCT_CREATED',
            'DOMAIN',
            '{"kind": "created"}',
            now()
        );

        COMMIT;

        INSERT INTO product_watchlist (user_id, product_id, state)
        VALUES (
            '10000000-0000-0000-0000-000000000001',
            '30000000-0000-0000-0000-000000000001',
            'ACTIVE'
        );

        INSERT INTO search_filters (
            user_search_filter_id,
            user_id,
            name,
            state,
            search,
            language,
            currency
        )
        VALUES (
            '50000000-0000-0000-0000-000000000001',
            '10000000-0000-0000-0000-000000000001',
            'Vases',
            'ACTIVE',
            '{"product_query": ["vase"]}',
            'en',
            'EUR'
        );

        INSERT INTO search_filter_matches (
            user_id,
            user_search_filter_id,
            product_id,
            origin_event_id
        )
        VALUES (
            '10000000-0000-0000-0000-000000000001',
            '50000000-0000-0000-0000-000000000001',
            '30000000-0000-0000-0000-000000000001',
            '40000000-0000-0000-0000-000000000001'
        );
        "#,
    ))
    .await
    .unwrap();

    let match_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM search_filter_matches")
        .fetch_one(&pool)
        .await
        .unwrap();

    assert_eq!(1, match_count);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_reject_noncanonical_persisted_enum_values() {
    let pool = get_postgres_client().await;

    let result =
        sqlx::query("INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, $3, $4)")
            .bind(uuid::Uuid::new_v4())
            .bind("invalid-state@example.com")
            .bind("FREE")
            .bind("User")
            .execute(&pool)
            .await;

    assert!(result.is_err(), "noncanonical user role must be rejected");
}
