use application::transaction::{Transaction, UnitOfWork};
use aws_lambda_events::sqs::{SqsBatchResponse, SqsEvent, SqsMessage};
use lambda_runtime::{Context, LambdaEvent};
use localization::Language;
use money::Currency;
use opensearch::GetParts;
use platform_postgres::SqlxUnitOfWork;
use product_listing_core::product_listing_search::ProductListingSearch;
use search_filter_core::{
    NewSearchFilter, SearchFilter, search_filter_state::SearchFilterState,
    user_search_filter_id::UserSearchFilterId, user_search_filter_name::UserSearchFilterName,
};
use search_filter_opensearch::OpenSearchSearchFilterIndex;
use search_filter_postgres::SqlxSearchFilterRepositoryFactory;
use search_filter_projection_lambda::{compose_projection_use_case, handler};
use search_filter_service::ports::{
    SearchFilterIndex, SearchFilterIndexQuery, SearchFilterRepository,
    SearchFilterRepositoryFactory,
};
use serde_json::{Value, json};
use std::time::{SystemTime, UNIX_EPOCH};
use test_api::{
    IntegrationTestService, OpenSearch as TestOpenSearch, Postgres, aura_integration_test,
    get_opensearch_client, get_postgres_client, refresh_index,
};
use user_core::user_id::UserId;

#[path = "support/projection_write_relay.rs"]
mod projection_write_relay;
use projection_write_relay::ProjectionWriteRelay;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
const INDEX: &str = "user_search_filters";
type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[aura_integration_test(services = [BUSINESS_SCHEMA, TestOpenSearch()])]
async fn update_replay_and_delete_fence_survive_delayed_upsert() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let target = get_opensearch_client().await.clone();
        let user = seed_user(&pool).await?;
        let mut filter = new_filter(user, "Original walnut cabinet")?;
        let id = filter.id();
        let version = insert(&pool, &filter).await?;
        assert_eq!(1, version);
        let use_case = compose_projection_use_case(pool.clone(), target.clone());
        assert!(
            failure_ids(handler(event("insert", user, id, 1, "INSERT"), use_case.as_ref()).await?)
                .is_empty()
        );
        let first = stored(id).await?;
        assert_eq!(json!(1), first["_version"]);
        assert_eq!(json!(1), first["_source"]["sourceVersion"]);

        filter.replace_search(
            ProductListingSearch::new(Language::En, Currency::Eur)
                .with_product_listing_query("Replacement pine table".try_into()?),
            None,
        );
        assert_eq!(2, update(&pool, &filter, version).await?);
        assert!(
            failure_ids(handler(event("update", user, id, 2, "UPDATE"), use_case.as_ref()).await?)
                .is_empty()
        );
        let updated = stored(id).await?;
        assert_eq!(json!(2), updated["_version"]);
        assert_eq!(json!(2), updated["_source"]["sourceVersion"]);
        assert_ne!(first["_source"]["search"], updated["_source"]["search"]);
        for (version, operation) in [(1, "INSERT"), (2, "UPDATE"), (1, "INSERT")] {
            assert!(
                failure_ids(
                    handler(
                        event("replay", user, id, version, operation),
                        use_case.as_ref()
                    )
                    .await?
                )
                .is_empty()
            );
            assert_eq!(updated, stored(id).await?);
        }

        // An upsert has already read committed version 2, but reaches the target after deletion.
        let mut delayed_target = ProjectionWriteRelay::pause(target.clone()).await?;
        let delayed_case = compose_projection_use_case(pool.clone(), delayed_target.client.clone());
        let delayed = tokio::spawn(async move {
            handler(
                event("delayed", user, id, 2, "UPDATE"),
                delayed_case.as_ref(),
            )
            .await
        });
        delayed_target.wait_until_received().await?;
        delete(&pool, id).await?;
        assert!(
            failure_ids(handler(event("delete", user, id, 2, "DELETE"), use_case.as_ref()).await?)
                .is_empty()
        );
        let tombstone = stored(id).await?;
        assert_eq!(json!(3), tombstone["_version"]);
        assert_eq!(
            json!({"userSearchFilterId": id, "sourceVersion": 3, "projectionDeleted": true}),
            tombstone["_source"]
        );
        assert_eq!(409, delayed_target.resume().await?);
        assert!(failure_ids(delayed.await??).is_empty());
        assert_eq!(tombstone, stored(id).await?);
        let visible = OpenSearchSearchFilterIndex::new(target.clone())
            .query(&SearchFilterIndexQuery::default())
            .await?;
        assert!(!visible.items.iter().any(|item| item.search_filter_id == id));
        for (version, operation) in [(1, "INSERT"), (2, "UPDATE"), (2, "DELETE")] {
            assert!(
                failure_ids(
                    handler(
                        event("after-delete", user, id, version, operation),
                        use_case.as_ref()
                    )
                    .await?
                )
                .is_empty()
            );
            assert_eq!(tombstone, stored(id).await?);
        }
        Ok(())
    }
    .await;
    assert!(
        result.is_ok(),
        "projection fence composition failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, TestOpenSearch()])]
async fn missing_source_and_lost_target_response_are_retried_safely() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let target = get_opensearch_client().await.clone();
        let user = seed_user(&pool).await?;
        let id = UserSearchFilterId::new();
        let use_case = compose_projection_use_case(pool.clone(), target.clone());
        // A missing committed row is a deletion invalidation, not a fabricated live projection.
        assert!(
            failure_ids(handler(event("missing", user, id, 1, "INSERT"), use_case.as_ref()).await?)
                .is_empty()
        );
        let tombstone = stored(id).await?;
        assert_eq!(json!(2), tombstone["_version"]);
        assert_eq!(json!(true), tombstone["_source"]["projectionDeleted"]);

        let filter = new_filter(user, "Lost response cabinet")?;
        let id = filter.id();
        insert(&pool, &filter).await?;
        let relay = ProjectionWriteRelay::drop_response(target.clone()).await?;
        let uncertain_case = compose_projection_use_case(pool.clone(), relay.client.clone());
        assert_eq!(
            vec!["lost"],
            failure_ids(
                handler(
                    event("lost", user, id, 1, "INSERT"),
                    uncertain_case.as_ref()
                )
                .await?
            )
        );
        assert!((200..300).contains(&relay.finish().await?));
        let accepted = stored(id).await?;
        assert_eq!(json!(1), accepted["_version"]);
        assert!(
            failure_ids(
                handler(
                    event("redelivery", user, id, 1, "INSERT"),
                    use_case.as_ref()
                )
                .await?
            )
            .is_empty()
        );
        assert_eq!(accepted, stored(id).await?);
        Ok(())
    }
    .await;
    assert!(
        result.is_ok(),
        "projection missing/response-loss composition failed: {result:?}"
    );
}

fn event(
    message_id: &str,
    user: UserId,
    id: UserSearchFilterId,
    version: i64,
    operation: &str,
) -> LambdaEvent<SqsEvent> {
    let body = format!(
        r#"{{"schema_version":2,"scope":"search-filter-projection","job_type":"SEARCH_FILTER_CHANGED","idempotency_key":"search-filter:{id}:{version}:{}","ordering_key":"search-filter:{id}","payload":{{"user_id":"{user}","user_search_filter_id":"{id}","version":{version},"operation":"{operation}"}}}}"#,
        operation.to_lowercase()
    );
    let mut message = SqsMessage::default();
    message.message_id = Some(message_id.to_owned());
    message.body = Some(body);
    let mut sqs = SqsEvent::default();
    sqs.records = vec![message];
    let mut context = Context::default();
    context.deadline = epoch_millis().saturating_add(60_000);
    LambdaEvent::new(sqs, context)
}

fn failure_ids(response: SqsBatchResponse) -> Vec<String> {
    response
        .batch_item_failures
        .into_iter()
        .map(|failure| failure.item_identifier)
        .collect()
}

fn epoch_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_millis() as u64
}

async fn stored(id: UserSearchFilterId) -> TestResult<Value> {
    refresh_index(INDEX).await;
    Ok(get_opensearch_client()
        .await
        .get(GetParts::IndexId(INDEX, &id.to_string()))
        .send()
        .await?
        .error_for_status_code()?
        .json()
        .await?)
}

async fn seed_user(pool: &sqlx::PgPool) -> TestResult<UserId> {
    let id = UserId::new();
    sqlx::query(
        "INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'ULTIMATE', 'USER')",
    )
    .bind(id.as_uuid())
    .bind(format!("projection-lambda-{id}@example.test"))
    .execute(pool)
    .await?;
    Ok(id)
}

fn new_filter(user: UserId, query: &str) -> TestResult<SearchFilter> {
    Ok(SearchFilter::create(NewSearchFilter {
        user_search_filter_id: UserSearchFilterId::new(),
        user_id: user,
        name: UserSearchFilterName::from("Lambda projection fixture"),
        notifications: true,
        state: SearchFilterState::Active,
        search: ProductListingSearch::new(Language::En, Currency::Eur)
            .with_product_listing_query(query.try_into()?),
        embedding: None,
    }))
}

async fn insert(pool: &sqlx::PgPool, filter: &SearchFilter) -> TestResult<i64> {
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await?;
    let version = SqlxSearchFilterRepositoryFactory
        .in_transaction(&mut tx)
        .insert(filter)
        .await?
        .version;
    tx.commit().await?;
    Ok(version)
}

async fn update(pool: &sqlx::PgPool, filter: &SearchFilter, expected: i64) -> TestResult<i64> {
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await?;
    let version = SqlxSearchFilterRepositoryFactory
        .in_transaction(&mut tx)
        .update(filter, expected)
        .await?
        .version;
    tx.commit().await?;
    Ok(version)
}

async fn delete(pool: &sqlx::PgPool, id: UserSearchFilterId) -> TestResult {
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await?;
    SqlxSearchFilterRepositoryFactory
        .in_transaction(&mut tx)
        .delete(id)
        .await?;
    tx.commit().await?;
    Ok(())
}
