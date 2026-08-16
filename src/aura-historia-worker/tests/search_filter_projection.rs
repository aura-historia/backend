use std::sync::Arc;
use std::time::{Duration, Instant};

use aura_historia_worker::search_filter_projection::consume_search_filter_projection_queue;
use aura_historia_worker::{QueueConfig, WorkerRunError, WorkerRuntime, serve_with_runtime};
use common::currency::domain::Currency;
use common::event_id::EventId;
use common::fx_rate_id::FxRateId;
use common::language::domain::Language;
use common::postgres::SqlxUnitOfWork;
use common::product_id::ProductId;
use common::product_lifecycle::domain::ProductLifecycle;
use common::product_slug_id::ProductSlugId;
use common::product_state::domain::ProductState;
use common::resource_state::domain::ResourceState;
use common::seller_slug_id::SellerSlugId;
use common::shop_id::ShopId;
use common::shop_name::ShopName;
use common::shop_slug_id::ShopSlugId;
use common::shops_product_id::ShopsProductId;
use common::transaction::{Transaction, UnitOfWork};
use common::user_id::UserId;
use common::user_search_filter_id::UserSearchFilterId;
use common::user_search_filter_name::UserSearchFilterName;
use fxrate_postgres::SqlxFxRateSnapshotRepositoryFactory;
use product_core::{
    product::{ProductAddress, ProductAuction, ProductPricing},
    title::Title,
};
use product_service::ports::{ProductSearchFilterMatchShopType, ProductSearchFilterMatchSource};
use search_filter_core::{NewSearchFilter, ProductSearch, SearchFilter};
use search_filter_opensearch::OpenSearchSearchFilterIndex;
use search_filter_postgres::{SqlxSearchFilterIndexReader, SqlxSearchFilterRepositoryFactory};
use search_filter_service::ports::{
    SearchFilterIndex, SearchFilterRepository, SearchFilterRepositoryFactory,
};
use search_filter_service::use_cases::{
    ProjectSearchFilterChangeHandler, ProjectSearchFilterChangeUseCase,
};
use test_api::{
    IntegrationTestService, OpenSearch, Postgres, Sequin, aura_integration_test,
    get_opensearch_client, get_postgres_client, get_sequin_worker_webhook_bind_addr, refresh_index,
};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
const WORKER_SEQUIN: Sequin = Sequin::worker_webhook();
const POLL_INTERVAL: Duration = Duration::from_millis(250);
const POLL_ATTEMPTS: usize = 120;
const ROLLBACK_OBSERVATION_DURATION: Duration = Duration::from_secs(2);

#[aura_integration_test(services = [BUSINESS_SCHEMA, OpenSearch(), WORKER_SEQUIN])]
async fn should_project_search_filter_insert_from_sequin() {
    let result = project_search_filter_insert_from_sequin().await;

    assert!(
        result.is_ok(),
        "Sequin search-filter insert projection acceptance test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OpenSearch(), WORKER_SEQUIN])]
async fn should_replace_search_filter_projection_after_sequin_update() {
    let result = project_search_filter_update_from_sequin().await;

    assert!(
        result.is_ok(),
        "Sequin search-filter update projection acceptance test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OpenSearch(), WORKER_SEQUIN])]
async fn should_not_project_rolled_back_search_filter_insert_from_sequin() {
    let result = reject_rolled_back_search_filter_insert_from_sequin().await;

    assert!(
        result.is_ok(),
        "Sequin rolled-back search-filter projection acceptance test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OpenSearch(), WORKER_SEQUIN])]
async fn should_remove_search_filter_projection_after_sequin_delete() {
    let result = project_search_filter_delete_from_sequin().await;

    assert!(
        result.is_ok(),
        "Sequin search-filter delete projection acceptance test failed: {result:?}"
    );
}

async fn project_search_filter_insert_from_sequin() -> Result<(), Box<dyn std::error::Error>> {
    let worker = ProjectionWorker::start().await?;
    let result = async {
        let user_id = seed_user(&worker.pool).await?;
        let filter = search_filter(user_id, "Sequin insert cabinet")?;

        let version = insert_filter(&worker.pool, &filter).await?;

        wait_for_percolation(&worker.index, filter.id(), "Sequin insert cabinet", true).await?;
        assert_eq!(1, version);
        Ok(())
    }
    .await;

    worker.finish(result).await
}

async fn project_search_filter_update_from_sequin() -> Result<(), Box<dyn std::error::Error>> {
    let worker = ProjectionWorker::start().await?;
    let result = async {
        let user_id = seed_user(&worker.pool).await?;
        let mut filter = search_filter(user_id, "Sequin original cabinet")?;
        let inserted_version = insert_filter(&worker.pool, &filter).await?;
        wait_for_percolation(&worker.index, filter.id(), "Sequin original cabinet", true).await?;

        filter.replace_search(
            ProductSearch::new(Language::En, Currency::Eur)
                .with_product_query("Sequin replacement cabinet".try_into()?),
            None,
        );
        let updated_version = update_filter(&worker.pool, &filter, inserted_version).await?;

        wait_for_percolation(
            &worker.index,
            filter.id(),
            "Sequin replacement cabinet",
            true,
        )
        .await?;
        wait_for_percolation(&worker.index, filter.id(), "Sequin original cabinet", false).await?;
        assert_eq!(2, updated_version);
        Ok(())
    }
    .await;

    worker.finish(result).await
}

async fn reject_rolled_back_search_filter_insert_from_sequin()
-> Result<(), Box<dyn std::error::Error>> {
    let worker = ProjectionWorker::start().await?;
    let result = async {
        let user_id = seed_user(&worker.pool).await?;
        let filter = search_filter(user_id, "Sequin rolled-back cabinet")?;

        insert_filter_then_rollback(&worker.pool, &filter).await?;
        assert_filter_is_not_persisted(&worker.pool, filter.id()).await?;

        assert_not_percolated_for(
            &worker.index,
            filter.id(),
            "Sequin rolled-back cabinet",
            ROLLBACK_OBSERVATION_DURATION,
        )
        .await
    }
    .await;

    worker.finish(result).await
}

async fn project_search_filter_delete_from_sequin() -> Result<(), Box<dyn std::error::Error>> {
    let worker = ProjectionWorker::start().await?;
    let result = async {
        let user_id = seed_user(&worker.pool).await?;
        let filter = search_filter(user_id, "Sequin deleted cabinet")?;

        insert_filter(&worker.pool, &filter).await?;
        wait_for_percolation(&worker.index, filter.id(), "Sequin deleted cabinet", true).await?;

        delete_filter(&worker.pool, filter.id()).await?;

        wait_for_percolation(&worker.index, filter.id(), "Sequin deleted cabinet", false).await?;
        Ok(())
    }
    .await;

    worker.finish(result).await
}

struct ProjectionWorker {
    pool: sqlx::PgPool,
    index: OpenSearchSearchFilterIndex,
    projection_task: JoinHandle<()>,
    shutdown_tx: oneshot::Sender<()>,
    server: JoinHandle<Result<(), WorkerRunError>>,
}

impl ProjectionWorker {
    async fn start() -> Result<Self, Box<dyn std::error::Error>> {
        let pool = get_postgres_client().await;
        seed_current_fx_snapshot(&pool).await?;
        let index = OpenSearchSearchFilterIndex::new(get_opensearch_client().await.clone());
        let handler: Arc<dyn ProjectSearchFilterChangeUseCase> =
            Arc::new(ProjectSearchFilterChangeHandler::new(
                SqlxUnitOfWork::new(pool.clone()),
                SqlxSearchFilterIndexReader::new(pool.clone()),
                SqlxFxRateSnapshotRepositoryFactory,
                index.clone(),
            ));
        let (runtime, mut receivers) =
            WorkerRuntime::with_search_filter_projection_queue(QueueConfig::new(16))?;
        let receiver = receivers
            .take(aura_historia_worker::cdc::WorkerQueue::SearchFilterOpenSearch)
            .ok_or_else(|| std::io::Error::other("search-filter worker queue is missing"))?;
        let projection_task =
            tokio::spawn(consume_search_filter_projection_queue(receiver, handler));
        let listener = tokio::net::TcpListener::bind(get_sequin_worker_webhook_bind_addr()).await?;
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server = tokio::spawn(serve_with_runtime(listener, runtime, async move {
            let _ = shutdown_rx.await;
        }));
        Ok(Self {
            pool,
            index,
            projection_task,
            shutdown_tx,
            server,
        })
    }

    async fn finish(
        self,
        test_result: Result<(), Box<dyn std::error::Error>>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let shutdown_result = self.shutdown().await;

        test_result?;
        shutdown_result
    }

    async fn shutdown(self) -> Result<(), Box<dyn std::error::Error>> {
        self.shutdown_tx
            .send(())
            .map_err(|_| std::io::Error::other("worker server shutdown channel closed"))?;
        self.server.await??;
        self.projection_task.await?;
        Ok(())
    }
}

async fn insert_filter(
    pool: &sqlx::PgPool,
    filter: &SearchFilter,
) -> Result<i64, Box<dyn std::error::Error>> {
    let mut transaction = SqlxUnitOfWork::new(pool.clone()).begin().await?;
    let inserted = SqlxSearchFilterRepositoryFactory
        .in_transaction(&mut transaction)
        .insert(filter)
        .await?;
    transaction.commit().await?;
    Ok(inserted.version)
}

async fn insert_filter_then_rollback(
    pool: &sqlx::PgPool,
    filter: &SearchFilter,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut transaction = SqlxUnitOfWork::new(pool.clone()).begin().await?;
    SqlxSearchFilterRepositoryFactory
        .in_transaction(&mut transaction)
        .insert(filter)
        .await?;
    drop(transaction);
    Ok(())
}

async fn update_filter(
    pool: &sqlx::PgPool,
    filter: &SearchFilter,
    expected_version: i64,
) -> Result<i64, Box<dyn std::error::Error>> {
    let mut transaction = SqlxUnitOfWork::new(pool.clone()).begin().await?;
    let updated = SqlxSearchFilterRepositoryFactory
        .in_transaction(&mut transaction)
        .update(filter, expected_version)
        .await?;
    transaction.commit().await?;
    Ok(updated.version)
}

async fn delete_filter(
    pool: &sqlx::PgPool,
    filter_id: UserSearchFilterId,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut transaction = SqlxUnitOfWork::new(pool.clone()).begin().await?;
    SqlxSearchFilterRepositoryFactory
        .in_transaction(&mut transaction)
        .delete(filter_id)
        .await?;
    transaction.commit().await?;
    Ok(())
}

async fn assert_filter_is_not_persisted(
    pool: &sqlx::PgPool,
    filter_id: UserSearchFilterId,
) -> Result<(), Box<dyn std::error::Error>> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM search_filters WHERE user_search_filter_id = $1)",
    )
    .bind(uuid::Uuid::parse_str(&filter_id.to_string())?)
    .fetch_one(pool)
    .await?;

    assert!(
        !exists,
        "rolled-back search filter {filter_id} was persisted"
    );
    Ok(())
}

async fn assert_not_percolated_for(
    index: &OpenSearchSearchFilterIndex,
    filter_id: UserSearchFilterId,
    title: &str,
    observation_duration: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = Instant::now() + observation_duration;

    loop {
        refresh_index("user_search_filters").await;
        let product = product_source(title)?;
        let matches = index.percolate(&product, None).await?;
        if matches
            .iter()
            .any(|search_filter| search_filter.search_filter_id == filter_id)
        {
            return Err(std::io::Error::other(format!(
                "rolled-back search filter {filter_id} was projected for {title:?}"
            ))
            .into());
        }
        if Instant::now() >= deadline {
            return Ok(());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

async fn wait_for_percolation(
    index: &OpenSearchSearchFilterIndex,
    filter_id: UserSearchFilterId,
    title: &str,
    should_match: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    for _ in 0..POLL_ATTEMPTS {
        refresh_index("user_search_filters").await;
        let product = product_source(title)?;
        let matches = index.percolate(&product, None).await?;
        let found = matches
            .iter()
            .any(|search_filter| search_filter.search_filter_id == filter_id);
        if found == should_match {
            return Ok(());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    Err(std::io::Error::other(format!(
        "search filter {filter_id} did not reach expected percolation state {should_match} for {title:?}"
    ))
    .into())
}

fn product_source(
    title: &str,
) -> Result<ProductSearchFilterMatchSource, Box<dyn std::error::Error>> {
    let title = Title::from(title);
    let url = url::Url::parse("https://shop.example.test/products/test-product")?;
    let event_id = EventId::new();

    Ok(ProductSearchFilterMatchSource {
        event_id,
        event_kind: product_service::ports::ProductSearchFilterMatchSourceEventKind::Domain,
        current_event_id: event_id,
        product_id: ProductId::new(),
        product_slug_id: ProductSlugId::from("test-product"),
        shop_id: ShopId::new(),
        shop_slug_id: ShopSlugId::from("test-shop"),
        shop_name: ShopName::from("Test shop"),
        shop_type: ProductSearchFilterMatchShopType::Marketplace,
        seller_id: ShopId::new(),
        seller_slug_id: SellerSlugId::from(ShopSlugId::from("test-seller")),
        seller_name: ShopName::from("Test seller"),
        shops_product_id: ShopsProductId::from("test-product-1"),
        address: ProductAddress::default(),
        product_title: Some(common::localized::Localized::new(
            Language::En,
            title.clone(),
        )),
        product_description: None,
        titles: std::collections::HashMap::from([(Language::En, title)]),
        descriptions: std::collections::HashMap::new(),
        pricing: ProductPricing::default(),
        sale_valuation: None,
        state: ProductState::Listed,
        lifecycle: ProductLifecycle::Active,
        url: url.clone(),
        view_url: url,
        image: None,
        images: indexmap::IndexSet::new(),
        auction: ProductAuction::default(),
        created: time::OffsetDateTime::UNIX_EPOCH,
        updated: time::OffsetDateTime::UNIX_EPOCH,
    })
}

fn search_filter(user_id: UserId, query: &str) -> Result<SearchFilter, Box<dyn std::error::Error>> {
    Ok(SearchFilter::create(NewSearchFilter {
        user_search_filter_id: UserSearchFilterId::new(),
        user_id,
        name: UserSearchFilterName::from("Sequin acceptance filter"),
        notifications: true,
        state: ResourceState::Active,
        search: ProductSearch::new(Language::En, Currency::Eur)
            .with_product_query(query.try_into()?),
        embedding: None,
    }))
}

async fn seed_current_fx_snapshot(pool: &sqlx::PgPool) -> Result<(), sqlx::Error> {
    let fx_rate_id = FxRateId::new();
    sqlx::query(
        "INSERT INTO fx_rates (fx_rate_id, captured_at, source, source_event_id) VALUES ($1, now(), $2, $3)",
    )
    .bind(uuid::Uuid::from(fx_rate_id))
    .bind("fxratesapi")
    .bind(fx_rate_id.to_string())
    .execute(pool)
    .await?;

    for currency in [
        "EUR", "GBP", "USD", "AUD", "CAD", "NZD", "CNY", "BRL", "PLN", "TRY", "JPY", "CZK", "RUB",
        "AED", "SAR", "HKD", "SGD", "CHF",
    ] {
        sqlx::query(
            "INSERT INTO fx_rate_quotes (fx_rate_id, currency, units_per_eur) VALUES ($1, $2, $3)",
        )
        .bind(uuid::Uuid::from(fx_rate_id))
        .bind(currency)
        .bind(if currency == "EUR" {
            1_000_000_i64
        } else {
            1_250_000_i64
        })
        .execute(pool)
        .await?;
    }
    Ok(())
}

async fn seed_user(pool: &sqlx::PgPool) -> Result<UserId, sqlx::Error> {
    let user_id = UserId::new();
    sqlx::query(
        "INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'ULTIMATE', 'USER')",
    )
    .bind(uuid::Uuid::from(user_id))
    .bind(format!("sequin-worker-acceptance-{user_id}@example.com"))
    .execute(pool)
    .await?;
    Ok(user_id)
}
