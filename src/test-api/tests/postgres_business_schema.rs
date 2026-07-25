use test_api::*;
use uuid::Uuid;

const BUSINESS_SCHEMA: Postgres = Postgres::with_additional_migrations(
    "src/user/migrations",
    &[
        "src/shop/migrations",
        "src/partner-shop-application/migrations",
        "src/product/migrations",
        "src/product-watchlist/migrations",
        "src/search-filter/migrations",
    ],
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

    assert!(tables.contains(&"users".to_string()));
    assert!(tables.contains(&"shops".to_string()));
    assert!(tables.contains(&"user_partner_shops".to_string()));
    assert!(tables.contains(&"partner_shop_applications".to_string()));
    assert!(tables.contains(&"products".to_string()));
    assert!(tables.contains(&"product_events".to_string()));
    assert!(tables.contains(&"product_watchlist".to_string()));
    assert!(tables.contains(&"search_filters".to_string()));
    assert!(tables.contains(&"search_filter_matches".to_string()));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_support_core_business_relations() {
    let pool = get_postgres_client().await;
    let user_id = Uuid::parse_str("10000000-0000-0000-0000-000000000001").unwrap();
    let shop_id = Uuid::parse_str("20000000-0000-0000-0000-000000000001").unwrap();
    let product_id = Uuid::parse_str("30000000-0000-0000-0000-000000000001").unwrap();
    let event_id = Uuid::parse_str("40000000-0000-0000-0000-000000000001").unwrap();
    let search_filter_id = Uuid::parse_str("50000000-0000-0000-0000-000000000001").unwrap();
    let partner_application_id = Uuid::parse_str("60000000-0000-0000-0000-000000000001").unwrap();

    sqlx::query(
        "INSERT INTO users (user_id, email, tier, role, created_by, updated_by) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(user_id)
    .bind("user@example.com")
    .bind("FREE")
    .bind("USER")
    .bind("system")
    .bind("system")
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO shops (shop_id, shop_slug_id, name, shop_type, partner_status, shop_domains, created_by, updated_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(shop_id)
    .bind("shop-one")
    .bind("Shop One")
    .bind("AUCTION_HOUSE")
    .bind("PARTNERED")
    .bind(vec!["shop.example.com".to_string()])
    .bind("system")
    .bind("system")
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query("INSERT INTO user_partner_shops (user_id, shop_id) VALUES ($1, $2)")
        .bind(user_id)
        .bind(shop_id)
        .execute(&pool)
        .await
        .unwrap();

    sqlx::query(
        "INSERT INTO partner_shop_applications (\
             partner_shop_application_id, applicant_user_id, business_state, execution_state, \
             payload_type, existing_shop_id, created_by, updated_by\
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(partner_application_id)
    .bind(user_id)
    .bind("ACCEPTED")
    .bind("SUCCEEDED")
    .bind("EXISTING_SHOP")
    .bind(shop_id)
    .bind("system")
    .bind("system")
    .execute(&pool)
    .await
    .unwrap();

    let mut tx = pool.begin().await.unwrap();

    sqlx::query(
        "INSERT INTO products (\
             product_id, product_slug_id, shop_slug_id, seller_slug_id, event_id, shop_id, seller_id, \
             shops_product_id, shop_name, seller_name, shop_type, title_native_text, \
             title_native_language, state, lifecycle, url, view_url, product_images, created_by, updated_by\
         ) VALUES (\
             $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20\
         )",
    )
    .bind(product_id)
    .bind("product-one")
    .bind("shop-one")
    .bind("shop-one")
    .bind(event_id)
    .bind(shop_id)
    .bind(shop_id)
    .bind("external-1")
    .bind("Shop One")
    .bind("Shop One")
    .bind("AUCTION_HOUSE")
    .bind("A vase")
    .bind("en")
    .bind("ACTIVE")
    .bind("LISTED")
    .bind("https://shop.example.com/products/external-1")
    .bind("https://aura.example.com/shops/shop-one/products/product-one")
    .bind(sqlx::types::Json(serde_json::json!([
        { "position": 0, "url": "https://cdn.example.com/image.jpg" }
    ])))
    .bind("system")
    .bind("system")
    .execute(&mut *tx)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO product_events (\
             event_id, product_id, shop_id, shops_product_id, event_type, event_group, payload, event_time, created_by\
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, now(), $8)",
    )
    .bind(event_id)
    .bind(product_id)
    .bind(shop_id)
    .bind("external-1")
    .bind("PRODUCT_CREATED")
    .bind("DOMAIN")
    .bind(sqlx::types::Json(serde_json::json!({ "kind": "created" })))
    .bind("system")
    .execute(&mut *tx)
    .await
    .unwrap();

    tx.commit().await.unwrap();

    sqlx::query(
        "INSERT INTO product_watchlist (\
             user_id, product_id, shop_id, shops_product_id, state, created_by, updated_by\
         ) VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(user_id)
    .bind(product_id)
    .bind(shop_id)
    .bind("external-1")
    .bind("ACTIVE")
    .bind("system")
    .bind("system")
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO search_filters (\
             user_search_filter_id, user_id, name, state, search, language, currency, created_by, updated_by\
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(search_filter_id)
    .bind(user_id)
    .bind("Vases")
    .bind("ACTIVE")
    .bind(sqlx::types::Json(serde_json::json!({ "product_query": ["vase"] })))
    .bind("en")
    .bind("EUR")
    .bind("system")
    .bind("system")
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO search_filter_matches (\
             user_id, user_search_filter_id, product_id, shop_id, shops_product_id, origin_event_id, \
             created_by, updated_by\
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(user_id)
    .bind(search_filter_id)
    .bind(product_id)
    .bind(shop_id)
    .bind("external-1")
    .bind(event_id)
    .bind("system")
    .bind("system")
    .execute(&pool)
    .await
    .unwrap();

    let match_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM search_filter_matches")
        .fetch_one(&pool)
        .await
        .unwrap();

    assert_eq!(1, match_count);
}
