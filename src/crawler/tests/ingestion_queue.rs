//! The crawler feeds the business stack end to end: its raw capture service admits observations
//! through the ingestion role, which may only call the enqueue function, and the ingestion
//! consumer captures them from the job queue.

use crawler::service::raw_capture::{
    ProductListingRawCaptureItem, ProductListingRawCaptureOutcome, ProductListingRawCaptureService,
    ProductListingRawCaptureServiceImpl,
};
use listing_source_core::ListingSourceId;
use platform_postgres_queue::{JobQueue, Settlement};
use platform_worker_queue_contract::job_queue::product_listing_ingestion_job_scope;
use product_listing_ingestion_lambda::{compose_ingestion_use_case, process_queued_ingestion_job};
use product_listing_ingestion_queue::QueueProductListingIngestionPublisher;
use product_listing_normalization::{
    NormalizationContext, ProductListingNormalizationInput, RawProductListingOperation,
    RawProductListingPayloadFormat, RawProductListingProvenance, RawProductListingValues,
    SourcePayload,
};
use product_listing_service::use_cases::SubmitInternalProductListingIngestionHandler;
use serde_json::json;
use sqlx::{
    PgPool,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use std::sync::Arc;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

/// Sessions acting as the crawler's ingestion role, which the fixture creates without login.
async fn crawler_role_pool(pool: &PgPool) -> PgPool {
    let options: PgConnectOptions = (*pool.connect_options()).clone();
    PgPoolOptions::new()
        .max_connections(2)
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("SET ROLE aura_crawler_ingestion")
                    .execute(connection)
                    .await
                    .map(|_| ())
            })
        })
        .connect_with(options)
        .await
        .unwrap()
}

async fn seed_source(pool: &PgPool) -> ListingSourceId {
    let party = uuid::Uuid::now_v7();
    let source = ListingSourceId::new();
    sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
        .bind(party)
        .bind(format!("crawler-{party}"))
        .bind("Crawler ingestion test")
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, \
         operator_party_id, listing_source_type) VALUES ($1, $2, $3, $4, 'DEALER')",
    )
    .bind(source.into_uuid())
    .bind(format!("crawler-{}", source.into_uuid().simple()))
    .bind("Crawler ingestion source")
    .bind(party)
    .execute(pool)
    .await
    .unwrap();
    source
}

fn observation(source: ListingSourceId, url: &str, title: &str) -> ProductListingRawCaptureItem {
    let input = ProductListingNormalizationInput::new(
        RawProductListingOperation::Upsert,
        RawProductListingPayloadFormat::CrawlerExtractedProduct,
        1,
        1,
        SourcePayload::new(json!({ "title": title })).unwrap(),
        RawProductListingValues::new(json!({})).unwrap(),
        NormalizationContext::new(json!({})).unwrap(),
    )
    .unwrap();
    let provenance = RawProductListingProvenance::new(json!({})).unwrap();
    ProductListingRawCaptureItem::crawler(source, url, input, provenance)
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_capture_crawler_observations_admitted_through_the_ingestion_role() {
    let pool = get_postgres_client().await;
    let source = seed_source(&pool).await;
    let crawler = ProductListingRawCaptureServiceImpl::new(
        Arc::new(SubmitInternalProductListingIngestionHandler::new(
            QueueProductListingIngestionPublisher::postgres(crawler_role_pool(&pool).await),
        )),
        1,
    );

    let outcomes = crawler
        .capture(vec![
            observation(source, "https://shop.example/chair", "Chair"),
            observation(source, "https://shop.example/table", "Table"),
        ])
        .await;
    assert_eq!(
        vec![ProductListingRawCaptureOutcome::Persisted; 2],
        outcomes
    );

    let queue = JobQueue::new(pool.clone(), product_listing_ingestion_job_scope());
    let use_case = compose_ingestion_use_case(pool.clone());
    let jobs = queue.claim(10).await.unwrap();
    assert_eq!(2, jobs.len(), "each page is its own raw stream group");
    for job in jobs {
        let disposition =
            process_queued_ingestion_job(job.job_id(), job.payload(), use_case.as_ref()).await;
        assert_eq!(
            Settlement::Acknowledged,
            queue.settle(job.lease(), disposition).await.unwrap()
        );
    }

    let captured: Vec<String> = sqlx::query_scalar(
        "SELECT source_record_key FROM product_listing_raw_streams \
         WHERE listing_source_id = $1 AND ingestion_method = 'WEB_CRAWL' \
         ORDER BY source_record_key",
    )
    .bind(source.into_uuid())
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        vec![
            "https://shop.example/chair".to_owned(),
            "https://shop.example/table".to_owned()
        ],
        captured
    );
    assert_eq!(
        0,
        queue.inspect().await.unwrap().depth,
        "acknowledged jobs leave the queue"
    );
}
