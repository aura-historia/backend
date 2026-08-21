use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::event_id::EventId;
use fxrate_core::FxRateId;
use localization::Language;
use money::Currency;
use platform_postgres::SqlxUnitOfWork;
use product_core::product_id::ProductId;
use product_core::{product::ProductPriceValuationBasis, product_search::ProductSearch};
use search_filter_core::ResourceState;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use search_filter_core::user_search_filter_name::UserSearchFilterName;
use search_filter_core::{
    NewSearchFilter, PriceMatchValuation, SearchFilter, SearchFilterProductMatch,
};
use search_filter_postgres::{
    SqlxSearchFilterIndexReader, SqlxSearchFilterMatchRepositoryFactory,
    SqlxSearchFilterQuotaReaderFactory, SqlxSearchFilterReader, SqlxSearchFilterRepositoryFactory,
};
use search_filter_service::ports::{
    SearchFilterIndexReader, SearchFilterMatchRepository, SearchFilterMatchRepositoryFactory,
    SearchFilterQuotaReader, SearchFilterQuotaReaderFactory, SearchFilterReader,
    SearchFilterRepository, SearchFilterRepositoryFactory,
};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use user_core::user_id::UserId;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
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
    assert!(matches!(loaded, Some(ref value) if value.filter.notifications()));
    let expected_version = match loaded {
        Some(value) => value.version,
        None => panic!("inserted filter was not found"),
    };
    filter.change_notifications(false);
    repo.in_transaction(&mut tx)
        .update(&filter, expected_version)
        .await
        .unwrap_or_else(|error| panic!("update failed: {error:?}"));
    commit(tx).await;

    let filters = reader
        .find_for_user(user_id)
        .await
        .unwrap_or_else(|error| panic!("list failed: {error:?}"));
    assert_eq!(1, filters.len());
    assert!(!filters[0].notifications);
    assert!(filters[0].updated >= filters[0].created);

    let mut tx = begin(&unit).await;
    repo.in_transaction(&mut tx)
        .delete(filter.id())
        .await
        .unwrap_or_else(|error| panic!("delete failed: {error:?}"));
    commit(tx).await;
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_read_complete_versioned_projection_pages_from_postgres() {
    let pool = get_postgres_client().await;
    let unit = SqlxUnitOfWork::new(pool.clone());
    let repository = SqlxSearchFilterRepositoryFactory;
    let reader = SqlxSearchFilterIndexReader::new(pool.clone());
    let user_id = seed_user(&pool, "search-filter-projection-reader@example.com").await;
    let first = sample_filter(user_id, "first projection");
    let second = sample_filter(user_id, "second projection");

    let mut tx = begin(&unit).await;
    repository
        .in_transaction(&mut tx)
        .insert(&first)
        .await
        .unwrap_or_else(|error| panic!("first insert failed: {error:?}"));
    repository
        .in_transaction(&mut tx)
        .insert(&second)
        .await
        .unwrap_or_else(|error| panic!("second insert failed: {error:?}"));
    commit(tx).await;

    let projection = reader
        .find_by_id(first.id())
        .await
        .unwrap_or_else(|error| panic!("projection read failed: {error:?}"));
    assert!(matches!(
        projection,
        Some(ref projection)
            if projection.view.search_filter_id == first.id() && projection.source_version == 1
    ));

    let first_page = reader
        .list_after(None, 1)
        .await
        .unwrap_or_else(|error| panic!("first projection page failed: {error:?}"));
    assert_eq!(1, first_page.len());
    let second_page = reader
        .list_after(Some(first_page[0].view.search_filter_id), 10)
        .await
        .unwrap_or_else(|error| panic!("second projection page failed: {error:?}"));
    assert_eq!(1, second_page.len());
    assert_ne!(
        first_page[0].view.search_filter_id,
        second_page[0].view.search_filter_id
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
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
async fn should_count_only_active_search_filters_in_transaction() {
    let pool = get_postgres_client().await;
    let unit = SqlxUnitOfWork::new(pool.clone());
    let filters = SqlxSearchFilterRepositoryFactory;
    let quotas = SqlxSearchFilterQuotaReaderFactory;
    let user_id = seed_user(&pool, "search-filter-postgres-quota@example.com").await;
    let active = sample_filter(user_id, "active filter");
    let mut inactive = sample_filter(user_id, "inactive filter");
    let _ = inactive.change_state(ResourceState::InactiveByUser);

    let mut tx = begin(&unit).await;
    filters
        .in_transaction(&mut tx)
        .insert(&active)
        .await
        .unwrap_or_else(|error| panic!("insert active filter failed: {error:?}"));
    filters
        .in_transaction(&mut tx)
        .insert(&inactive)
        .await
        .unwrap_or_else(|error| panic!("insert inactive filter failed: {error:?}"));

    let active_count = quotas
        .in_transaction(&mut tx)
        .count_active_for_user(user_id)
        .await
        .unwrap_or_else(|error| panic!("count active filters failed: {error:?}"));
    assert_eq!(1, active_count);
    commit(tx).await;
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_insert_find_and_update_search_filter_match() {
    let pool = get_postgres_client().await;
    let unit = SqlxUnitOfWork::new(pool.clone());
    let filters = SqlxSearchFilterRepositoryFactory;
    let matches = SqlxSearchFilterMatchRepositoryFactory;
    let user_id = seed_user(&pool, "search-filter-postgres-match@example.com").await;
    let filter = sample_filter(user_id, "match filter");
    let product_id = seed_product(&pool, "search-filter-match-product").await;
    let event_id = seed_product_event(&pool, product_id).await;
    let fx_rate_id = seed_fx_rate(&pool).await;
    let mut product_match = SearchFilterProductMatch {
        user_id,
        user_search_filter_id: filter.id(),
        user_search_filter_name: Some(filter.name().clone()),
        product_id,
        origin_event_id: event_id,
        price_match_valuation: Some(PriceMatchValuation {
            basis: ProductPriceValuationBasis::Event,
            fx_rate_id,
        }),
        enhanced_match_reason: None,
        feedback: None,
    };

    let mut tx = begin(&unit).await;
    filters
        .in_transaction(&mut tx)
        .insert(&filter)
        .await
        .unwrap_or_else(|error| panic!("insert filter failed: {error:?}"));
    let inserted = matches
        .in_transaction(&mut tx)
        .insert(&product_match)
        .await
        .unwrap_or_else(|error| panic!("insert match failed: {error:?}"));
    assert!(inserted.updated >= inserted.created);
    let loaded = matches
        .in_transaction(&mut tx)
        .find_by_filter_and_product(filter.id(), product_id)
        .await
        .unwrap_or_else(|error| panic!("find match failed: {error:?}"));
    assert!(matches!(loaded, Some(ref value) if value.product_match == product_match));
    product_match.change_feedback(Some(true));
    let updated = matches
        .in_transaction(&mut tx)
        .update(&product_match)
        .await
        .unwrap_or_else(|error| panic!("update match failed: {error:?}"));
    assert_eq!(inserted.created, updated.created);
    assert!(updated.updated >= inserted.updated);
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

async fn begin(unit: &SqlxUnitOfWork) -> platform_postgres::SqlxTransaction {
    unit.begin()
        .await
        .unwrap_or_else(|error| panic!("begin failed: {error:?}"))
}

async fn commit(tx: platform_postgres::SqlxTransaction) {
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

async fn seed_fx_rate(pool: &sqlx::PgPool) -> FxRateId {
    let fx_rate_id = FxRateId::new();
    sqlx::query(
        "INSERT INTO fx_rates (fx_rate_id, captured_at, source, source_event_id) VALUES ($1, now(), 'fxratesapi', $2)",
    )
    .bind(uuid::Uuid::from(fx_rate_id))
    .bind(uuid::Uuid::new_v4().to_string())
    .execute(pool)
    .await
    .unwrap_or_else(|error| panic!("seed FX rate failed: {error:?}"));
    fx_rate_id
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
