use aura_historia_jobs::{
    DomainJob, DomainJobPayload, IdempotencyKey, OrderingKey, ProductListingEventJob, WorkerQueue,
    encode,
};
use aws_lambda_events::sqs::{SqsBatchResponse, SqsEvent, SqsMessage};
use domain_primitives::event_id::EventId;
use lambda_runtime::{Context, LambdaEvent};
use listing_source_core::ListingSourceId;
use product_content_assessment_lambda::{compose_product_content_assessment_use_case, handler};
use product_listing_core::{
    product_listing_id::ProductListingId, product_listing_slug_id::ProductListingSlugId,
};
use serde_json::{Value, json};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn cancelled_uncommitted_assessment_is_retried_and_committed_duplicate_is_a_noop() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let (product, source) = seed_discovery(&pool).await?;
        let body = job_body(product, source)?;
        let use_case = compose_product_content_assessment_use_case(pool.clone());

        let mut barrier = pool.begin().await?;
        sqlx::query("LOCK TABLE product_listing_content_assessments IN ACCESS EXCLUSIVE MODE")
            .execute(&mut *barrier)
            .await?;
        let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *barrier)
            .await?;
        let pending = tokio::spawn({
            let use_case = use_case.clone();
            let body = body.clone();
            async move { handler(event("first-attempt", body), use_case.as_ref()).await }
        });
        wait_for_blocked_write(&pool, blocker_pid, 1).await?;
        assert!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM product_listing_content_assessments WHERE product_listing_id = $1")
            .bind(product.as_uuid()).fetch_one(&mut *barrier).await? == 0);
        pending.abort();
        assert!(
            pending.await.is_err(),
            "attempt must be cancelled before commit"
        );
        barrier.commit().await?;
        assert!(snapshot(&pool, product).await?.is_none());

        assert!(
            failure_ids(handler(event("redelivery", body.clone()), use_case.as_ref()).await?)
                .is_empty()
        );
        let committed = snapshot(&pool, product)
            .await?
            .ok_or("missing committed assessment")?;
        assert_eq!(json!(source.as_uuid()), committed["source_event_id"]);
        assert_eq!("ALLOWED", committed["decision"]);
        assert!(
            failure_ids(handler(event("duplicate", body), use_case.as_ref()).await?).is_empty()
        );
        assert_eq!(Some(committed), snapshot(&pool, product).await?);
        Ok(())
    }
    .await;
    assert!(
        result.is_ok(),
        "cancel/redelivery composition failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn overlapping_lambda_invocations_write_only_one_authoritative_assessment() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let (product, source) = seed_discovery(&pool).await?;
        let body = job_body(product, source)?;
        let use_case = compose_product_content_assessment_use_case(pool.clone());
        let mut barrier = pool.begin().await?;
        sqlx::query("LOCK TABLE product_listing_content_assessments IN ACCESS EXCLUSIVE MODE")
            .execute(&mut *barrier).await?;
        let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *barrier).await?;
        let first = tokio::spawn({
            let use_case = use_case.clone();
            let body = body.clone();
            async move { handler(event("first", body), use_case.as_ref()).await }
        });
        wait_for_blocked_write(&pool, blocker_pid, 1).await?;
        let first_xid: String = sqlx::query_scalar("SELECT backend_xid::text FROM pg_stat_activity WHERE $1 = ANY(pg_blocking_pids(pid)) AND query LIKE '%product_listing_content_assessments%' LIMIT 1")
            .bind(blocker_pid).fetch_one(&pool).await?;
        let second = tokio::spawn({
            let use_case = use_case.clone();
            async move { handler(event("second", body), use_case.as_ref()).await }
        });
        wait_for_blocked_write(&pool, blocker_pid, 2).await?;
        let count_before: i64 = sqlx::query_scalar("SELECT count(*) FROM product_listing_content_assessments WHERE product_listing_id = $1")
            .bind(product.as_uuid()).fetch_one(&mut *barrier).await?;
        assert_eq!(0, count_before);
        barrier.commit().await?;
        assert!(failure_ids(first.await??).is_empty());
        assert!(failure_ids(second.await??).is_empty());
        let committed = snapshot(&pool, product).await?.ok_or("missing assessment")?;
        assert_eq!(json!(source.as_uuid()), committed["source_event_id"]);
        assert_eq!("ALLOWED", committed["decision"]);
        assert_eq!(first_xid, committed["tuple_version"], "overlap must not rewrite the first writer's row");
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM product_listing_content_assessments WHERE product_listing_id = $1")
            .bind(product.as_uuid()).fetch_one(&pool).await?;
        assert_eq!(1, count);
        let source_count: i64 = sqlx::query_scalar("SELECT count(*) FROM product_listing_events WHERE product_listing_id = $1")
            .bind(product.as_uuid()).fetch_one(&pool).await?;
        assert_eq!(1, source_count);
        Ok(())
    }.await;
    assert!(
        result.is_ok(),
        "concurrent assessment composition failed: {result:?}"
    );
}

async fn wait_for_blocked_write(
    pool: &sqlx::PgPool,
    blocker_pid: i32,
    expected: i64,
) -> TestResult {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            // The second invocation waits on the first product row lock; both are downstream
            // of the test's assessment-table lock, so count the entire blocking chain.
            let blocked: i64 = sqlx::query_scalar("WITH RECURSIVE blocked(pid) AS (SELECT pid FROM pg_stat_activity WHERE $1 = ANY(pg_blocking_pids(pid)) UNION SELECT a.pid FROM pg_stat_activity a JOIN blocked b ON b.pid = ANY(pg_blocking_pids(a.pid))) SELECT count(*) FROM blocked")
                .bind(blocker_pid).fetch_one(pool).await?;
            if blocked >= expected { return Ok::<(), sqlx::Error>(()); }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }).await??;
    Ok(())
}

async fn snapshot(
    pool: &sqlx::PgPool,
    product: ProductListingId,
) -> Result<Option<Value>, sqlx::Error> {
    sqlx::query_scalar("SELECT to_jsonb(assessment) || jsonb_build_object('tuple_version', xmin::text) FROM product_listing_content_assessments assessment WHERE product_listing_id = $1")
        .bind(product.as_uuid()).fetch_optional(pool).await
}

fn failure_ids(response: SqsBatchResponse) -> Vec<String> {
    response
        .batch_item_failures
        .into_iter()
        .map(|failure| failure.item_identifier)
        .collect()
}

fn event(id: &str, body: String) -> LambdaEvent<SqsEvent> {
    let mut message = SqsMessage::default();
    message.message_id = Some(id.to_owned());
    message.body = Some(body);
    let mut payload = SqsEvent::default();
    payload.records = vec![message];
    let mut context = Context::default();
    context.deadline = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        + 60_000;
    LambdaEvent::new(payload, context)
}

fn job_body(
    product: ProductListingId,
    source: EventId,
) -> Result<String, aura_historia_jobs::WireError> {
    encode(&DomainJob {
        target_queue: WorkerQueue::ProductListingContentAssessment,
        idempotency_key: IdempotencyKey::new(format!("product-event:{source}")),
        ordering_key: OrderingKey::new(format!("product:{product}")),
        payload: DomainJobPayload::<aura_historia_jobs::SearchFilterOperation>::ProductListingEvent(
            ProductListingEventJob {
                event_id: source,
                product_listing_id: product,
            },
        ),
    })
}

async fn seed_discovery(
    pool: &sqlx::PgPool,
) -> Result<(ProductListingId, EventId), Box<dyn std::error::Error + Send + Sync>> {
    let product = ProductListingId::new();
    let source = EventId::new();
    let listing_source = ListingSourceId::new();
    let product_uuid = product.as_uuid();
    let slug = ProductListingSlugId::from_title_and_suffix(
        "content assessment lambda product",
        &product_uuid.simple().to_string()[26..],
    )
    .map_err(|_| "invalid fixture slug")?;
    let operator = uuid::Uuid::now_v7();
    let mut tx = pool.begin().await?;
    sqlx::query("WITH operator AS (INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, concat($2, '-operator'), 'Fixture operator') RETURNING party_id) INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) SELECT $3, $2, 'Content assessment lambda source', party_id FROM operator")
        .bind(operator).bind(format!("content-assessment-lambda-source-{}", listing_source.as_uuid()))
        .bind(listing_source.as_uuid()).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, title_text, title_language, description_text, description_language, availability, lifecycle, url, product_images) VALUES ($1, $2, $3, $3, $3, $4, $5, 'Antiker Eichenstuhl', 'de', 'Bemalter Stuhl', 'de', 'AVAILABLE', 'ACTIVE', 'https://example.test/product', '[]')")
        .bind(product_uuid).bind(slug.as_ref()).bind(source.as_uuid())
        .bind(listing_source.as_uuid()).bind(product.to_string()).execute(&mut *tx).await?;
    let payload = json!({
        "listingSourceId": listing_source.as_uuid().to_string(),
        "sourceListingId": product.to_string(),
        "title": {"language": "de", "text": "Antiker Eichenstuhl"},
        "description": {"language": "de", "text": "Bemalter Stuhl"},
        "pricing": {"price": null, "priceEstimateMin": null, "priceEstimateMax": null},
        "availability": "AVAILABLE", "url": "https://example.test/product", "imageCount": 0,
        "auction": null
    });
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_DISCOVERED', 'DOMAIN', 1, $3, now())")
        .bind(source.as_uuid()).bind(product_uuid).bind(payload).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok((product, source))
}
