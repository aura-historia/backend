use common::postgres::SqlxUnitOfWork;
use common::product_id::ProductId;
use common::resource_state::domain::ResourceState;
use common::transaction::{Transaction, UnitOfWork};
use common::user_id::UserId;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use watchlist_core::WatchlistProduct;
use watchlist_postgres::{
    SqlxWatchlistQuotaReaderFactory, SqlxWatchlistReaderFactory, SqlxWatchlistRepositoryFactory,
};
use watchlist_service::ports::{
    WatchlistQuotaReader, WatchlistQuotaReaderFactory, WatchlistReader, WatchlistReaderFactory,
    WatchlistRepository, WatchlistRepositoryFactory,
};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_insert_find_update_read_and_delete_watchlist_entry() {
    let pool = get_postgres_client().await;
    let unit = SqlxUnitOfWork::new(pool.clone());
    let repo = SqlxWatchlistRepositoryFactory;
    let reader = SqlxWatchlistReaderFactory;
    let user_id = seed_user(&pool, "watchlist-postgres-user@example.com").await;
    let product_id = seed_product(&pool, "watchlist-postgres-product").await;
    let mut entry = WatchlistProduct::rehydrate(user_id, product_id, true, ResourceState::Active);

    let mut tx = begin(&unit).await;
    repo.in_transaction(&mut tx)
        .insert(&entry)
        .await
        .unwrap_or_else(|error| panic!("insert failed: {error:?}"));
    commit(tx).await;

    let mut tx = begin(&unit).await;
    let loaded = repo
        .in_transaction(&mut tx)
        .find_by_user_and_product(user_id, product_id)
        .await
        .unwrap_or_else(|error| panic!("find failed: {error:?}"));
    assert!(matches!(loaded, Some(ref value) if value.notifications()));

    entry.change_notifications(false);
    repo.in_transaction(&mut tx)
        .update(&entry)
        .await
        .unwrap_or_else(|error| panic!("update failed: {error:?}"));
    commit(tx).await;

    let mut tx = begin(&unit).await;
    let views = reader
        .in_transaction(&mut tx)
        .find_for_user(user_id)
        .await
        .unwrap_or_else(|error| panic!("read failed: {error:?}"));
    assert_eq!(1, views.len());
    assert!(!views[0].notifications);
    assert!(views[0].updated >= views[0].created);

    let user_ids = reader
        .in_transaction(&mut tx)
        .find_user_ids_for_product(product_id)
        .await
        .unwrap_or_else(|error| panic!("read users failed: {error:?}"));
    assert_eq!(vec![user_id], user_ids);
    commit(tx).await;

    let mut tx = begin(&unit).await;
    repo.in_transaction(&mut tx)
        .delete(user_id, product_id)
        .await
        .unwrap_or_else(|error| panic!("delete failed: {error:?}"));
    commit(tx).await;
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_count_only_active_watchlist_entries_in_transaction() {
    let pool = get_postgres_client().await;
    let unit = SqlxUnitOfWork::new(pool.clone());
    let repository = SqlxWatchlistRepositoryFactory;
    let quotas = SqlxWatchlistQuotaReaderFactory;
    let user_id = seed_user(&pool, "watchlist-postgres-quota@example.com").await;
    let active_product_id = seed_product(&pool, "watchlist-postgres-active-product").await;
    let inactive_product_id = seed_product(&pool, "watchlist-postgres-inactive-product").await;
    let active =
        WatchlistProduct::rehydrate(user_id, active_product_id, true, ResourceState::Active);
    let inactive = WatchlistProduct::rehydrate(
        user_id,
        inactive_product_id,
        true,
        ResourceState::InactiveByUser,
    );

    let mut tx = begin(&unit).await;
    repository
        .in_transaction(&mut tx)
        .insert(&active)
        .await
        .unwrap_or_else(|error| panic!("insert active entry failed: {error:?}"));
    repository
        .in_transaction(&mut tx)
        .insert(&inactive)
        .await
        .unwrap_or_else(|error| panic!("insert inactive entry failed: {error:?}"));
    let active_count = quotas
        .in_transaction(&mut tx)
        .count_active_for_user(user_id)
        .await
        .unwrap_or_else(|error| panic!("count active entries failed: {error:?}"));

    assert_eq!(1, active_count);
    commit(tx).await;
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_return_already_exists_when_watchlist_entry_exists() {
    let pool = get_postgres_client().await;
    let unit = SqlxUnitOfWork::new(pool.clone());
    let repo = SqlxWatchlistRepositoryFactory;
    let user_id = seed_user(&pool, "watchlist-postgres-duplicate@example.com").await;
    let product_id = seed_product(&pool, "watchlist-postgres-duplicate-product").await;
    let entry = WatchlistProduct::rehydrate(user_id, product_id, true, ResourceState::Active);

    let mut tx = begin(&unit).await;
    repo.in_transaction(&mut tx)
        .insert(&entry)
        .await
        .unwrap_or_else(|error| panic!("insert failed: {error:?}"));
    let second = repo.in_transaction(&mut tx).insert(&entry).await;

    assert!(matches!(
        second,
        Err(watchlist_service::ports::WatchlistRepositoryError::AlreadyExists)
    ));
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
        "INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'ULTIMATE', 'USER')",
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
    sqlx::query("INSERT INTO shops (shop_id, shop_slug_id, name, shop_type, partner_status, shop_domains) VALUES ($1, $2, $3, 'MARKETPLACE', 'SCRAPED', '{}')")
        .bind(shop_id).bind(format!("{slug}-shop")).bind(format!("{slug} shop")).execute(&mut *tx).await.unwrap_or_else(|error| panic!("seed shop failed: {error:?}"));
    sqlx::query("INSERT INTO product_events (event_id, product_id, event_type, event_group, payload, event_time) VALUES ($1, $2, 'Created', 'DOMAIN', '{}', now())")
        .bind(event_id).bind(uuid::Uuid::from(product_id)).execute(&mut *tx).await.unwrap_or_else(|error| panic!("seed event failed: {error:?}"));
    sqlx::query("INSERT INTO products (product_id, product_slug_id, event_id, shop_id, seller_id, shops_product_id, state, lifecycle, url) VALUES ($1, $2, $3, $4, $4, $5, 'LISTED', 'ACTIVE', 'https://example.com/product')")
        .bind(uuid::Uuid::from(product_id)).bind(slug).bind(event_id).bind(shop_id).bind(slug).execute(&mut *tx).await.unwrap_or_else(|error| panic!("seed product failed: {error:?}"));
    tx.commit()
        .await
        .unwrap_or_else(|error| panic!("seed commit failed: {error:?}"));
    product_id
}
