use aura_historia_worker::cdc::WorkerQueue;
use aura_historia_worker::search_filter_match_notifications::consume_search_filter_match_notification_queue;
use aura_historia_worker::search_filter_percolator::consume_search_filter_percolator_queue;
use aura_historia_worker::{QueueConfig, WorkerRunError, WorkerRuntime, serve_with_runtime};
use common::currency::domain::Currency;
use common::event_id::EventId;
use common::language::domain::Language;
use common::postgres::SqlxUnitOfWork;
use common::resource_state::domain::ResourceState;
use common::transaction::{Transaction, UnitOfWork};
use common::user_id::UserId;
use common::user_search_filter_id::UserSearchFilterId;
use common::user_search_filter_name::UserSearchFilterName;
use notification_core::notification::NotificationPayload;
use notification_dynamodb::{
    all_notifications_reader::DynamoDbAllNotificationsReader,
    conditional_writer::ConditionalDynamoDbNotificationWriter,
};
use notification_service::ports::all_notifications_reader::{
    AllNotificationsReadItem, AllNotificationsReader,
};
use notification_service::use_cases::commands::create_notification::CreateNotificationHandler;
use product_postgres::SqlxProductSearchFilterMatchSourceReaderFactory;
use search_filter_core::{NewSearchFilter, ProductSearch, SearchFilter};
use search_filter_opensearch::OpenSearchSearchFilterIndex;
use search_filter_postgres::{
    SqlxActiveSearchFilterMatchCandidateReaderFactory, SqlxSearchFilterIndexReader,
    SqlxSearchFilterMatchNotificationSourceReaderFactory, SqlxSearchFilterMatchWriterFactory,
    SqlxSearchFilterMonthlyMatchQuotaReaderFactory, SqlxSearchFilterRepositoryFactory,
};
use search_filter_service::ports::{
    ProductMatchEvaluation, ProductMatchEvaluator, ProductMatchEvaluatorError,
    SearchFilterRepository, SearchFilterRepositoryFactory, SearchFilterView,
};
use search_filter_service::use_cases::{
    GenerateSearchFilterMatchNotificationHandler, GenerateSearchFilterMatchNotificationUseCase,
    MatchProductEventHandler, MatchProductEventUseCase, ProjectSearchFilterChangeCommand,
    ProjectSearchFilterChangeHandler, ProjectSearchFilterChangeUseCase,
    SearchFilterProjectionOperation,
};
use serde_json::json;
use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};
use test_api::{
    DynamoDB, IntegrationTestService, OpenSearch, Postgres, RunningSequin, aura_integration_test,
    get_dynamodb_client, get_opensearch_client, get_postgres_client, refresh_index,
    start_sequin_for_tables,
};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use user_postgres::SqlxUserTierEntitlementsFactory;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
const POLL_INTERVAL: Duration = Duration::from_millis(200);
const POLL_ATTEMPTS: usize = 80;
const NO_SIDE_EFFECT_OBSERVATION: Duration = Duration::from_secs(2);

struct NonMatchingProductMatchEvaluator;

#[async_trait::async_trait]
impl ProductMatchEvaluator for NonMatchingProductMatchEvaluator {
    async fn evaluate(
        &self,
        _product: &product_service::ports::ProductSearchFilterMatchSource,
        _filter: &SearchFilterView,
    ) -> Result<ProductMatchEvaluation, ProductMatchEvaluatorError> {
        Ok(ProductMatchEvaluation::NotMatched)
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OpenSearch(), DynamoDB()])]
async fn should_create_notifications_for_committed_product_create_and_update_events() {
    let result = committed_product_create_and_update_flow().await;

    assert!(
        result.is_ok(),
        "search-filter full create/update flow acceptance test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OpenSearch(), DynamoDB()])]
async fn should_match_only_active_filter_when_other_filters_are_inactive_or_do_not_match() {
    let result = active_inactive_and_no_match_flow().await;

    assert!(
        result.is_ok(),
        "search-filter filtering flow acceptance test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OpenSearch(), DynamoDB()])]
async fn should_suppress_notification_after_free_tier_monthly_quota() {
    let result = quota_flow().await;

    assert!(
        result.is_ok(),
        "search-filter quota flow acceptance test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OpenSearch(), DynamoDB()])]
async fn should_ignore_policy_and_lifecycle_product_events_without_side_effects() {
    let result = ignored_product_events_flow().await;

    assert!(
        result.is_ok(),
        "search-filter ignored-event flow acceptance test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OpenSearch(), DynamoDB()])]
async fn should_not_process_rolled_back_product_event() {
    let result = rolled_back_product_event_flow().await;

    assert!(
        result.is_ok(),
        "search-filter rollback flow acceptance test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OpenSearch(), DynamoDB()])]
async fn should_keep_one_deterministic_notification_on_product_and_match_redelivery() {
    let result = redelivery_and_deterministic_selection_flow().await;

    assert!(
        result.is_ok(),
        "search-filter redelivery and deterministic notification flow acceptance test failed: {result:?}"
    );
}

async fn committed_product_create_and_update_flow() -> Result<(), Box<dyn std::error::Error>> {
    let mut worker = FullFlowWorker::start().await?;
    let result = async {
        let user_id = seed_user(&worker.pool, "Ultimate").await?;
        let product_query = format!("Worker percolator product {user_id}");
        let filter = search_filter(
            user_id,
            UserSearchFilterName::from("Create and update notification filter"),
            ResourceState::Active,
            &product_query,
        )?;
        worker.project_filter(&filter).await?;
        refresh_index("user_search_filters").await;

        let (updated_product_id, _) = seed_product(&worker.pool, &product_query).await?;
        worker.start_scoped_sequin_subscription().await;

        let (created_product_id, created_event_id) =
            create_product_with_domain_event(&worker.pool, &product_query).await?;
        assert_eq!(
            "PRODUCT_CREATED",
            product_event_type(&worker.pool, created_event_id).await?
        );
        wait_for_match(&worker.pool, created_event_id, 1).await?;
        assert_match_for_event(
            &worker.pool,
            created_event_id,
            user_id,
            filter.id(),
            created_product_id,
        )
        .await?;
        let created_notifications =
            wait_for_notifications(&worker.notifications, user_id, 1).await?;
        assert_search_filter_notification(
            &created_notifications[0],
            user_id,
            filter.id(),
            created_product_id,
            created_event_id,
        )?;

        let updated_event_id =
            update_product_and_insert_event(&worker.pool, updated_product_id, &product_query)
                .await?;
        assert_eq!(
            "PRODUCT_STATE_CHANGED",
            product_event_type(&worker.pool, updated_event_id).await?
        );
        wait_for_match(&worker.pool, updated_event_id, 1).await?;
        assert_match_for_event(
            &worker.pool,
            updated_event_id,
            user_id,
            filter.id(),
            updated_product_id,
        )
        .await?;
        let notifications = wait_for_notifications(&worker.notifications, user_id, 2).await?;
        let created_notification = notifications
            .iter()
            .find(|notification| notification.origin_event_id == created_event_id)
            .ok_or_else(|| std::io::Error::other("created product notification is missing"))?;
        assert_search_filter_notification(
            created_notification,
            user_id,
            filter.id(),
            created_product_id,
            created_event_id,
        )?;
        let updated_notification = notifications
            .iter()
            .find(|notification| notification.origin_event_id == updated_event_id)
            .ok_or_else(|| std::io::Error::other("updated product notification is missing"))?;
        assert_search_filter_notification(
            updated_notification,
            user_id,
            filter.id(),
            updated_product_id,
            updated_event_id,
        )?;
        assert_ne!(created_product_id, updated_product_id);
        Ok(())
    }
    .await;

    worker.finish(result).await
}

async fn active_inactive_and_no_match_flow() -> Result<(), Box<dyn std::error::Error>> {
    let mut worker = FullFlowWorker::start().await?;
    let result = async {
        let user_id = seed_user(&worker.pool, "Ultimate").await?;
        let product_query = format!("Worker percolator product {user_id}");
        let active_filter = search_filter(
            user_id,
            UserSearchFilterName::from("Active matching filter"),
            ResourceState::Active,
            &product_query,
        )?;
        let inactive_filter = search_filter(
            user_id,
            UserSearchFilterName::from("Inactive matching filter"),
            ResourceState::InactiveByUser,
            &product_query,
        )?;
        let no_match_filter = search_filter(
            user_id,
            UserSearchFilterName::from("Active non-matching filter"),
            ResourceState::Active,
            "Unrelated carved marble lion",
        )?;
        worker.project_filter(&active_filter).await?;
        worker.project_filter(&inactive_filter).await?;
        worker.project_filter(&no_match_filter).await?;
        refresh_index("user_search_filters").await;

        worker.start_scoped_sequin_subscription().await;
        let (product_id, event_id) =
            create_product_with_domain_event(&worker.pool, &product_query).await?;

        wait_for_match(&worker.pool, event_id, 1).await?;
        let matches = matches_for_event(&worker.pool, event_id).await?;
        assert_eq!(vec![active_filter.id()], matches);
        let notifications = wait_for_notifications(&worker.notifications, user_id, 1).await?;
        assert_search_filter_notification(
            &notifications[0],
            user_id,
            active_filter.id(),
            product_id,
            event_id,
        )?;
        Ok(())
    }
    .await;

    worker.finish(result).await
}

async fn quota_flow() -> Result<(), Box<dyn std::error::Error>> {
    let mut worker = FullFlowWorker::start().await?;
    let result = async {
        let user_id = seed_user(&worker.pool, "Free").await?;
        let product_query = format!("Worker percolator product {user_id}");
        let filter = search_filter(
            user_id,
            UserSearchFilterName::from("Free tier quota filter"),
            ResourceState::Active,
            &product_query,
        )?;
        worker.project_filter(&filter).await?;
        refresh_index("user_search_filters").await;

        for _ in 0..10 {
            let (product_id, event_id) =
                seed_product(&worker.pool, "Historical quota product").await?;
            insert_search_filter_match(&worker.pool, user_id, filter.id(), product_id, event_id)
                .await?;
            age_match_before_current_event(&worker.pool, filter.id(), product_id).await?;
        }

        worker.start_scoped_sequin_subscription().await;
        let (_, event_id) = create_product_with_domain_event(&worker.pool, &product_query).await?;

        wait_for_match(&worker.pool, event_id, 1).await?;
        assert_no_more_than_notifications(
            &worker.notifications,
            user_id,
            0,
            NO_SIDE_EFFECT_OBSERVATION,
        )
        .await
    }
    .await;

    worker.finish(result).await
}

async fn ignored_product_events_flow() -> Result<(), Box<dyn std::error::Error>> {
    let mut worker = FullFlowWorker::start().await?;
    let result = async {
        let user_id = seed_user(&worker.pool, "Ultimate").await?;
        let product_query = format!("Worker percolator product {user_id}");
        let filter = search_filter(
            user_id,
            UserSearchFilterName::from("Ignored event filter"),
            ResourceState::Active,
            &product_query,
        )?;
        worker.project_filter(&filter).await?;
        refresh_index("user_search_filters").await;
        let (product_id, _) = seed_product(&worker.pool, &product_query).await?;
        worker.start_scoped_sequin_subscription().await;

        let policy_event =
            insert_product_event(&worker.pool, product_id, "POLICY_ACCEPTED", "POLICY").await?;
        let lifecycle_event =
            insert_product_event(&worker.pool, product_id, "LIFECYCLE_DELETED", "LIFECYCLE")
                .await?;

        assert_no_matches_for(&worker.pool, policy_event, NO_SIDE_EFFECT_OBSERVATION).await?;
        assert_no_matches_for(&worker.pool, lifecycle_event, NO_SIDE_EFFECT_OBSERVATION).await?;
        assert_no_more_than_notifications(
            &worker.notifications,
            user_id,
            0,
            NO_SIDE_EFFECT_OBSERVATION,
        )
        .await
    }
    .await;

    worker.finish(result).await
}

async fn rolled_back_product_event_flow() -> Result<(), Box<dyn std::error::Error>> {
    let mut worker = FullFlowWorker::start().await?;
    let result = async {
        let user_id = seed_user(&worker.pool, "Ultimate").await?;
        let product_query = format!("Worker percolator product {user_id}");
        let filter = search_filter(
            user_id,
            UserSearchFilterName::from("Rollback filter"),
            ResourceState::Active,
            &product_query,
        )?;
        worker.project_filter(&filter).await?;
        refresh_index("user_search_filters").await;
        let (product_id, _) = seed_product(&worker.pool, &product_query).await?;
        worker.start_scoped_sequin_subscription().await;

        let event_id = insert_product_event_then_rollback(
            &worker.pool,
            product_id,
            "PRODUCT_STATE_CHANGED",
            "DOMAIN",
        )
        .await?;

        assert_event_is_not_persisted(&worker.pool, event_id).await?;
        assert_no_matches_for(&worker.pool, event_id, NO_SIDE_EFFECT_OBSERVATION).await?;
        assert_no_more_than_notifications(
            &worker.notifications,
            user_id,
            0,
            NO_SIDE_EFFECT_OBSERVATION,
        )
        .await
    }
    .await;

    worker.finish(result).await
}

async fn redelivery_and_deterministic_selection_flow() -> Result<(), Box<dyn std::error::Error>> {
    let mut worker = FullFlowWorker::start().await?;
    let result = async {
        let user_id = seed_user(&worker.pool, "Ultimate").await?;
        let product_query = format!("Worker percolator product {user_id}");
        let first_filter = search_filter(
            user_id,
            UserSearchFilterName::from("First deterministic filter"),
            ResourceState::Active,
            &product_query,
        )?;
        let second_filter = search_filter(
            user_id,
            UserSearchFilterName::from("Second deterministic filter"),
            ResourceState::Active,
            &product_query,
        )?;
        let selected_filter_id = [first_filter.id(), second_filter.id()]
            .into_iter()
            .min_by_key(ToString::to_string)
            .ok_or_else(|| std::io::Error::other("notification candidates are missing"))?;
        worker.project_filter(&first_filter).await?;
        worker.project_filter(&second_filter).await?;
        refresh_index("user_search_filters").await;
        let (product_id, _) = seed_product(&worker.pool, &product_query).await?;
        worker.start_scoped_sequin_subscription().await;

        let event_id =
            insert_product_event(&worker.pool, product_id, "PRODUCT_STATE_CHANGED", "DOMAIN")
                .await?;
        wait_for_match(&worker.pool, event_id, 2).await?;
        let notifications = wait_for_notifications(&worker.notifications, user_id, 1).await?;
        assert_search_filter_notification(
            &notifications[0],
            user_id,
            selected_filter_id,
            product_id,
            event_id,
        )?;

        redeliver_product_event(&worker.server, product_id, event_id).await?;
        assert_match_count_for_duration(&worker.pool, event_id, 2, NO_SIDE_EFFECT_OBSERVATION)
            .await?;
        redeliver_search_filter_match(
            &worker.server,
            user_id,
            selected_filter_id,
            product_id,
            event_id,
        )
        .await?;
        assert_no_more_than_notifications(
            &worker.notifications,
            user_id,
            1,
            NO_SIDE_EFFECT_OBSERVATION,
        )
        .await
    }
    .await;

    worker.finish(result).await
}

struct FullFlowWorker {
    pool: sqlx::PgPool,
    index: OpenSearchSearchFilterIndex,
    notifications: DynamoDbAllNotificationsReader<'static>,
    server: ScopedWorkerServer,
    scoped_sequin: Option<RunningSequin>,
    _unused_receivers: aura_historia_worker::cdc::WorkerQueueReceivers,
    percolator_consumer: JoinHandle<()>,
    notification_consumer: JoinHandle<()>,
}

impl FullFlowWorker {
    async fn start() -> Result<Self, Box<dyn std::error::Error>> {
        let pool = get_postgres_client().await;
        let index = OpenSearchSearchFilterIndex::new(get_opensearch_client().await.clone());
        let dynamodb = get_dynamodb_client().await;
        let percolator_handler: Arc<dyn MatchProductEventUseCase> =
            Arc::new(MatchProductEventHandler::new(
                SqlxUnitOfWork::new(pool.clone()),
                index.clone(),
                NonMatchingProductMatchEvaluator,
                SqlxActiveSearchFilterMatchCandidateReaderFactory,
                SqlxSearchFilterMatchWriterFactory,
            ));
        let notification_handler: Arc<dyn GenerateSearchFilterMatchNotificationUseCase> =
            Arc::new(GenerateSearchFilterMatchNotificationHandler::new(
                SqlxUnitOfWork::new(pool.clone()),
                SqlxSearchFilterMonthlyMatchQuotaReaderFactory,
                SqlxUserTierEntitlementsFactory::new(),
                CreateNotificationHandler::new(ConditionalDynamoDbNotificationWriter::new(
                    dynamodb.clone(),
                    "table_1",
                )),
            ));
        let (runtime, mut receivers) = WorkerRuntime::with_all_queues(QueueConfig::new(16))?;
        let percolator_receiver = receivers
            .take(WorkerQueue::SearchFilterPercolator)
            .ok_or_else(|| std::io::Error::other("search-filter percolator queue is missing"))?;
        let notification_receiver = receivers
            .take(WorkerQueue::SearchFilterMatchNotification)
            .ok_or_else(|| {
                std::io::Error::other("search-filter match notification queue is missing")
            })?;
        let percolator_consumer = tokio::spawn(consume_search_filter_percolator_queue(
            percolator_receiver,
            percolator_handler,
            SqlxUnitOfWork::new(pool.clone()),
            SqlxProductSearchFilterMatchSourceReaderFactory::new(),
        ));
        let notification_consumer = tokio::spawn(consume_search_filter_match_notification_queue(
            notification_receiver,
            notification_handler,
            SqlxUnitOfWork::new(pool.clone()),
            SqlxSearchFilterMatchNotificationSourceReaderFactory,
            SqlxUnitOfWork::new(pool.clone()),
            SqlxProductSearchFilterMatchSourceReaderFactory::new(),
        ));
        let server = ScopedWorkerServer::start(runtime).await?;

        Ok(Self {
            pool,
            index,
            notifications: DynamoDbAllNotificationsReader::new(dynamodb, "table_1"),
            server,
            scoped_sequin: None,
            _unused_receivers: receivers,
            percolator_consumer,
            notification_consumer,
        })
    }

    async fn project_filter(
        &self,
        filter: &SearchFilter,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let version = insert_filter(&self.pool, filter).await?;
        ProjectSearchFilterChangeHandler::new(
            SqlxSearchFilterIndexReader::new(self.pool.clone()),
            self.index.clone(),
        )
        .execute(ProjectSearchFilterChangeCommand {
            search_filter_id: filter.id(),
            source_version: version,
            operation: SearchFilterProjectionOperation::Upsert,
        })
        .await?;
        Ok(())
    }

    async fn start_scoped_sequin_subscription(&mut self) {
        self.scoped_sequin = Some(
            start_sequin_for_tables(
                &self.server.webhook_url(),
                &["public.product_events", "public.search_filter_matches"],
            )
            .await,
        );
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
        let Self {
            server,
            scoped_sequin,
            _unused_receivers,
            percolator_consumer,
            notification_consumer,
            ..
        } = self;
        let sequin_shutdown_result = match scoped_sequin {
            Some(sequin) => sequin.stop_delivery().await,
            None => Ok(()),
        };
        let server_shutdown_result = server.shutdown().await;
        let (percolator_shutdown_result, notification_shutdown_result) =
            tokio::join!(percolator_consumer, notification_consumer);

        drop(_unused_receivers);
        sequin_shutdown_result?;
        server_shutdown_result?;
        percolator_shutdown_result?;
        notification_shutdown_result?;
        Ok(())
    }
}

struct ScopedWorkerServer {
    address: SocketAddr,
    shutdown_tx: oneshot::Sender<()>,
    server: JoinHandle<Result<(), WorkerRunError>>,
}

impl ScopedWorkerServer {
    async fn start(runtime: WorkerRuntime) -> Result<Self, std::io::Error> {
        let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await?;
        let address = listener.local_addr()?;
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server = tokio::spawn(serve_with_runtime(listener, runtime, async move {
            let _ = shutdown_rx.await;
        }));

        Ok(Self {
            address,
            shutdown_tx,
            server,
        })
    }

    fn webhook_url(&self) -> String {
        format!(
            "http://host.docker.internal:{}/cdc/sequin",
            self.address.port()
        )
    }

    fn local_webhook_url(&self) -> String {
        format!("http://127.0.0.1:{}/cdc/sequin", self.address.port())
    }

    async fn shutdown(self) -> Result<(), Box<dyn std::error::Error>> {
        self.shutdown_tx
            .send(())
            .map_err(|_| std::io::Error::other("worker server shutdown channel closed"))?;
        self.server.await??;
        Ok(())
    }
}

async fn seed_user(pool: &sqlx::PgPool, tier: &str) -> Result<UserId, sqlx::Error> {
    let user_id = UserId::new();
    sqlx::query("INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, $3, 'User')")
        .bind(uuid::Uuid::from(user_id))
        .bind(format!("worker-percolator-{user_id}@example.test"))
        .bind(tier)
        .execute(pool)
        .await?;
    Ok(user_id)
}

async fn seed_product(
    pool: &sqlx::PgPool,
    title: &str,
) -> Result<(common::product_id::ProductId, EventId), sqlx::Error> {
    seed_product_with_event(pool, "LIFECYCLE", title).await
}

async fn create_product_with_domain_event(
    pool: &sqlx::PgPool,
    title: &str,
) -> Result<(common::product_id::ProductId, EventId), sqlx::Error> {
    seed_product_with_event(pool, "DOMAIN", title).await
}

async fn seed_product_with_event(
    pool: &sqlx::PgPool,
    event_group: &str,
    title: &str,
) -> Result<(common::product_id::ProductId, EventId), sqlx::Error> {
    let product_id = common::product_id::ProductId::new();
    let product_uuid = uuid::Uuid::from(product_id);
    let event_id = EventId::new();
    let shop_id = uuid::Uuid::new_v4();
    let product_slug_suffix = product_uuid.simple().to_string()[..6].to_owned();
    let mut tx = pool.begin().await?;
    sqlx::query("INSERT INTO shops (shop_id, shop_slug_id, name, shop_type, partner_status, shop_domains) VALUES ($1, $2, $3, 'COMMERCIAL_DEALER', 'SCRAPED', '{}')")
        .bind(shop_id)
        .bind(format!("worker-percolator-shop-{shop_id}"))
        .bind("Worker percolator shop")
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO products (product_id, product_slug_id, event_id, shop_id, seller_id, shops_product_id, title_text, title_language, description_text, description_language, state, lifecycle, url, product_images) VALUES ($1, $2, $3, $4, $4, $5, $6, 'en', 'Worker percolator description', 'en', 'LISTED', 'ACTIVE', 'https://example.test/product', '[]')")
        .bind(product_uuid)
        .bind(format!("worker-percolator-product-{product_slug_suffix}"))
        .bind(uuid::Uuid::from(event_id))
        .bind(shop_id)
        .bind(product_uuid.to_string())
        .bind(title)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO product_events (event_id, product_id, event_type, event_group, payload, event_time) VALUES ($1, $2, 'PRODUCT_CREATED', $3, '{}', now())")
        .bind(uuid::Uuid::from(event_id))
        .bind(product_uuid)
        .bind(event_group)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok((product_id, event_id))
}

fn search_filter(
    user_id: UserId,
    name: UserSearchFilterName,
    state: ResourceState,
    product_query: &str,
) -> Result<SearchFilter, Box<dyn std::error::Error>> {
    Ok(SearchFilter::create(NewSearchFilter {
        user_search_filter_id: UserSearchFilterId::new(),
        user_id,
        name,
        notifications: true,
        state,
        search: ProductSearch::new(Language::En, Currency::Eur)
            .with_product_query(product_query.try_into()?),
        embedding: None,
    }))
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

async fn insert_product_event(
    pool: &sqlx::PgPool,
    product_id: common::product_id::ProductId,
    event_type: &str,
    event_group: &str,
) -> Result<EventId, sqlx::Error> {
    let event_id = EventId::new();
    sqlx::query("INSERT INTO product_events (event_id, product_id, event_type, event_group, payload, event_time) VALUES ($1, $2, $3, $4, '{}', now())")
        .bind(uuid::Uuid::from(event_id))
        .bind(uuid::Uuid::from(product_id))
        .bind(event_type)
        .bind(event_group)
        .execute(pool)
        .await?;
    Ok(event_id)
}

async fn update_product_and_insert_event(
    pool: &sqlx::PgPool,
    product_id: common::product_id::ProductId,
    title: &str,
) -> Result<EventId, sqlx::Error> {
    let event_id = EventId::new();
    let mut tx = pool.begin().await?;
    sqlx::query("INSERT INTO product_events (event_id, product_id, event_type, event_group, payload, event_time) VALUES ($1, $2, 'PRODUCT_STATE_CHANGED', 'DOMAIN', '{}', now())")
        .bind(uuid::Uuid::from(event_id))
        .bind(uuid::Uuid::from(product_id))
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "UPDATE products SET event_id = $1, title_text = $2, updated = now() WHERE product_id = $3",
    )
    .bind(uuid::Uuid::from(event_id))
    .bind(title)
    .bind(uuid::Uuid::from(product_id))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(event_id)
}

async fn insert_product_event_then_rollback(
    pool: &sqlx::PgPool,
    product_id: common::product_id::ProductId,
    event_type: &str,
    event_group: &str,
) -> Result<EventId, sqlx::Error> {
    let event_id = EventId::new();
    let mut tx = pool.begin().await?;
    sqlx::query("INSERT INTO product_events (event_id, product_id, event_type, event_group, payload, event_time) VALUES ($1, $2, $3, $4, '{}', now())")
        .bind(uuid::Uuid::from(event_id))
        .bind(uuid::Uuid::from(product_id))
        .bind(event_type)
        .bind(event_group)
        .execute(&mut *tx)
        .await?;
    drop(tx);
    Ok(event_id)
}

async fn insert_search_filter_match(
    pool: &sqlx::PgPool,
    user_id: UserId,
    search_filter_id: UserSearchFilterId,
    product_id: common::product_id::ProductId,
    origin_event_id: EventId,
) -> Result<(), Box<dyn std::error::Error>> {
    sqlx::query(
        "INSERT INTO search_filter_matches (user_id, user_search_filter_id, product_id, origin_event_id, user_search_filter_name) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(uuid::Uuid::from(user_id))
    .bind(uuid::Uuid::parse_str(&search_filter_id.to_string())?)
    .bind(uuid::Uuid::from(product_id))
    .bind(uuid::Uuid::from(origin_event_id))
    .bind("Free tier quota filter")
    .execute(pool)
    .await?;
    Ok(())
}

async fn age_match_before_current_event(
    pool: &sqlx::PgPool,
    search_filter_id: UserSearchFilterId,
    product_id: common::product_id::ProductId,
) -> Result<(), Box<dyn std::error::Error>> {
    sqlx::query(
        "UPDATE search_filter_matches SET created = now() - INTERVAL '1 day', updated = now() - INTERVAL '1 day' WHERE user_search_filter_id = $1 AND product_id = $2",
    )
    .bind(uuid::Uuid::parse_str(&search_filter_id.to_string())?)
    .bind(uuid::Uuid::from(product_id))
    .execute(pool)
    .await?;
    Ok(())
}

async fn product_event_type(pool: &sqlx::PgPool, event_id: EventId) -> Result<String, sqlx::Error> {
    sqlx::query_scalar("SELECT event_type FROM product_events WHERE event_id = $1")
        .bind(uuid::Uuid::from(event_id))
        .fetch_one(pool)
        .await
}

async fn wait_for_match(
    pool: &sqlx::PgPool,
    event_id: EventId,
    expected: i64,
) -> Result<(), Box<dyn std::error::Error>> {
    for _ in 0..POLL_ATTEMPTS {
        if match_count(pool, event_id).await? == expected {
            return Ok(());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err(std::io::Error::other(format!(
        "product event {event_id} did not create {expected} search-filter matches"
    ))
    .into())
}

async fn match_count(pool: &sqlx::PgPool, event_id: EventId) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT count(*) FROM search_filter_matches WHERE origin_event_id = $1")
        .bind(uuid::Uuid::from(event_id))
        .fetch_one(pool)
        .await
}

async fn matches_for_event(
    pool: &sqlx::PgPool,
    event_id: EventId,
) -> Result<Vec<UserSearchFilterId>, Box<dyn std::error::Error>> {
    let ids = sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT user_search_filter_id FROM search_filter_matches WHERE origin_event_id = $1 ORDER BY user_search_filter_id",
    )
    .bind(uuid::Uuid::from(event_id))
    .fetch_all(pool)
    .await?;
    Ok(ids.into_iter().map(UserSearchFilterId::from).collect())
}

async fn assert_match_for_event(
    pool: &sqlx::PgPool,
    event_id: EventId,
    user_id: UserId,
    search_filter_id: UserSearchFilterId,
    product_id: common::product_id::ProductId,
) -> Result<(), Box<dyn std::error::Error>> {
    let (matched_user_id, matched_search_filter_id, matched_product_id): (
        uuid::Uuid,
        uuid::Uuid,
        uuid::Uuid,
    ) = sqlx::query_as(
        "SELECT user_id, user_search_filter_id, product_id FROM search_filter_matches WHERE origin_event_id = $1",
    )
    .bind(uuid::Uuid::from(event_id))
    .fetch_one(pool)
    .await?;

    assert_eq!(uuid::Uuid::from(user_id), matched_user_id);
    assert_eq!(
        uuid::Uuid::parse_str(&search_filter_id.to_string())?,
        matched_search_filter_id
    );
    assert_eq!(uuid::Uuid::from(product_id), matched_product_id);
    Ok(())
}

async fn assert_match_count_for_duration(
    pool: &sqlx::PgPool,
    event_id: EventId,
    expected: i64,
    duration: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = Instant::now() + duration;
    loop {
        if match_count(pool, event_id).await? != expected {
            return Err(std::io::Error::other(format!(
                "product event {event_id} did not remain at {expected} search-filter matches"
            ))
            .into());
        }
        if Instant::now() >= deadline {
            return Ok(());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

async fn assert_no_matches_for(
    pool: &sqlx::PgPool,
    event_id: EventId,
    duration: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    assert_match_count_for_duration(pool, event_id, 0, duration).await
}

async fn assert_event_is_not_persisted(
    pool: &sqlx::PgPool,
    event_id: EventId,
) -> Result<(), Box<dyn std::error::Error>> {
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM product_events WHERE event_id = $1)")
            .bind(uuid::Uuid::from(event_id))
            .fetch_one(pool)
            .await?;
    assert!(
        !exists,
        "rolled-back product event {event_id} was persisted"
    );
    Ok(())
}

async fn wait_for_notifications(
    reader: &DynamoDbAllNotificationsReader<'_>,
    user_id: UserId,
    expected: usize,
) -> Result<Vec<AllNotificationsReadItem>, Box<dyn std::error::Error>> {
    for _ in 0..POLL_ATTEMPTS {
        let notifications = reader.list_all_by_user(&user_id).await?;
        if notifications.len() == expected {
            return Ok(notifications);
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err(std::io::Error::other(format!(
        "user {user_id} did not receive {expected} notifications"
    ))
    .into())
}

async fn assert_no_more_than_notifications(
    reader: &DynamoDbAllNotificationsReader<'_>,
    user_id: UserId,
    maximum: usize,
    duration: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = Instant::now() + duration;
    loop {
        let notifications = reader.list_all_by_user(&user_id).await?;
        if notifications.len() > maximum {
            return Err(std::io::Error::other(format!(
                "user {user_id} received {} notifications; expected at most {maximum}",
                notifications.len()
            ))
            .into());
        }
        if Instant::now() >= deadline {
            return Ok(());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

fn assert_search_filter_notification(
    notification: &AllNotificationsReadItem,
    user_id: UserId,
    search_filter_id: UserSearchFilterId,
    product_id: common::product_id::ProductId,
    origin_event_id: EventId,
) -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(user_id, notification.user_id);
    assert_eq!(origin_event_id, notification.origin_event_id);
    let NotificationPayload::SearchFilter {
        product_id: notification_product_id,
        search_filter_payload,
        ..
    } = &notification.notification_payload
    else {
        return Err(std::io::Error::other("expected a search-filter notification").into());
    };
    assert_eq!(&product_id, notification_product_id);
    assert_eq!(
        &search_filter_id,
        &search_filter_payload.user_search_filter_id
    );
    Ok(())
}

async fn redeliver_product_event(
    server: &ScopedWorkerServer,
    product_id: common::product_id::ProductId,
    event_id: EventId,
) -> Result<(), Box<dyn std::error::Error>> {
    post_sequin_change(
        server.local_webhook_url(),
        json!({
            "record": {
                "event_id": event_id.to_string(),
                "product_id": product_id.to_string(),
                "event_type": "PRODUCT_STATE_CHANGED",
                "event_group": "DOMAIN"
            },
            "action": "insert",
            "metadata": {"table_schema": "public", "table_name": "product_events"}
        }),
    )
    .await
}

async fn redeliver_search_filter_match(
    server: &ScopedWorkerServer,
    user_id: UserId,
    search_filter_id: UserSearchFilterId,
    product_id: common::product_id::ProductId,
    origin_event_id: EventId,
) -> Result<(), Box<dyn std::error::Error>> {
    post_sequin_change(
        server.local_webhook_url(),
        json!({
            "record": {
                "user_id": user_id.to_string(),
                "user_search_filter_id": search_filter_id.to_string(),
                "product_id": product_id.to_string(),
                "origin_event_id": origin_event_id.to_string()
            },
            "action": "insert",
            "metadata": {"table_schema": "public", "table_name": "search_filter_matches"}
        }),
    )
    .await
}

async fn post_sequin_change(
    webhook_url: String,
    change: serde_json::Value,
) -> Result<(), Box<dyn std::error::Error>> {
    let response = reqwest::Client::new()
        .post(webhook_url)
        .json(&change)
        .send()
        .await?;
    assert_eq!(reqwest::StatusCode::ACCEPTED, response.status());
    Ok(())
}
