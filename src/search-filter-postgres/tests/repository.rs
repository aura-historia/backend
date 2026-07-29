use common::currency::domain::Currency;
use common::event_id::EventId;
use common::language::domain::Language;
use common::postgres::SqlxUnitOfWork;
use common::product_id::ProductId;
use common::resource_state::domain::ResourceState;
use common::transaction::{Transaction, UnitOfWork};
use common::user_id::UserId;
use common::user_search_filter_id::UserSearchFilterId;
use common::user_search_filter_name::UserSearchFilterName;
use product_core::product_search::ProductSearch;
use search_filter_core::{NewSearchFilter, SearchFilter, SearchFilterProductMatch};
use search_filter_postgres::{
    SqlxSearchFilterMatchRepositoryFactory, SqlxSearchFilterReader,
    SqlxSearchFilterRepositoryFactory,
};
use search_filter_service::ports::{
    SearchFilterMatchRepository, SearchFilterMatchRepositoryFactory, SearchFilterReader,
    SearchFilterRepository, SearchFilterRepositoryFactory,
};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::OffsetDateTime;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
#[serial_test::serial]
async fn should_insert_find_update_read_and_delete_search_filter() {
    let pool = get_postgres_client().await;
    let unit = SqlxUnitOfWork::new(pool.clone());
    let repo = SqlxSearchFilterRepositoryFactory;
    let reader = SqlxSearchFilterReader::new(pool.clone());
    let user_id = seed_user(&pool, "search-filter-postgres-user@example.com").await;
    let mut filter = sample_filter(user_id, "daily finds");

    let mut tx = begin(&unit).await;
    repo.in_transaction(&mut tx)
        .insert(&filter)
        .await
        .unwrap_or_else(|error| panic!("insert failed: {error:?}"));
    let loaded = repo
        .in_transaction(&mut tx)
        .find_by_id(filter.id())
        .await
        .unwrap_or_else(|error| panic!("find failed: {error:?}"));
    assert!(matches!(loaded, Some(ref value) if value.notifications()));
    filter.change_notifications(false);
    repo.in_transaction(&mut tx)
        .update(&filter)
        .await
        .unwrap_or_else(|error| panic!("update failed: {error:?}"));
    commit(tx).await;

    let filters = reader
        .find_for_user(user_id)
        .await
        .unwrap_or_else(|error| panic!("list failed: {error:?}"));
    assert_eq!(1, filters.len());
    assert!(!filters[0].filter.notifications());
    assert!(filters[0].updated >= filters[0].created);

    let mut tx = begin(&unit).await;
    repo.in_transaction(&mut tx)
        .delete(filter.id())
        .await
        .unwrap_or_else(|error| panic!("delete failed: {error:?}"));
    commit(tx).await;
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
#[serial_test::serial]
async fn should_return_already_exists_when_search_filter_exists() {
    let pool = get_postgres_client().await;
    let unit = SqlxUnitOfWork::new(pool.clone());
    let repo = SqlxSearchFilterRepositoryFactory;
    let user_id = seed_user(&pool, "search-filter-postgres-duplicate@example.com").await;
    let filter = sample_filter(user_id, "daily duplicate");

    let mut tx = begin(&unit).await;
    repo.in_transaction(&mut tx)
        .insert(&filter)
        .await
        .unwrap_or_else(|error| panic!("insert failed: {error:?}"));
    let second = repo.in_transaction(&mut tx).insert(&filter).await;

    assert!(matches!(
        second,
        Err(search_filter_service::ports::SearchFilterRepositoryError::AlreadyExists)
    ));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
#[serial_test::serial]
async fn should_insert_find_and_update_search_filter_match() {
    let pool = get_postgres_client().await;
    let unit = SqlxUnitOfWork::new(pool.clone());
    let filters = SqlxSearchFilterRepositoryFactory;
    let matches = SqlxSearchFilterMatchRepositoryFactory;
    let user_id = seed_user(&pool, "search-filter-postgres-match@example.com").await;
    let filter = sample_filter(user_id, "match filter");
    let product_id = seed_product(&pool, "search-filter-match-product").await;
    let event_id = seed_product_event(&pool, product_id).await;
    let now = OffsetDateTime::now_utc();
    let mut product_match = SearchFilterProductMatch {
        user_id,
        user_search_filter_id: filter.id(),
        user_search_filter_name: Some(filter.name().clone()),
        product_id,
        origin_event_id: event_id,
        enhanced_match_reason: None,
        feedback: None,
        created: now,
        updated: now,
    };

    let mut tx = begin(&unit).await;
    filters
        .in_transaction(&mut tx)
        .insert(&filter)
        .await
        .unwrap_or_else(|error| panic!("insert filter failed: {error:?}"));
    matches
        .in_transaction(&mut tx)
        .insert(&product_match)
        .await
        .unwrap_or_else(|error| panic!("insert match failed: {error:?}"));
    let loaded = matches
        .in_transaction(&mut tx)
        .find_by_filter_and_product(filter.id(), product_id)
        .await
        .unwrap_or_else(|error| panic!("find match failed: {error:?}"));
    assert!(loaded.is_some());
    product_match.feedback = Some(true);
    product_match.updated = OffsetDateTime::now_utc();
    matches
        .in_transaction(&mut tx)
        .update(&product_match)
        .await
        .unwrap_or_else(|error| panic!("update match failed: {error:?}"));
    commit(tx).await;
}

fn sample_filter(user_id: UserId, name: &str) -> SearchFilter {
    SearchFilter::create(NewSearchFilter {
        user_search_filter_id: UserSearchFilterId::new(),
        user_id,
        name: UserSearchFilterName::from(name),
        notifications: true,
        state: ResourceState::Active,
        search: ProductSearch::new(Language::En, Currency::Eur),
        embedding: None,
    })
}

async fn begin(unit: &SqlxUnitOfWork) -> common::postgres::SqlxTransaction {
    unit.begin()
        .await
        .unwrap_or_else(|error| panic!("begin failed: {error:?}"))
}

async fn commit(tx: common::postgres::SqlxTransaction) {
    tx.commit()
        .await
        .unwrap_or_else(|error| panic!("commit failed: {error:?}"));
}

async fn seed_user(pool: &sqlx::PgPool, email: &str) -> UserId {
    let id = UserId::new();
    sqlx::query(
        "INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'Ultimate', 'User')",
    )
    .bind(uuid::Uuid::from(id))
    .bind(email)
    .execute(pool)
    .await
    .unwrap_or_else(|error| panic!("seed user failed: {error:?}"));
    id
}

async fn seed_product(pool: &sqlx::PgPool, slug: &str) -> ProductId {
    let product_id = ProductId::new();
    let shop_id = uuid::Uuid::new_v4();
    let event_id = uuid::Uuid::new_v4();
    let mut tx = pool
        .begin()
        .await
        .unwrap_or_else(|error| panic!("seed tx failed: {error:?}"));
    sqlx::query("INSERT INTO shops (shop_id, shop_slug_id, name, shop_type, partner_status, shop_domains) VALUES ($1, $2, $3, 'Online', 'None', '{}')")
        .bind(shop_id).bind(format!("{slug}-shop")).bind(format!("{slug} shop")).execute(&mut *tx).await.unwrap_or_else(|error| panic!("seed shop failed: {error:?}"));
    sqlx::query("INSERT INTO product_events (event_id, product_id, event_type, event_group, payload, event_time) VALUES ($1, $2, 'Created', 'DOMAIN', '{}', now())")
        .bind(event_id).bind(uuid::Uuid::from(product_id)).execute(&mut *tx).await.unwrap_or_else(|error| panic!("seed event failed: {error:?}"));
    sqlx::query("INSERT INTO products (product_id, product_slug_id, event_id, shop_id, seller_id, shops_product_id, state, lifecycle, url) VALUES ($1, $2, $3, $4, $4, $5, 'Listed', 'Active', 'https://example.com/product')")
        .bind(uuid::Uuid::from(product_id)).bind(slug).bind(event_id).bind(shop_id).bind(slug).execute(&mut *tx).await.unwrap_or_else(|error| panic!("seed product failed: {error:?}"));
    tx.commit()
        .await
        .unwrap_or_else(|error| panic!("seed commit failed: {error:?}"));
    product_id
}

async fn seed_product_event(pool: &sqlx::PgPool, product_id: ProductId) -> EventId {
    let event_id = EventId::new();
    sqlx::query("INSERT INTO product_events (event_id, product_id, event_type, event_group, payload, event_time) VALUES ($1, $2, 'Updated', 'DOMAIN', '{}', now())")
        .bind(uuid::Uuid::from(event_id))
        .bind(uuid::Uuid::from(product_id))
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("seed product event failed: {error:?}"));
    event_id
}
