use aura_historia_worker::cdc::WorkerQueue;
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
    sync::Arc,
    time::{Duration, Instant},
};
use test_api::{
    DynamoDB, IntegrationTestService, OpenSearch, Postgres, aura_integration_test,
    get_dynamodb_client, get_opensearch_client, get_postgres_client,
    get_sequin_worker_webhook_bind_addr, refresh_index,
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

#[aura_integration_test(services = [BUSINESS_SCHEMA, OpenSearch()])]
async fn should_match_non_enhanced_filter_for_committed_product_event() {
    let result = match_non_enhanced_filter_for_committed_product_event().await;

    assert!(
        result.is_ok(),
        "search-filter percolator happy-path acceptance test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OpenSearch()])]
async fn should_ignore_policy_and_lifecycle_product_events() {
    let result = ignore_policy_and_lifecycle_product_events().await;

    assert!(
        result.is_ok(),
        "search-filter percolator ignored-event acceptance test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OpenSearch()])]
async fn should_not_match_rolled_back_product_event() {
    let result = not_match_rolled_back_product_event().await;

    assert!(
        result.is_ok(),
        "search-filter percolator rollback acceptance test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OpenSearch()])]
async fn should_preserve_one_match_on_product_event_redelivery() {
    let result = preserve_one_match_on_product_event_redelivery().await;

    assert!(
        result.is_ok(),
        "search-filter percolator redelivery acceptance test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DynamoDB()])]
async fn should_create_one_notification_from_committed_search_filter_match() {
    let result = create_one_notification_from_committed_search_filter_match().await;

    assert!(
        result.is_ok(),
        "search-filter match notification CDC acceptance test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DynamoDB()])]
async fn should_select_lowest_filter_id_for_one_user_event_notification() {
    let result = select_lowest_filter_id_for_one_user_event_notification().await;

    assert!(
        result.is_ok(),
        "search-filter match notification selection acceptance test failed: {result:?}"
    );
}

async fn match_non_enhanced_filter_for_committed_product_event()
-> Result<(), Box<dyn std::error::Error>> {
    let worker = PercolatorWorker::start().await?;
    let result = async {
        let (_user_id, product_id) = worker.prepare_matching_fixture().await?;
        let _sequin = worker.start_sequin().await;

        let event_id =
            insert_product_event(&worker.pool, product_id, "PRODUCT_STATE_CHANGED", "DOMAIN")
                .await?;

        wait_for_match(&worker.pool, event_id, 1).await?;
        Ok(())
    }
    .await;

    worker.finish(result).await
}

async fn ignore_policy_and_lifecycle_product_events() -> Result<(), Box<dyn std::error::Error>> {
    let worker = PercolatorWorker::start().await?;
    let result = async {
        let (_user_id, product_id) = worker.prepare_matching_fixture().await?;
        let _sequin = worker.start_sequin().await;

        let policy_event =
            insert_product_event(&worker.pool, product_id, "POLICY_ACCEPTED", "POLICY").await?;
        let lifecycle_event =
            insert_product_event(&worker.pool, product_id, "LIFECYCLE_DELETED", "LIFECYCLE")
                .await?;

        assert_no_matches_for(&worker.pool, policy_event, NO_SIDE_EFFECT_OBSERVATION).await?;
        assert_no_matches_for(&worker.pool, lifecycle_event, NO_SIDE_EFFECT_OBSERVATION).await
    }
    .await;

    worker.finish(result).await
}

async fn not_match_rolled_back_product_event() -> Result<(), Box<dyn std::error::Error>> {
    let worker = PercolatorWorker::start().await?;
    let result = async {
        let (_user_id, product_id) = worker.prepare_matching_fixture().await?;
        let _sequin = worker.start_sequin().await;

        let event_id = insert_product_event_then_rollback(
            &worker.pool,
            product_id,
            "PRODUCT_STATE_CHANGED",
            "DOMAIN",
        )
        .await?;

        assert_event_is_not_persisted(&worker.pool, event_id).await?;
        assert_no_matches_for(&worker.pool, event_id, NO_SIDE_EFFECT_OBSERVATION).await
    }
    .await;

    worker.finish(result).await
}

async fn preserve_one_match_on_product_event_redelivery() -> Result<(), Box<dyn std::error::Error>>
{
    let worker = PercolatorWorker::start().await?;
    let result = async {
        let (_user_id, product_id) = worker.prepare_matching_fixture().await?;
        let _sequin = worker.start_sequin().await;

        let event_id =
            insert_product_event(&worker.pool, product_id, "PRODUCT_STATE_CHANGED", "DOMAIN")
                .await?;
        wait_for_match(&worker.pool, event_id, 1).await?;

        let response = reqwest::Client::new()
            .post(format!(
                "http://{}/cdc/sequin",
                get_sequin_worker_webhook_bind_addr()
            ))
            .json(&json!({
                "record": {
                    "event_id": event_id.to_string(),
                    "product_id": product_id.to_string(),
                    "event_type": "PRODUCT_STATE_CHANGED",
                    "event_group": "DOMAIN"
                },
                "action": "insert",
                "metadata": {"table_schema": "public", "table_name": "product_events"}
            }))
            .send()
            .await?;
        assert_eq!(reqwest::StatusCode::ACCEPTED, response.status());

        assert_match_count(&worker.pool, event_id, 1).await
    }
    .await;

    worker.finish(result).await
}

async fn create_one_notification_from_committed_search_filter_match()
-> Result<(), Box<dyn std::error::Error>> {
    let pool = get_postgres_client().await;
    let user_id = seed_user(&pool).await?;
    let product_id = seed_product(&pool).await?;
    let filter = search_filter(user_id)?;
    insert_filter(&pool, &filter).await?;
    let origin_event_id: uuid::Uuid =
        sqlx::query_scalar("SELECT event_id FROM products WHERE product_id = $1")
            .bind(uuid::Uuid::from(product_id))
            .fetch_one(&pool)
            .await?;
    let worker = MatchNotificationWorker::start(pool.clone()).await?;
    let result = async {
        let _sequin = worker.start_sequin().await;
        insert_search_filter_match(
            &pool,
            user_id,
            filter.id(),
            product_id,
            EventId::from(origin_event_id),
        )
        .await?;

        let notifications = wait_for_notifications(&worker.notifications, user_id, 1).await?;
        assert!(matches!(
            notifications[0].notification_payload,
            NotificationPayload::SearchFilter { .. }
        ));
        assert_eq!(
            EventId::from(origin_event_id),
            notifications[0].origin_event_id
        );

        let response = reqwest::Client::new()
            .post(format!(
                "http://{}/cdc/sequin",
                get_sequin_worker_webhook_bind_addr()
            ))
            .json(&json!({
                "record": {
                    "user_id": user_id.to_string(),
                    "user_search_filter_id": filter.id().to_string(),
                    "product_id": product_id.to_string(),
                    "origin_event_id": origin_event_id.to_string()
                },
                "action": "insert",
                "metadata": {"table_schema": "public", "table_name": "search_filter_matches"}
            }))
            .send()
            .await?;
        assert_eq!(reqwest::StatusCode::ACCEPTED, response.status());
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

async fn select_lowest_filter_id_for_one_user_event_notification()
-> Result<(), Box<dyn std::error::Error>> {
    let pool = get_postgres_client().await;
    let user_id = seed_user(&pool).await?;
    let product_id = seed_product(&pool).await?;
    let first_filter = search_filter_with_name(
        user_id,
        UserSearchFilterName::from("First notification candidate"),
    )?;
    let second_filter = search_filter_with_name(
        user_id,
        UserSearchFilterName::from("Second notification candidate"),
    )?;
    insert_filter(&pool, &first_filter).await?;
    insert_filter(&pool, &second_filter).await?;
    let origin_event_id: uuid::Uuid =
        sqlx::query_scalar("SELECT event_id FROM products WHERE product_id = $1")
            .bind(uuid::Uuid::from(product_id))
            .fetch_one(&pool)
            .await?;
    let selected_filter_id = [first_filter.id(), second_filter.id()]
        .into_iter()
        .min_by_key(ToString::to_string)
        .ok_or_else(|| std::io::Error::other("notification candidates are missing"))?;
    let worker = MatchNotificationWorker::start(pool.clone()).await?;
    let result = async {
        let _sequin = worker.start_sequin().await;
        insert_search_filter_matches(
            &pool,
            user_id,
            &[first_filter.id(), second_filter.id()],
            product_id,
            EventId::from(origin_event_id),
        )
        .await?;

        let notifications = wait_for_notifications(&worker.notifications, user_id, 1).await?;
        let NotificationPayload::SearchFilter {
            search_filter_payload,
            ..
        } = &notifications[0].notification_payload
        else {
            return Err(std::io::Error::other("expected a search-filter notification").into());
        };
        assert_eq!(
            selected_filter_id,
            search_filter_payload.user_search_filter_id
        );
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

struct MatchNotificationWorker {
    notifications: DynamoDbAllNotificationsReader<'static>,
    consumer: JoinHandle<()>,
    shutdown_tx: oneshot::Sender<()>,
    server: JoinHandle<Result<(), WorkerRunError>>,
}

impl MatchNotificationWorker {
    async fn start(pool: sqlx::PgPool) -> Result<Self, Box<dyn std::error::Error>> {
        let dynamodb = get_dynamodb_client().await;
        let handler: Arc<dyn GenerateSearchFilterMatchNotificationUseCase> =
            Arc::new(GenerateSearchFilterMatchNotificationHandler::new(
                SqlxUnitOfWork::new(pool.clone()),
                SqlxSearchFilterMonthlyMatchQuotaReaderFactory,
                SqlxUserTierEntitlementsFactory::new(),
                CreateNotificationHandler::new(ConditionalDynamoDbNotificationWriter::new(
                    dynamodb.clone(),
                    "table_1",
                )),
            ));
        let (runtime, mut receivers) =
            WorkerRuntime::with_search_filter_match_notification_queue(QueueConfig::new(16))?;
        let receiver = receivers
            .take(WorkerQueue::SearchFilterMatchNotification)
            .ok_or_else(|| {
                std::io::Error::other("search-filter match notification queue is missing")
            })?;
        let consumer = tokio::spawn(
            aura_historia_worker::search_filter_match_notifications::consume_search_filter_match_notification_queue(
                receiver,
                handler,
                SqlxUnitOfWork::new(pool.clone()),
                SqlxSearchFilterMatchNotificationSourceReaderFactory,
                SqlxUnitOfWork::new(pool),
                SqlxProductSearchFilterMatchSourceReaderFactory::new(),
            ),
        );
        let listener = tokio::net::TcpListener::bind(get_sequin_worker_webhook_bind_addr()).await?;
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server = tokio::spawn(serve_with_runtime(listener, runtime, async move {
            let _ = shutdown_rx.await;
        }));

        Ok(Self {
            notifications: DynamoDbAllNotificationsReader::new(dynamodb, "table_1"),
            consumer,
            shutdown_tx,
            server,
        })
    }

    async fn start_sequin(&self) -> test_api::RunningSequin {
        test_api::start_sequin_for_tables(
            &format!(
                "http://host.docker.internal:{}/cdc/sequin",
                get_sequin_worker_webhook_bind_addr().port(),
            ),
            &["public.search_filter_matches"],
        )
        .await
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
        self.consumer.await?;
        Ok(())
    }
}

struct PercolatorWorker {
    pool: sqlx::PgPool,
    index: OpenSearchSearchFilterIndex,

    consumer: JoinHandle<()>,
    shutdown_tx: oneshot::Sender<()>,
    server: JoinHandle<Result<(), WorkerRunError>>,
}

impl PercolatorWorker {
    async fn start() -> Result<Self, Box<dyn std::error::Error>> {
        let pool = get_postgres_client().await;

        let index = OpenSearchSearchFilterIndex::new(get_opensearch_client().await.clone());
        let handler: Arc<dyn MatchProductEventUseCase> = Arc::new(MatchProductEventHandler::new(
            SqlxUnitOfWork::new(pool.clone()),
            index.clone(),
            NonMatchingProductMatchEvaluator,
            SqlxActiveSearchFilterMatchCandidateReaderFactory,
            SqlxSearchFilterMatchWriterFactory,
        ));
        let (runtime, mut receivers) =
            WorkerRuntime::with_search_filter_percolator_queue(QueueConfig::new(16))?;
        let receiver = receivers
            .take(WorkerQueue::SearchFilterPercolator)
            .ok_or_else(|| std::io::Error::other("search-filter percolator queue is missing"))?;
        let consumer = tokio::spawn(consume_search_filter_percolator_queue(
            receiver,
            handler,
            SqlxUnitOfWork::new(pool.clone()),
            SqlxProductSearchFilterMatchSourceReaderFactory::new(),
        ));
        let listener = tokio::net::TcpListener::bind(get_sequin_worker_webhook_bind_addr()).await?;
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server = tokio::spawn(serve_with_runtime(listener, runtime, async move {
            let _ = shutdown_rx.await;
        }));

        Ok(Self {
            pool,
            index,

            consumer,
            shutdown_tx,
            server,
        })
    }

    async fn prepare_matching_fixture(
        &self,
    ) -> Result<(UserId, common::product_id::ProductId), Box<dyn std::error::Error>> {
        let user_id = seed_user(&self.pool).await?;
        let product_id = seed_product(&self.pool).await?;
        let filter = search_filter(user_id)?;
        let version = insert_filter(&self.pool, &filter).await?;
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
        refresh_index("user_search_filters").await;
        Ok((user_id, product_id))
    }

    async fn start_sequin(&self) -> test_api::RunningSequin {
        test_api::start_sequin_for_tables(
            &format!(
                "http://host.docker.internal:{}/cdc/sequin",
                get_sequin_worker_webhook_bind_addr().port(),
            ),
            &["public.product_events"],
        )
        .await
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
        self.consumer.await?;
        Ok(())
    }
}

async fn seed_user(pool: &sqlx::PgPool) -> Result<UserId, sqlx::Error> {
    let user_id = UserId::new();
    sqlx::query(
        "INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'Ultimate', 'User')",
    )
    .bind(uuid::Uuid::from(user_id))
    .bind(format!("worker-percolator-{user_id}@example.test"))
    .execute(pool)
    .await?;
    Ok(user_id)
}

async fn seed_product(pool: &sqlx::PgPool) -> Result<common::product_id::ProductId, sqlx::Error> {
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
    sqlx::query("INSERT INTO products (product_id, product_slug_id, event_id, shop_id, seller_id, shops_product_id, title_text, title_language, state, lifecycle, url, product_images) VALUES ($1, $2, $3, $4, $4, $5, 'Worker percolator product', 'en', 'LISTED', 'ACTIVE', 'https://example.test/product', '[]')")
        .bind(product_uuid)
        .bind(format!("worker-percolator-product-{product_slug_suffix}"))
        .bind(uuid::Uuid::from(event_id))
        .bind(shop_id)
        .bind(product_uuid.to_string())
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO product_events (event_id, product_id, event_type, event_group, payload, event_time) VALUES ($1, $2, 'PRODUCT_CREATED', 'DOMAIN', '{}', now())")
        .bind(uuid::Uuid::from(event_id))
        .bind(product_uuid)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(product_id)
}

fn search_filter(user_id: UserId) -> Result<SearchFilter, Box<dyn std::error::Error>> {
    search_filter_with_name(
        user_id,
        UserSearchFilterName::from("Percolator acceptance filter"),
    )
}

fn search_filter_with_name(
    user_id: UserId,
    name: UserSearchFilterName,
) -> Result<SearchFilter, Box<dyn std::error::Error>> {
    Ok(SearchFilter::create(NewSearchFilter {
        user_search_filter_id: UserSearchFilterId::new(),
        user_id,
        name,
        notifications: true,
        state: ResourceState::Active,
        search: ProductSearch::new(Language::En, Currency::Eur)
            .with_product_query("Worker percolator product".try_into()?),
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
    insert_search_filter_matches(
        pool,
        user_id,
        &[search_filter_id],
        product_id,
        origin_event_id,
    )
    .await
}

async fn insert_search_filter_matches(
    pool: &sqlx::PgPool,
    user_id: UserId,
    search_filter_ids: &[UserSearchFilterId],
    product_id: common::product_id::ProductId,
    origin_event_id: EventId,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut transaction = pool.begin().await?;
    for search_filter_id in search_filter_ids {
        sqlx::query(
            "INSERT INTO search_filter_matches (user_id, user_search_filter_id, product_id, origin_event_id, user_search_filter_name) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(uuid::Uuid::from(user_id))
        .bind(uuid::Uuid::parse_str(&search_filter_id.to_string())?)
        .bind(uuid::Uuid::from(product_id))
        .bind(uuid::Uuid::from(origin_event_id))
        .bind("Percolator acceptance filter")
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;
    Ok(())
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

async fn assert_match_count(
    pool: &sqlx::PgPool,
    event_id: EventId,
    expected: i64,
) -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(expected, match_count(pool, event_id).await?);
    Ok(())
}

async fn match_count(pool: &sqlx::PgPool, event_id: EventId) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT count(*) FROM search_filter_matches WHERE origin_event_id = $1")
        .bind(uuid::Uuid::from(event_id))
        .fetch_one(pool)
        .await
}

async fn assert_no_matches_for(
    pool: &sqlx::PgPool,
    event_id: EventId,
    duration: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = Instant::now() + duration;
    loop {
        if match_count(pool, event_id).await? != 0 {
            return Err(std::io::Error::other(format!(
                "ignored product event {event_id} created a search-filter match"
            ))
            .into());
        }
        if Instant::now() >= deadline {
            return Ok(());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
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
