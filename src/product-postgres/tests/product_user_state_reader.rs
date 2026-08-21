use common::{
    event_id::EventId, notification_id::NotificationId, product_id::ProductId, user_id::UserId,
    user_search_filter_id::UserSearchFilterId,
};
use product_postgres::SqlxProductUserStateReader;
use product_service::ports::{
    ProductUserStateLookup, ProductUserStateReadError, ProductUserStateReader,
};
use product_service::user_state::ProductUserState;

use std::collections::HashMap;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::{Duration, OffsetDateTime};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_return_complete_state_with_safe_and_unsafe_content_and_free_tier_limit() {
    let pool = get_postgres_client().await;
    let user_id = seed_user(&pool, "FREE", false).await;
    let first_filter_id = UserSearchFilterId::from(uuid::Uuid::from_u128(1));
    let later_filter_id = UserSearchFilterId::from(uuid::Uuid::from_u128(2));
    let month_start = OffsetDateTime::UNIX_EPOCH + Duration::days(31);
    let unsafe_product = seed_product(&pool).await;
    let safe_product = seed_product(&pool).await;
    set_product_images(
        &pool,
        unsafe_product,
        serde_json::json!([{
            "url": "https://example.test/unsafe.jpg",
            "prohibited_content": "NAZI_GERMANY"
        }]),
    )
    .await;

    insert_search_filter(&pool, user_id, first_filter_id, "Early filter").await;
    insert_search_filter(&pool, user_id, later_filter_id, "Later filter").await;

    for hour in 0_i64..10 {
        let product = seed_product(&pool).await;
        insert_search_filter_match(
            &pool,
            user_id,
            first_filter_id,
            product,
            "Early filter",
            None,
            None,
            month_start + Duration::hours(hour),
        )
        .await;
    }

    insert_watchlist(&pool, user_id, unsafe_product, false).await;
    insert_search_filter_match(
        &pool,
        user_id,
        first_filter_id,
        unsafe_product,
        "Early filter",
        Some("Matches the early filter."),
        Some(true),
        month_start + Duration::hours(10),
    )
    .await;
    insert_search_filter_match(
        &pool,
        user_id,
        later_filter_id,
        unsafe_product,
        "Later filter",
        Some("Must not be selected."),
        Some(false),
        month_start + Duration::hours(10),
    )
    .await;

    let states = find_for_user(
        &pool,
        ProductUserStateLookup {
            user_id,
            product_ids: vec![unsafe_product, safe_product],
        },
    )
    .await;

    let unsafe_state = state(&states, unsafe_product);
    assert!(unsafe_state.watchlist.watching);
    assert!(!unsafe_state.watchlist.notifications);
    assert!(!unsafe_state.prohibited_content.consent);
    assert!(unsafe_state.notification.unseen_notification_ids.is_empty());
    assert!(unsafe_state.search_filter.matched);
    assert!(unsafe_state.search_filter.hidden);
    assert_eq!(
        unsafe_state.search_filter.user_search_filter_id,
        Some(first_filter_id)
    );
    assert_eq!(
        unsafe_state
            .search_filter
            .user_search_filter_name
            .as_ref()
            .map(AsRef::as_ref),
        Some("Early filter")
    );
    assert_eq!(
        unsafe_state
            .search_filter
            .match_reason
            .as_ref()
            .map(AsRef::as_ref),
        Some("Matches the early filter.")
    );
    assert_eq!(unsafe_state.search_filter.match_feedback, Some(true));

    let safe_state = state(&states, safe_product);
    assert!(!safe_state.watchlist.watching);
    assert!(!safe_state.watchlist.notifications);
    assert!(safe_state.prohibited_content.consent);
    assert!(!safe_state.search_filter.matched);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_return_default_state_when_user_has_no_watchlist_or_matches() {
    let pool = get_postgres_client().await;
    let user_id = seed_user(&pool, "FREE", false).await;
    let product_id = seed_product(&pool).await;
    set_product_images(
        &pool,
        product_id,
        serde_json::json!([{
            "url": "https://example.test/unsafe.jpg",
            "prohibited_content": "UNKNOWN"
        }]),
    )
    .await;

    let states = find_for_user(
        &pool,
        ProductUserStateLookup {
            user_id,
            product_ids: vec![product_id],
        },
    )
    .await;

    assert_eq!(state(&states, product_id), &ProductUserState::default());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_reject_unknown_user_instead_of_defaulting_state() {
    let pool = get_postgres_client().await;

    let result = find_for_user_result(
        &pool,
        ProductUserStateLookup {
            user_id: UserId::new(),
            product_ids: vec![ProductId::new()],
        },
    )
    .await;

    assert!(matches!(
        result,
        Err(ProductUserStateReadError::InvalidReadModel { .. })
    ));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_return_all_unseen_notification_ids_newest_first_without_cross_product_leakage() {
    let pool = get_postgres_client().await;
    let user_id = seed_user(&pool, "PRO", false).await;
    let product_id = seed_product(&pool).await;
    let other_product_id = seed_product(&pool).await;
    let watchlist_notification_id = NotificationId::new();
    let filter_notification_id = NotificationId::new();
    let seen_notification_id = NotificationId::new();
    let other_notification_id = NotificationId::new();
    let filter_id = UserSearchFilterId::new();

    insert_watchlist_notification(
        &pool,
        user_id,
        product_id,
        watchlist_notification_id,
        OffsetDateTime::UNIX_EPOCH + Duration::seconds(1),
        false,
    )
    .await;
    insert_search_filter_notification(
        &pool,
        user_id,
        product_id,
        filter_id,
        filter_notification_id,
        OffsetDateTime::UNIX_EPOCH + Duration::seconds(2),
        false,
    )
    .await;
    insert_watchlist_notification(
        &pool,
        user_id,
        product_id,
        seen_notification_id,
        OffsetDateTime::UNIX_EPOCH + Duration::seconds(3),
        true,
    )
    .await;
    insert_watchlist_notification(
        &pool,
        user_id,
        other_product_id,
        other_notification_id,
        OffsetDateTime::UNIX_EPOCH + Duration::seconds(4),
        false,
    )
    .await;

    let states = find_for_user(
        &pool,
        ProductUserStateLookup {
            user_id,
            product_ids: vec![product_id, other_product_id],
        },
    )
    .await;

    assert_eq!(
        vec![filter_notification_id, watchlist_notification_id],
        state(&states, product_id)
            .notification
            .unseen_notification_ids
    );
    assert_eq!(
        vec![other_notification_id],
        state(&states, other_product_id)
            .notification
            .unseen_notification_ids
    );

    let update =
        sqlx::query("UPDATE notifications SET seen = true WHERE user_id = $1 AND product_id = $2")
            .bind(uuid::Uuid::from(user_id))
            .bind(uuid::Uuid::from(product_id))
            .execute(&pool)
            .await;
    if let Err(error) = update {
        panic!("failed to mark product notifications seen: {error}");
    }

    let states = find_for_user(
        &pool,
        ProductUserStateLookup {
            user_id,
            product_ids: vec![product_id],
        },
    )
    .await;
    assert!(
        state(&states, product_id)
            .notification
            .unseen_notification_ids
            .is_empty()
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_return_user_state_for_many_products_in_one_batched_lookup() {
    let pool = get_postgres_client().await;
    let user_id = seed_user(&pool, "PRO", false).await;
    let watched_product = seed_product(&pool).await;
    let safe_product = seed_product(&pool).await;
    let unsafe_product = seed_product(&pool).await;
    insert_watchlist(&pool, user_id, watched_product, true).await;

    set_product_images(
        &pool,
        unsafe_product,
        serde_json::json!([{
            "url": "https://example.test/unsafe.jpg",
            "prohibited_content": "NAZI_GERMANY"
        }]),
    )
    .await;

    let states = find_for_user(
        &pool,
        ProductUserStateLookup {
            user_id,
            product_ids: vec![watched_product, safe_product, unsafe_product],
        },
    )
    .await;

    assert_eq!(states.len(), 3);
    assert!(state(&states, watched_product).watchlist.watching);
    assert!(state(&states, watched_product).watchlist.notifications);
    assert!(state(&states, safe_product).prohibited_content.consent);
    assert!(!state(&states, unsafe_product).prohibited_content.consent);
}

async fn find_for_user(
    pool: &sqlx::PgPool,
    lookup: ProductUserStateLookup,
) -> HashMap<ProductId, ProductUserState> {
    match find_for_user_result(pool, lookup).await {
        Ok(states) => states,
        Err(error) => panic!("failed to read product user state: {error}"),
    }
}

async fn find_for_user_result(
    pool: &sqlx::PgPool,
    lookup: ProductUserStateLookup,
) -> Result<HashMap<ProductId, ProductUserState>, ProductUserStateReadError> {
    SqlxProductUserStateReader::new(pool.clone())
        .find_for_user(&lookup)
        .await
}

fn state(
    states: &HashMap<ProductId, ProductUserState>,
    product_id: ProductId,
) -> &ProductUserState {
    match states.get(&product_id) {
        Some(state) => state,
        None => panic!("missing user state for product {product_id}"),
    }
}

async fn seed_user(pool: &sqlx::PgPool, tier: &str, prohibited_content_consent: bool) -> UserId {
    let user_id = UserId::new();
    let result = sqlx::query(
        r#"
        INSERT INTO users (user_id, email, prohibited_content_consent, tier, role)
        VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(uuid::Uuid::from(user_id))
    .bind(format!("{user_id}@example.test"))
    .bind(prohibited_content_consent)
    .bind(tier)
    .bind("USER")
    .execute(pool)
    .await;

    if let Err(error) = result {
        panic!("failed to seed user: {error}");
    }

    user_id
}

async fn seed_product(pool: &sqlx::PgPool) -> ProductId {
    let product_id = ProductId::new();
    let event_id = EventId::new();
    let shop_id = uuid::Uuid::from(ProductId::new());
    let raw_product_id = uuid::Uuid::from(product_id);
    let mut transaction = match pool.begin().await {
        Ok(transaction) => transaction,
        Err(error) => panic!("failed to begin product seed transaction: {error}"),
    };
    let shop_label = format!("product-user-state-{shop_id}");

    let shop_result = sqlx::query(
        r#"
        INSERT INTO shops (shop_id, shop_slug_id, name, shop_type, partner_status, shop_domains)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(shop_id)
    .bind(&shop_label)
    .bind(&shop_label)
    .bind("COMMERCIAL_DEALER")
    .bind("SCRAPED")
    .bind(Vec::<String>::from([format!("{shop_label}.example")]))
    .execute(&mut *transaction)
    .await;
    if let Err(error) = shop_result {
        panic!("failed to seed product shop: {error}");
    }

    let product_result = sqlx::query(
        r#"
        INSERT INTO products (
            product_id, product_slug_id, event_id, shop_id, seller_id, shops_product_id,
            state, lifecycle, url
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        "#,
    )
    .bind(raw_product_id)
    .bind(format!("product-user-state-{raw_product_id}"))
    .bind(uuid::Uuid::from(event_id))
    .bind(shop_id)
    .bind(shop_id)
    .bind(raw_product_id.to_string())
    .bind("LISTED")
    .bind("ACTIVE")
    .bind("https://example.test/product")
    .execute(&mut *transaction)
    .await;
    if let Err(error) = product_result {
        panic!("failed to seed product: {error}");
    }

    let event_result = sqlx::query(
        r#"
        INSERT INTO product_events (
            event_id, product_id, event_type, event_group, payload, event_time
        ) VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(uuid::Uuid::from(event_id))
    .bind(raw_product_id)
    .bind("PRODUCT_CREATED")
    .bind("DOMAIN")
    .bind(serde_json::json!({}))
    .bind(OffsetDateTime::UNIX_EPOCH)
    .execute(&mut *transaction)
    .await;
    if let Err(error) = event_result {
        panic!("failed to seed product event: {error}");
    }

    if let Err(error) = transaction.commit().await {
        panic!("failed to commit product seed transaction: {error}");
    }

    product_id
}

async fn set_product_images(pool: &sqlx::PgPool, product_id: ProductId, images: serde_json::Value) {
    let result = sqlx::query("UPDATE products SET product_images = $1 WHERE product_id = $2")
        .bind(images)
        .bind(uuid::Uuid::from(product_id))
        .execute(pool)
        .await;

    if let Err(error) = result {
        panic!("failed to set product images: {error}");
    }
}

async fn insert_watchlist_notification(
    pool: &sqlx::PgPool,
    user_id: UserId,
    product_id: ProductId,
    notification_id: NotificationId,
    created: OffsetDateTime,
    seen: bool,
) {
    let result = sqlx::query(
        r#"
        INSERT INTO notifications (
            notification_id, user_id, kind, origin_event_id, product_id, payload, seen, created
        ) VALUES ($1, $2, 'WATCHLIST_STATE_CHANGED', $3, $4, $5, $6, $7)
        "#,
    )
    .bind(uuid::Uuid::from(notification_id))
    .bind(uuid::Uuid::from(user_id))
    .bind(uuid::Uuid::new_v4())
    .bind(uuid::Uuid::from(product_id))
    .bind(serde_json::json!({}))
    .bind(seen)
    .bind(created)
    .execute(pool)
    .await;

    if let Err(error) = result {
        panic!("failed to seed watchlist notification: {error}");
    }
}

async fn insert_search_filter_notification(
    pool: &sqlx::PgPool,
    user_id: UserId,
    product_id: ProductId,
    filter_id: UserSearchFilterId,
    notification_id: NotificationId,
    created: OffsetDateTime,
    seen: bool,
) {
    let result = sqlx::query(
        r#"
        INSERT INTO notifications (
            notification_id, user_id, kind, origin_event_id, product_id,
            user_search_filter_id, payload, seen, created
        ) VALUES ($1, $2, 'SEARCH_FILTER_MATCH', $3, $4, $5, $6, $7, $8)
        "#,
    )
    .bind(uuid::Uuid::from(notification_id))
    .bind(uuid::Uuid::from(user_id))
    .bind(uuid::Uuid::new_v4())
    .bind(uuid::Uuid::from(product_id))
    .bind(uuid_from_filter_id(filter_id))
    .bind(serde_json::json!({}))
    .bind(seen)
    .bind(created)
    .execute(pool)
    .await;

    if let Err(error) = result {
        panic!("failed to seed search-filter notification: {error}");
    }
}

async fn insert_watchlist(
    pool: &sqlx::PgPool,
    user_id: UserId,
    product_id: ProductId,
    notifications: bool,
) {
    let result = sqlx::query(
        r#"
        INSERT INTO product_watchlist (user_id, product_id, notifications, state, active_since, notifications_enabled_since)
        VALUES ($1, $2, $3, $4, CASE WHEN $4 = 'ACTIVE' THEN now() ELSE NULL END, CASE WHEN $3 THEN now() ELSE NULL END)
        "#,
    )
    .bind(uuid::Uuid::from(user_id))
    .bind(uuid::Uuid::from(product_id))
    .bind(notifications)
    .bind("ACTIVE")
    .execute(pool)
    .await;

    if let Err(error) = result {
        panic!("failed to seed product watchlist: {error}");
    }
}

async fn insert_search_filter(
    pool: &sqlx::PgPool,
    user_id: UserId,
    filter_id: UserSearchFilterId,
    name: &str,
) {
    let result = sqlx::query(
        r#"
        INSERT INTO search_filters (
            user_search_filter_id, user_id, name, notifications, state, search, language, currency
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        "#,
    )
    .bind(uuid_from_filter_id(filter_id))
    .bind(uuid::Uuid::from(user_id))
    .bind(name)
    .bind(true)
    .bind("ACTIVE")
    .bind(serde_json::json!({}))
    .bind("en")
    .bind("EUR")
    .execute(pool)
    .await;

    if let Err(error) = result {
        panic!("failed to seed search filter: {error}");
    }
}

#[allow(clippy::too_many_arguments)]
async fn insert_search_filter_match(
    pool: &sqlx::PgPool,
    user_id: UserId,
    filter_id: UserSearchFilterId,
    product_id: ProductId,
    name: &str,
    reason: Option<&str>,
    feedback: Option<bool>,
    created: OffsetDateTime,
) {
    let origin_event_id = event_id_for_product(pool, product_id).await;
    let result = sqlx::query(
        r#"
        INSERT INTO search_filter_matches (
            user_id, user_search_filter_id, product_id, origin_event_id,
            user_search_filter_name, enhanced_match_reason, feedback, created
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        "#,
    )
    .bind(uuid::Uuid::from(user_id))
    .bind(uuid_from_filter_id(filter_id))
    .bind(uuid::Uuid::from(product_id))
    .bind(origin_event_id)
    .bind(name)
    .bind(reason)
    .bind(feedback)
    .bind(created)
    .execute(pool)
    .await;

    if let Err(error) = result {
        panic!("failed to seed search filter match: {error}");
    }
}

async fn event_id_for_product(pool: &sqlx::PgPool, product_id: ProductId) -> uuid::Uuid {
    let result =
        sqlx::query_scalar::<_, uuid::Uuid>("SELECT event_id FROM products WHERE product_id = $1")
            .bind(uuid::Uuid::from(product_id))
            .fetch_one(pool)
            .await;

    match result {
        Ok(event_id) => event_id,
        Err(error) => panic!("failed to read product event ID: {error}"),
    }
}

fn uuid_from_filter_id(filter_id: UserSearchFilterId) -> uuid::Uuid {
    match uuid::Uuid::parse_str(&filter_id.to_string()) {
        Ok(value) => value,
        Err(error) => panic!("invalid user search filter ID: {error}"),
    }
}
