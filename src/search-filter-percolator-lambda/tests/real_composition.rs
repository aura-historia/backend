use application::transaction::{Transaction, UnitOfWork};
use aws_lambda_events::sqs::{SqsBatchResponse, SqsEvent, SqsMessage};
use domain_primitives::event_id::EventId;
use fxrate_postgres::SqlxFxRateSnapshotRepositoryFactory;
use lambda_runtime::{Context, LambdaEvent};
use large_language_model::{
    LargeLanguageModel, LargeLanguageModelError, StructuredGenerationRequest,
};
use listing_source_core::ListingSourceId;
use localization::Language;
use money::Currency;
use platform_postgres::SqlxUnitOfWork;
use product_listing_core::{
    product_listing_id::ProductListingId, product_listing_search::ProductListingSearch,
};
use product_listing_postgres::{
    SqlxProductListingCurrentEventGuardFactory,
    SqlxProductListingSearchFilterMatchSourceReaderFactory,
};
use search_filter_core::{
    NewSearchFilter, SearchFilter, search_filter_state::SearchFilterState,
    user_search_filter_name::UserSearchFilterName,
};
use search_filter_opensearch::OpenSearchSearchFilterIndex;
use search_filter_percolator_lambda::handler;
use search_filter_postgres::{
    SqlxActiveSearchFilterMatchCandidateReaderFactory, SqlxSearchFilterIndexReader,
    SqlxSearchFilterMatchWriterFactory, SqlxSearchFilterRepositoryFactory,
};
use search_filter_service::{
    ports::{SearchFilterRepository, SearchFilterRepositoryFactory},
    use_cases::{
        MatchProductListingEventHandler, MatchProductListingEventUseCase,
        ProjectSearchFilterChangeCommand, ProjectSearchFilterChangeHandler,
        ProjectSearchFilterChangeUseCase, SearchFilterProjectionOperation,
    },
};
use serde_json::json;
use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use test_api::{
    IntegrationTestService, OpenSearch as TestOpenSearch, Postgres, aura_integration_test,
    get_opensearch_client, get_postgres_client, refresh_index,
};
use user_core::user_id::UserId;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

// Plain saved filters must not call Vertex AI; only the provider boundary is stubbed.
struct UnexpectedEnhancedEvaluation;
#[async_trait::async_trait]
impl LargeLanguageModel for UnexpectedEnhancedEvaluation {
    async fn generate<Output>(
        &self,
        _request: StructuredGenerationRequest,
    ) -> Result<Output, LargeLanguageModelError>
    where
        Output: serde::de::DeserializeOwned + Send,
    {
        panic!("plain percolation unexpectedly called the enhanced evaluator")
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, TestOpenSearch()])]
async fn current_event_dedupes_after_response_loss_and_skips_stale_and_withdrawn_events() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let target = get_opensearch_client().await.clone();
        let user = seed_user(&pool).await?;
        let query = format!("Percolator walnut cabinet {user}");
        let active = new_filter(user, &query, SearchFilterState::Active)?;
        let inactive = new_filter(user, &query, SearchFilterState::InactiveByUser)?;
        for filter in [&active, &inactive] {
            insert_filter(&pool, filter).await?;
            let projection = ProjectSearchFilterChangeHandler::new(
                SqlxSearchFilterIndexReader::new(pool.clone()),
                OpenSearchSearchFilterIndex::new(target.clone()),
            );
            projection
                .execute(ProjectSearchFilterChangeCommand {
                    search_filter_id: filter.id(),
                    source_version: 1,
                    operation: SearchFilterProjectionOperation::Upsert,
                })
                .await?;
        }
        refresh_index("user_search_filters").await;
        let (product, first_event) = insert_product(&pool, &query).await?;
        let use_case = real_matcher(pool.clone(), target);
        let missing = EventId::new();
        let response = handler(
            batch([
                ("missing", job(missing, product)),
                ("current", job(first_event, product)),
            ]),
            use_case.as_ref(),
        )
        .await?;
        assert_eq!(vec!["missing"], failure_ids(response));
        let before = match_rows(&pool, first_event).await?;
        assert_eq!(1, before.len());
        assert_eq!(*active.id().as_uuid(), before[0].0);
        // Simulate a lost successful Lambda response: replay the same domain job with a new SQS ID.
        assert!(
            failure_ids(
                handler(
                    batch([("redelivery", job(first_event, product))]),
                    use_case.as_ref()
                )
                .await?
            )
            .is_empty()
        );
        assert_eq!(
            before,
            match_rows(&pool, first_event).await?,
            "duplicate must not rewrite the match"
        );
        assert!(match_rows(&pool, missing).await?.is_empty());

        let newer = advance(&pool, product, "ACTIVE", "AVAILABLE").await?;
        assert!(
            failure_ids(
                handler(
                    batch([("stale", job(first_event, product))]),
                    use_case.as_ref()
                )
                .await?
            )
            .is_empty()
        );
        assert_eq!(before, match_rows(&pool, first_event).await?);
        assert!(
            failure_ids(handler(batch([("newer", job(newer, product))]), use_case.as_ref()).await?)
                .is_empty()
        );
        // One match per filter/product, even when a later current event is delivered.
        assert!(match_rows(&pool, newer).await?.is_empty());
        assert_eq!(before, match_rows(&pool, first_event).await?);

        let withdrawn = advance(&pool, product, "WITHDRAWN", "AVAILABLE").await?;
        assert!(
            failure_ids(
                handler(
                    batch([("withdrawn", job(withdrawn, product))]),
                    use_case.as_ref()
                )
                .await?
            )
            .is_empty()
        );
        assert!(match_rows(&pool, withdrawn).await?.is_empty());
        assert!(
            failure_ids(handler(batch([("old", job(newer, product))]), use_case.as_ref()).await?)
                .is_empty()
        );
        assert_eq!(before, match_rows(&pool, first_event).await?);
        assert!(match_rows(&pool, newer).await?.is_empty());
        Ok(())
    }
    .await;
    assert!(
        result.is_ok(),
        "percolator real composition failed: {result:?}"
    );
}

fn real_matcher(
    pool: sqlx::PgPool,
    target: opensearch::OpenSearch,
) -> Arc<dyn MatchProductListingEventUseCase> {
    Arc::new(MatchProductListingEventHandler::new(
        SqlxUnitOfWork::new(pool),
        SqlxProductListingSearchFilterMatchSourceReaderFactory::new(),
        SqlxProductListingCurrentEventGuardFactory::new(),
        SqlxFxRateSnapshotRepositoryFactory,
        OpenSearchSearchFilterIndex::new(target),
        UnexpectedEnhancedEvaluation,
        SqlxActiveSearchFilterMatchCandidateReaderFactory,
        SqlxSearchFilterMatchWriterFactory,
    ))
}

fn job(event: EventId, product: ProductListingId) -> String {
    format!(
        r#"{{"schema_version":2,"scope":"search-filter-percolator","job_type":"PRODUCT_LISTING_EVENT","idempotency_key":"product-event:{event}","ordering_key":"product:{product}","payload":{{"event_id":"{event}","product_listing_id":"{product}"}}}}"#
    )
}

fn batch<const N: usize>(records: [(&str, String); N]) -> LambdaEvent<SqsEvent> {
    let mut sqs = SqsEvent::default();
    sqs.records = records
        .into_iter()
        .map(|(id, body)| {
            let mut message = SqsMessage::default();
            message.message_id = Some(id.to_owned());
            message.body = Some(body);
            message
        })
        .collect();
    let mut context = Context::default();
    context.deadline = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_millis() as u64
        + 60_000;
    LambdaEvent::new(sqs, context)
}

fn failure_ids(response: SqsBatchResponse) -> Vec<String> {
    response
        .batch_item_failures
        .into_iter()
        .map(|failure| failure.item_identifier)
        .collect()
}

async fn seed_user(pool: &sqlx::PgPool) -> TestResult<UserId> {
    let id = UserId::new();
    sqlx::query(
        "INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'ULTIMATE', 'USER')",
    )
    .bind(id.as_uuid())
    .bind(format!("percolator-lambda-{id}@example.test"))
    .execute(pool)
    .await?;
    Ok(id)
}

fn new_filter(user: UserId, query: &str, state: SearchFilterState) -> TestResult<SearchFilter> {
    Ok(SearchFilter::create(NewSearchFilter {
        user_search_filter_id: search_filter_core::user_search_filter_id::UserSearchFilterId::new(),
        user_id: user,
        name: UserSearchFilterName::from("Percolator composition fixture"),
        notifications: true,
        state,
        search: ProductListingSearch::new(Language::En, Currency::Eur)
            .with_product_listing_query(query.try_into()?),
        embedding: None,
    }))
}

async fn insert_filter(pool: &sqlx::PgPool, filter: &SearchFilter) -> TestResult {
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await?;
    SqlxSearchFilterRepositoryFactory
        .in_transaction(&mut tx)
        .insert(filter)
        .await?;
    tx.commit().await?;
    Ok(())
}

async fn insert_product(
    pool: &sqlx::PgPool,
    title: &str,
) -> TestResult<(ProductListingId, EventId)> {
    let product = ProductListingId::new();
    let event = EventId::new();
    let source = ListingSourceId::new();
    let operator = uuid::Uuid::now_v7();
    let slug = format!("percolator-source-{}", source.as_uuid());
    let suffix = product.as_uuid().simple().to_string();
    let mut tx = pool.begin().await?;
    sqlx::query("WITH operator AS (INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, concat($2, '-operator'), 'Percolator operator') RETURNING party_id) INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) SELECT $3, $2, 'Percolator source', party_id FROM operator")
        .bind(operator).bind(slug).bind(source.as_uuid()).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, title_text, title_language, description_text, description_language, availability, lifecycle, url, product_images) VALUES ($1, $2, $3, $3, $3, $4, $5, $6, 'en', 'Fixture description', 'en', 'AVAILABLE', 'ACTIVE', 'https://example.test/product', '[]')")
        .bind(product.as_uuid()).bind(format!("percolator-product-{}", &suffix[26..]))
        .bind(event.as_uuid()).bind(source.as_uuid()).bind(product.to_string()).bind(title)
        .execute(&mut *tx).await?;
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_DISCOVERED', 'DOMAIN', 1, $3, now())")
        .bind(event.as_uuid()).bind(product.as_uuid())
        .bind(json!({"listingSourceId": source.as_uuid().to_string(), "sourceListingId": product.to_string(), "title": {"language": "en", "text": title}, "description": null, "pricing": {"price": null, "priceEstimateMin": null, "priceEstimateMax": null}, "availability": "AVAILABLE", "url": "https://example.test/product", "imageCount": 0, "auction": null}))
        .execute(&mut *tx).await?;
    tx.commit().await?;
    Ok((product, event))
}

async fn advance(
    pool: &sqlx::PgPool,
    product: ProductListingId,
    lifecycle: &str,
    previous_availability: &str,
) -> TestResult<EventId> {
    let event = EventId::new();
    let mut tx = pool.begin().await?;
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_CHANGED', 'DOMAIN', 1, $3, now())")
        .bind(event.as_uuid()).bind(product.as_uuid())
        .bind(if lifecycle == "WITHDRAWN" { json!({"lifecycle": {"transition": "WITHDRAWN", "previousAvailability": previous_availability}}) } else { json!({"images": {"previousCount": 0, "currentCount": 0}}) })
        .execute(&mut *tx).await?;
    sqlx::query("UPDATE product_listings SET current_event_id = $1, lifecycle = $2, availability = CASE WHEN $2 = 'WITHDRAWN' THEN NULL ELSE 'AVAILABLE' END, version = version + 1, projection_version = projection_version + 1, updated = now() WHERE product_listing_id = $3")
        .bind(event.as_uuid()).bind(lifecycle).bind(product.as_uuid()).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(event)
}

async fn match_rows(pool: &sqlx::PgPool, event: EventId) -> TestResult<Vec<(uuid::Uuid, String)>> {
    Ok(sqlx::query_as("SELECT user_search_filter_id, xmin::text FROM search_filter_matches WHERE origin_event_id = $1 ORDER BY user_search_filter_id")
        .bind(event.as_uuid()).fetch_all(pool).await?)
}
