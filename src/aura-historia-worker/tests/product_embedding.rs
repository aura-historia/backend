mod support;

use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use aura_historia_jobs::{
    DomainJob, DomainJobPayload, IdempotencyKey, OrderingKey, ProductListingEventJob, WorkerQueue,
    encode,
};
use aura_historia_worker::embedding_batches::compose_round_use_case;
use aura_historia_worker::scopes::product_embedding::{
    ProductEmbeddingJobs, compose_product_embedding_use_case,
};
use domain_primitives::event_id::EventId;
use embedding::{EMBEDDING_DIMENSIONS, EmbeddingText};
use listing_source_core::ListingSourceId;
use platform_postgres_queue::{Disposition, JobId};
use platform_worker::{Job, JobHandler, error_class};
use product_listing_core::{
    product_listing_id::ProductListingId, product_listing_slug_id::ProductListingSlugId,
};
use product_listing_service::use_cases::{
    EmbedProductListingBatchesUseCase, ProductEmbeddingRoundReport,
};
use serde_json::json;
use std::sync::atomic::Ordering;
use support::{FakeEmbeddingBatches, append_change};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_schedule_the_revision_and_commit_its_batch_result_once() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let handler = ProductEmbeddingJobs::new(compose_product_embedding_use_case(pool.clone()));
        let batches = FakeEmbeddingBatches::default();
        let rounds = compose_round_use_case(pool.clone(), batches.clone());
        let missing = (ProductListingId::new(), EventId::new());
        assert_eq!(
            Disposition::Retry(error_class("missing_source")),
            handler.handle(&job(&job_body(missing.0, missing.1)?)).await
        );

        let (product, source) = seed_discovery(&pool).await?;
        let body = job_body(product, source)?;
        assert_eq!(Disposition::Complete, handler.handle(&job(&body)).await);
        assert!(embedding(&pool, product).await?.0.is_none(), "only scheduled");

        let submitted = round(rounds.as_ref()).await?;
        assert_eq!(1, submitted.submitted_embeddings);
        {
            let submitted = batches.submitted.lock().unwrap();
            let request = &submitted[0][0];
            assert_eq!("Antiker Eichenstuhl", request.title.as_str());
            assert_eq!(
                Some("Bemalter Stuhl"),
                request.additional_text.as_ref().map(EmbeddingText::as_str)
            );
            assert_eq!(
                Some("https://example.test/image.jpg"),
                request.image_url.as_ref().map(|url| url.as_url().as_str())
            );
        }

        let collected = round(rounds.as_ref()).await?;
        assert_eq!((1, 1), (collected.batches_collected, collected.settlements.applied));
        let committed = embedding(&pool, product).await?;
        let vector = committed.0.as_ref().ok_or("missing committed embedding")?;
        assert_eq!(EMBEDDING_DIMENSIONS, vector.len());
        assert!((vector[0] - 1.0 / (EMBEDDING_DIMENSIONS as f32).sqrt()).abs() < 0.000_001);
        assert_eq!(*source.as_uuid(), committed.2);
        assert_ne!(*source.as_uuid(), committed.1);
        assert_eq!(1, completion_count(&pool, product).await?);
        let payload: serde_json::Value = sqlx::query_scalar("SELECT payload FROM product_listing_events WHERE product_listing_id = $1 AND event_type = 'ENRICHMENT_EMBEDDED'")
            .bind(product.as_uuid()).fetch_one(&pool).await?;
        assert_eq!(json!({"sourceEventId": source.as_uuid().to_string()}), payload);

        assert_eq!(Disposition::Complete, handler.handle(&job(&body)).await, "a duplicate job");
        assert_eq!(0, round(rounds.as_ref()).await?.submitted_embeddings);
        assert_eq!(committed, embedding(&pool, product).await?);
        assert_eq!(1, completion_count(&pool, product).await?);
        Ok(())
    }
    .await;
    assert!(result.is_ok(), "{result:?}");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_drop_a_batch_result_for_a_revision_superseded_while_its_batch_ran() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let handler = ProductEmbeddingJobs::new(compose_product_embedding_use_case(pool.clone()));
        let rounds = compose_round_use_case(pool.clone(), FakeEmbeddingBatches::default());
        let (product, source) = seed_discovery(&pool).await?;
        assert_eq!(Disposition::Complete, handler.handle(&job(&job_body(product, source)?)).await);
        round(rounds.as_ref()).await?;

        let newer = EventId::new();
        let mut tx = pool.begin().await?;
        sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_CHANGED', 'DOMAIN', 1, $3, now())")
            .bind(newer.as_uuid()).bind(product.as_uuid())
            .bind(json!({"images": {"previousCount": 1, "currentCount": 1}}))
            .execute(&mut *tx).await?;
        sqlx::query("UPDATE product_listings SET embedding_source_event_id = $1, current_event_id = $1, embedding = NULL, version = version + 1 WHERE product_listing_id = $2")
            .bind(newer.as_uuid()).bind(product.as_uuid()).execute(&mut *tx).await?;
        tx.commit().await?;

        let collected = round(rounds.as_ref()).await?;
        assert_eq!(1, collected.batches_collected);
        assert_eq!(0, collected.settlements.applied);
        assert_eq!(0, collected.submitted_embeddings, "the superseded work is discarded");
        assert_eq!(0, completion_count(&pool, product).await?);
        let stored = embedding(&pool, product).await?;
        assert!(stored.0.is_none());
        assert_eq!(*newer.as_uuid(), stored.2);
        Ok(())
    }
    .await;
    assert!(result.is_ok(), "{result:?}");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_requeue_unprocessed_work_and_submit_it_again() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let handler = ProductEmbeddingJobs::new(compose_product_embedding_use_case(pool.clone()));
        let batches = FakeEmbeddingBatches::default();
        let rounds = compose_round_use_case(pool.clone(), batches.clone());
        let (product, source) = seed_discovery(&pool).await?;
        handler.handle(&job(&job_body(product, source)?)).await;
        round(rounds.as_ref()).await?;
        batches.leave_unprocessed.store(true, Ordering::SeqCst);

        let expired = round(rounds.as_ref()).await?;

        assert_eq!(
            (1, 1),
            (expired.settlements.requeued, expired.submitted_embeddings),
            "requeued and submitted again in the same round"
        );
        let attempts: i16 = sqlx::query_scalar(
            "SELECT attempts FROM product_listing_embedding_work WHERE product_listing_id = $1",
        )
        .bind(product.as_uuid())
        .fetch_one(&pool)
        .await?;
        assert_eq!(0, attempts);
        assert_eq!(1, round(rounds.as_ref()).await?.settlements.applied);
        Ok(())
    }
    .await;
    assert!(result.is_ok(), "{result:?}");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_wait_while_not_for_sale_and_embed_the_current_revision_once_back_on_sale() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let handler = ProductEmbeddingJobs::new(compose_product_embedding_use_case(pool.clone()));
        let rounds = compose_round_use_case(pool.clone(), FakeEmbeddingBatches::default());
        let (product, discovery) = seed_discovery(&pool).await?;
        sqlx::query("UPDATE product_listings SET availability = 'OUT_OF_STOCK' WHERE product_listing_id = $1")
            .bind(product.as_uuid()).execute(&pool).await?;
        let discovery_job = job_body(product, discovery)?;
        assert_eq!(Disposition::Complete, handler.handle(&job(&discovery_job)).await);
        let reimaged = append_change(
            &pool,
            product,
            json!({"images": {"previousCount": 1, "currentCount": 1}}),
            "UPDATE product_listings SET embedding_source_event_id = $1, current_event_id = $1, version = version + 1 WHERE product_listing_id = $2",
        )
        .await?;
        assert_eq!(Disposition::Complete, handler.handle(&job(&job_body(product, reimaged)?)).await);
        assert_eq!(None, scheduled_revision(&pool, product).await?, "nothing is scheduled while out of stock");

        let restocked = append_change(
            &pool,
            product,
            json!({"availability": {"previous": "OUT_OF_STOCK", "current": null}}),
            "UPDATE product_listings SET availability = NULL, current_event_id = $1, version = version + 1 WHERE product_listing_id = $2",
        )
        .await?;
        let restocked_job = job_body(product, restocked)?;
        assert_eq!(Disposition::Complete, handler.handle(&job(&restocked_job)).await);
        assert_eq!(
            Some(*reimaged.as_uuid()),
            scheduled_revision(&pool, product).await?,
            "the catch-up targets the current embedding revision, not its own event"
        );
        assert_eq!(
            Disposition::Complete,
            handler.handle(&job(&discovery_job)).await,
            "the late discovery job is stale"
        );
        assert_eq!(Some(*reimaged.as_uuid()), scheduled_revision(&pool, product).await?);

        assert_eq!(1, round(rounds.as_ref()).await?.submitted_embeddings);
        assert_eq!(1, round(rounds.as_ref()).await?.settlements.applied);
        assert_eq!(*reimaged.as_uuid(), embedding(&pool, product).await?.2);
        assert!(embedding(&pool, product).await?.0.is_some());
        assert_eq!(Disposition::Complete, handler.handle(&job(&restocked_job)).await);
        assert_eq!(None, scheduled_revision(&pool, product).await?, "an embedded revision is a duplicate");
        assert_eq!(1, completion_count(&pool, product).await?);
        Ok(())
    }
    .await;
    assert!(result.is_ok(), "{result:?}");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_wait_while_withdrawn_and_embed_the_current_revision_once_restored() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let handler = ProductEmbeddingJobs::new(compose_product_embedding_use_case(pool.clone()));
        let (product, discovery) = seed_discovery(&pool).await?;
        sqlx::query("UPDATE product_listings SET lifecycle = 'WITHDRAWN', availability = NULL WHERE product_listing_id = $1")
            .bind(product.as_uuid()).execute(&pool).await?;
        assert_eq!(Disposition::Complete, handler.handle(&job(&job_body(product, discovery)?)).await);
        assert_eq!(None, scheduled_revision(&pool, product).await?, "nothing is scheduled while withdrawn");

        let restored = append_change(
            &pool,
            product,
            json!({"lifecycle": {"transition": "RESTORED"},
                "availability": {"previous": null, "current": "AVAILABLE"}}),
            "UPDATE product_listings SET lifecycle = 'ACTIVE', availability = 'AVAILABLE', current_event_id = $1, version = version + 1 WHERE product_listing_id = $2",
        )
        .await?;
        assert_eq!(Disposition::Complete, handler.handle(&job(&job_body(product, restored)?)).await);
        assert_eq!(Some(*discovery.as_uuid()), scheduled_revision(&pool, product).await?);
        Ok(())
    }
    .await;
    assert!(result.is_ok(), "{result:?}");
}

async fn scheduled_revision(
    pool: &sqlx::PgPool,
    product: ProductListingId,
) -> Result<Option<uuid::Uuid>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT source_event_id FROM product_listing_embedding_work WHERE product_listing_id = $1",
    )
    .bind(product.as_uuid())
    .fetch_optional(pool)
    .await
}

async fn round(
    rounds: &dyn EmbedProductListingBatchesUseCase,
) -> Result<ProductEmbeddingRoundReport, Box<dyn std::error::Error + Send + Sync>> {
    let context = OperationContext {
        principal: Principal::System,
        request_id: RequestId::new("product-embedding-round:test"),
        correlation_id: CorrelationId::new("product-embedding-round:test"),
    };
    Ok(rounds.execute(&context).await?)
}

async fn embedding(
    pool: &sqlx::PgPool,
    product: ProductListingId,
) -> Result<(Option<Vec<f32>>, uuid::Uuid, uuid::Uuid), sqlx::Error> {
    sqlx::query_as("SELECT embedding, current_event_id, embedding_source_event_id FROM product_listings WHERE product_listing_id = $1")
        .bind(product.as_uuid()).fetch_one(pool).await
}

async fn completion_count(
    pool: &sqlx::PgPool,
    product: ProductListingId,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT count(*) FROM product_listing_events WHERE product_listing_id = $1 AND event_type = 'ENRICHMENT_EMBEDDED'")
        .bind(product.as_uuid()).fetch_one(pool).await
}

async fn seed_discovery(
    pool: &sqlx::PgPool,
) -> Result<(ProductListingId, EventId), Box<dyn std::error::Error + Send + Sync>> {
    let product = ProductListingId::new();
    let source = EventId::new();
    let listing_source = ListingSourceId::new();
    let slug = ProductListingSlugId::from_title_and_suffix(
        "embedding lambda product",
        &product.as_uuid().simple().to_string()[26..],
    )
    .map_err(|_| "invalid fixture slug")?;
    let mut tx = pool.begin().await?;
    sqlx::query("WITH operator AS (INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, concat($2, '-operator'), 'Fixture operator') RETURNING party_id) INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id, listing_source_type) SELECT $3, $2, 'Embedding lambda source', party_id, 'DEALER' FROM operator")
        .bind(uuid::Uuid::now_v7()).bind(format!("embedding-lambda-source-{}", listing_source.as_uuid()))
        .bind(listing_source.as_uuid()).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, title_text, title_language, description_text, description_language, availability, lifecycle, url, product_images) VALUES ($1, $2, $3, $3, $3, $4, $5, 'Antiker Eichenstuhl', 'de', 'Bemalter Stuhl', 'de', 'AVAILABLE', 'ACTIVE', 'https://example.test/product', '[{\"url\": \"https://example.test/image.jpg\"}]')")
        .bind(product.as_uuid()).bind(slug.as_ref()).bind(source.as_uuid())
        .bind(listing_source.as_uuid()).bind(product.to_string()).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_DISCOVERED', 'DOMAIN', 1, $3, now())")
        .bind(source.as_uuid()).bind(product.as_uuid()).bind(json!({
            "listingSourceId": listing_source.as_uuid().to_string(), "sourceListingId": product.to_string(),
            "title": {"language": "de", "text": "Antiker Eichenstuhl"},
            "description": {"language": "de", "text": "Bemalter Stuhl"},
            "pricing": {"price": null, "priceEstimateMin": null, "priceEstimateMax": null},
            "availability": "AVAILABLE", "url": "https://example.test/product", "imageCount": 1, "auction": null
        })).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok((product, source))
}

fn job_body(
    product: ProductListingId,
    source: EventId,
) -> Result<String, aura_historia_jobs::WireError> {
    encode(&DomainJob {
        target_queue: WorkerQueue::ProductListingEmbed,
        idempotency_key: IdempotencyKey::new(format!("product-event:{source}")),
        ordering_key: OrderingKey::new(format!("product:{product}")),
        payload: DomainJobPayload::ProductListingEvent(ProductListingEventJob {
            event_id: source,
            product_listing_id: product,
        }),
    })
}

fn job(body: &str) -> Job {
    Job::new(
        JobId::from_uuid(uuid::Uuid::now_v7()),
        1,
        None,
        body.to_owned(),
    )
}
