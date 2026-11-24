mod support;

use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use aura_historia_jobs::{
    DomainJob, DomainJobPayload, IdempotencyKey, OrderingKey, ProductListingEventJob, WorkerQueue,
    encode,
};
use aura_historia_worker::scopes::product_translation::{
    ProductTranslationJobs, compose_product_translation_use_case,
};
use aura_historia_worker::translation_batches::compose_round_use_case;
use domain_primitives::event_id::EventId;
use listing_source_core::ListingSourceId;
use platform_postgres_queue::{Disposition, JobId};
use platform_worker::{Job, JobHandler, error_class};
use product_listing_core::{
    product_listing_id::ProductListingId, product_listing_slug_id::ProductListingSlugId,
};
use product_listing_service::use_cases::{
    TitleTranslationRoundReport, TranslateProductListingTitleBatchesUseCase,
};
use serde_json::json;
use std::sync::atomic::Ordering;
use support::{FakeBatches, FakeFallback};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

fn translation(language: &str) -> String {
    match language {
        "en" => "Antique oak chair",
        "fr" => "Chaise ancienne en chêne",
        "es" => "Silla antigua de roble",
        "it" => "Sedia antica in rovere",
        _ => "Antiker Eichenstuhl",
    }
    .to_owned()
}

fn fallback_translation(language: &str) -> String {
    format!("Fallback title in {language}")
}

async fn round(
    rounds: &dyn TranslateProductListingTitleBatchesUseCase,
) -> TitleTranslationRoundReport {
    rounds
        .execute(&OperationContext {
            principal: Principal::System,
            request_id: RequestId::new("round"),
            correlation_id: CorrelationId::new("round"),
        })
        .await
        .expect("translation round")
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn scheduled_title_is_translated_by_a_batch_once_and_a_duplicate_job_is_a_noop() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let handler = ProductTranslationJobs::new(compose_product_translation_use_case(pool.clone()));
        let batches = FakeBatches::new(translation);
        let rounds = compose_round_use_case(pool.clone(), batches.clone(), FakeFallback::new(fallback_translation));
        let missing = (ProductListingId::new(), EventId::new());
        assert_eq!(Disposition::Retry(error_class("missing_source")), handler.handle(&job(&job_body(missing.0, missing.1)?)).await);

        let (product, source) = seed_discovery(&pool, "Antiker Eichenstuhl").await?;
        let body = job_body(product, source)?;
        assert_eq!(Disposition::Complete, handler.handle(&job(&body)).await);
        assert!(translations(&pool, product).await?.is_empty(), "the job only schedules");

        let submitted = round(rounds.as_ref()).await;
        assert_eq!((1, 1), (submitted.submitted_groups, submitted.submitted_titles));
        let collected = round(rounds.as_ref()).await;
        assert_eq!((1, 1), (collected.batches_collected, collected.applied));
        let committed = translations(&pool, product).await?;
        assert_eq!(vec![
            ("en".to_owned(), "Antique oak chair".to_owned(), *source.as_uuid()),
            ("es".to_owned(), "Silla antigua de roble".to_owned(), *source.as_uuid()),
            ("fr".to_owned(), "Chaise ancienne en chêne".to_owned(), *source.as_uuid()),
            ("it".to_owned(), "Sedia antica in rovere".to_owned(), *source.as_uuid()),
        ], committed);
        assert_eq!(1, completion_count(&pool, product).await?);
        let payload: serde_json::Value = sqlx::query_scalar("SELECT payload FROM product_listing_events WHERE product_listing_id = $1 AND event_type = 'ENRICHMENT_TRANSLATED_TITLES'")
            .bind(product.as_uuid()).fetch_one(&pool).await?;
        assert_eq!(json!({"sourceEventId": source.as_uuid().to_string(), "sourceLanguage": "de", "targetLanguages": ["en", "fr", "es", "it"]}), payload);

        assert_eq!(Disposition::Complete, handler.handle(&job(&body)).await);
        let idle = round(rounds.as_ref()).await;
        assert_eq!(0, idle.submitted_titles, "a translated revision is not scheduled again");
        assert_eq!(committed, translations(&pool, product).await?);
        assert_eq!(1, batches.submissions().len());
        Ok(())
    }.await;
    assert!(
        result.is_ok(),
        "translation scheduling and batch commit failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn result_for_a_revision_superseded_while_its_batch_runs_is_dropped() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let handler = ProductTranslationJobs::new(compose_product_translation_use_case(pool.clone()));
        let batches = FakeBatches::new(translation);
        batches.ended.store(false, Ordering::SeqCst);
        let rounds = compose_round_use_case(pool.clone(), batches.clone(), FakeFallback::new(fallback_translation));
        let (product, source) = seed_discovery(&pool, "Antiker Eichenstuhl").await?;
        assert_eq!(Disposition::Complete, handler.handle(&job(&job_body(product, source)?)).await);
        assert_eq!(1, round(rounds.as_ref()).await.submitted_titles);
        assert_eq!(1, round(rounds.as_ref()).await.batches_in_progress);

        let newer = EventId::new();
        let mut tx = pool.begin().await?;
        sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_CHANGED', 'DOMAIN', 1, $3, now())")
            .bind(newer.as_uuid()).bind(product.as_uuid())
            .bind(json!({"images": {"previousCount": 0, "currentCount": 0}}))
            .execute(&mut *tx).await?;
        sqlx::query("UPDATE product_listings SET content_source_event_id = $1, current_event_id = $1, version = version + 1 WHERE product_listing_id = $2")
            .bind(newer.as_uuid()).bind(product.as_uuid()).execute(&mut *tx).await?;
        tx.commit().await?;
        batches.ended.store(true, Ordering::SeqCst);

        let collected = round(rounds.as_ref()).await;
        assert_eq!((1, 0), (collected.batches_collected, collected.applied));
        assert!(translations(&pool, product).await?.is_empty());
        assert_eq!(0, completion_count(&pool, product).await?);
        let work: i64 = sqlx::query_scalar("SELECT count(*) FROM product_listing_title_translation_work")
            .fetch_one(&pool).await?;
        assert_eq!(0, work, "superseded work is discarded");
        let (current, marker): (uuid::Uuid, uuid::Uuid) = sqlx::query_as(
            "SELECT current_event_id, content_source_event_id FROM product_listings WHERE product_listing_id = $1"
        ).bind(product.as_uuid()).fetch_one(&pool).await?;
        assert_eq!((*newer.as_uuid(), *newer.as_uuid()), (current, marker));
        Ok(())
    }.await;
    assert!(
        result.is_ok(),
        "stale batch result barrier failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn refused_group_is_split_and_only_its_refused_title_reaches_the_fallback() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let handler =
            ProductTranslationJobs::new(compose_product_translation_use_case(pool.clone()));
        let mut batches = FakeBatches::new(translation);
        batches.refuse = Some("Pestbakterien");
        let fallback = FakeFallback::new(fallback_translation);
        let rounds = compose_round_use_case(pool.clone(), batches.clone(), fallback.clone());
        let (chair, chair_source) = seed_discovery(&pool, "Antiker Eichenstuhl").await?;
        let (refused, refused_source) = seed_discovery(&pool, "Kulturen von Pestbakterien").await?;
        for (product, source) in [(chair, chair_source), (refused, refused_source)] {
            assert_eq!(
                Disposition::Complete,
                handler.handle(&job(&job_body(product, source)?)).await
            );
        }

        assert_eq!((1, 2), {
            let report = round(rounds.as_ref()).await;
            (report.submitted_groups, report.submitted_titles)
        });
        let split = round(rounds.as_ref()).await;
        assert_eq!(
            (2, 0, 2),
            (split.requeued, split.applied, split.submitted_groups),
            "the refused group is resubmitted title by title"
        );
        let settled = round(rounds.as_ref()).await;
        assert_eq!((2, 1), (settled.applied, settled.refusal_fallbacks));
        assert_eq!(1, fallback.calls.load(Ordering::SeqCst));

        assert_eq!("Antique oak chair", translations(&pool, chair).await?[0].1);
        assert_eq!(
            "Fallback title in en",
            translations(&pool, refused).await?[0].1
        );
        let submissions = batches.submissions();
        assert_eq!(
            vec![1, 2],
            submissions.iter().map(Vec::len).collect::<Vec<_>>()
        );
        Ok(())
    }
    .await;
    assert!(result.is_ok(), "refusal splitting failed: {result:?}");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn listing_not_for_sale_waits_until_it_returns_to_sale_and_then_translates_its_current_revision()
 {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let handler =
            ProductTranslationJobs::new(compose_product_translation_use_case(pool.clone()));
        let rounds = compose_round_use_case(
            pool.clone(),
            FakeBatches::new(translation),
            FakeFallback::new(fallback_translation),
        );
        let (product, discovery) = seed_discovery(&pool, "Antiker Eichenstuhl").await?;
        sqlx::query(
            "UPDATE product_listings SET availability = 'SOLD_OUT' WHERE product_listing_id = $1",
        )
        .bind(product.as_uuid())
        .execute(&pool)
        .await?;
        let discovery_job = job_body(product, discovery)?;
        assert_eq!(
            Disposition::Complete,
            handler.handle(&job(&discovery_job)).await
        );
        let retitled = append_change(
            &pool,
            product,
            json!({"title": {"previous": {"language": "de", "text": "Antiker Eichenstuhl"},
                "current": {"language": "de", "text": "Antiker Stuhl"}}}),
            "UPDATE product_listings SET title_text = 'Antiker Stuhl', content_source_event_id = $1, current_event_id = $1, version = version + 1 WHERE product_listing_id = $2",
        )
        .await?;
        assert_eq!(
            Disposition::Complete,
            handler.handle(&job(&job_body(product, retitled)?)).await
        );
        assert_eq!(
            None,
            scheduled_revision(&pool, product).await?,
            "nothing is scheduled while sold"
        );

        let restocked = append_change(
            &pool,
            product,
            json!({"availability": {"previous": "SOLD_OUT", "current": "IN_STOCK"}}),
            "UPDATE product_listings SET availability = 'IN_STOCK', current_event_id = $1, version = version + 1 WHERE product_listing_id = $2",
        )
        .await?;
        let restocked_job = job_body(product, restocked)?;
        assert_eq!(
            Disposition::Complete,
            handler.handle(&job(&restocked_job)).await
        );
        assert_eq!(
            Some(*retitled.as_uuid()),
            scheduled_revision(&pool, product).await?,
            "the catch-up targets the current content revision, not its own event"
        );
        assert_eq!(
            Disposition::Complete,
            handler.handle(&job(&discovery_job)).await,
            "the late discovery job is stale"
        );
        assert_eq!(
            Some(*retitled.as_uuid()),
            scheduled_revision(&pool, product).await?
        );

        assert_eq!(1, round(rounds.as_ref()).await.submitted_titles);
        assert_eq!(1, round(rounds.as_ref()).await.applied);
        let committed = translations(&pool, product).await?;
        assert_eq!(4, committed.len());
        assert!(
            committed
                .iter()
                .all(|(_, _, source)| source == retitled.as_uuid())
        );
        assert_eq!(
            Disposition::Complete,
            handler.handle(&job(&restocked_job)).await
        );
        assert_eq!(
            None,
            scheduled_revision(&pool, product).await?,
            "a translated revision is a duplicate"
        );
        assert_eq!(1, completion_count(&pool, product).await?);
        Ok(())
    }
    .await;
    assert!(result.is_ok(), "return-to-sale catch-up failed: {result:?}");
}

/// Appends a changed event and runs `update` for it, which binds the event ID to `$1` and the
/// listing ID to `$2`.
async fn append_change(
    pool: &sqlx::PgPool,
    product: ProductListingId,
    payload: serde_json::Value,
    update: &'static str,
) -> Result<EventId, sqlx::Error> {
    let event = EventId::new();
    let mut tx = pool.begin().await?;
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_CHANGED', 'DOMAIN', 1, $3, now())")
        .bind(event.as_uuid()).bind(product.as_uuid()).bind(payload)
        .execute(&mut *tx).await?;
    sqlx::query(update)
        .bind(event.as_uuid())
        .bind(product.as_uuid())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(event)
}

async fn scheduled_revision(
    pool: &sqlx::PgPool,
    product: ProductListingId,
) -> Result<Option<uuid::Uuid>, sqlx::Error> {
    sqlx::query_scalar("SELECT source_event_id FROM product_listing_title_translation_work WHERE product_listing_id = $1")
        .bind(product.as_uuid()).fetch_optional(pool).await
}

async fn translations(
    pool: &sqlx::PgPool,
    product: ProductListingId,
) -> Result<Vec<(String, String, uuid::Uuid)>, sqlx::Error> {
    sqlx::query_as("SELECT language, title, source_event_id FROM product_listing_translations WHERE product_listing_id = $1 ORDER BY language")
        .bind(product.as_uuid()).fetch_all(pool).await
}

async fn completion_count(
    pool: &sqlx::PgPool,
    product: ProductListingId,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT count(*) FROM product_listing_events WHERE product_listing_id = $1 AND event_type = 'ENRICHMENT_TRANSLATED_TITLES'")
        .bind(product.as_uuid()).fetch_one(pool).await
}

async fn seed_discovery(
    pool: &sqlx::PgPool,
    title: &str,
) -> Result<(ProductListingId, EventId), Box<dyn std::error::Error + Send + Sync>> {
    let product = ProductListingId::new();
    let source = EventId::new();
    let listing_source = ListingSourceId::new();
    let slug = ProductListingSlugId::from_title_and_suffix(
        "translation lambda product",
        &product.as_uuid().simple().to_string()[26..],
    )
    .map_err(|_| "invalid fixture slug")?;
    let mut tx = pool.begin().await?;
    sqlx::query("WITH operator AS (INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, concat($2, '-operator'), 'Fixture operator') RETURNING party_id) INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id, listing_source_type) SELECT $3, $2, 'Translation lambda source', party_id, 'DEALER' FROM operator")
        .bind(uuid::Uuid::now_v7()).bind(format!("translation-lambda-source-{}", listing_source.as_uuid()))
        .bind(listing_source.as_uuid()).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, title_text, title_language, availability, lifecycle, url, product_images) VALUES ($1, $2, $3, $3, $3, $4, $5, $6, 'de', 'AVAILABLE', 'ACTIVE', 'https://example.test/product', '[]')")
        .bind(product.as_uuid()).bind(slug.as_ref()).bind(source.as_uuid())
        .bind(listing_source.as_uuid()).bind(product.to_string()).bind(title).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_DISCOVERED', 'DOMAIN', 1, $3, now())")
        .bind(source.as_uuid()).bind(product.as_uuid()).bind(json!({
            "listingSourceId": listing_source.as_uuid().to_string(), "sourceListingId": product.to_string(),
            "title": {"language": "de", "text": title}, "description": null,
            "pricing": {"price": null, "priceEstimateMin": null, "priceEstimateMax": null},
            "availability": "AVAILABLE", "url": "https://example.test/product", "imageCount": 0, "auction": null
        })).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok((product, source))
}

fn job_body(
    product: ProductListingId,
    source: EventId,
) -> Result<String, aura_historia_jobs::WireError> {
    encode(&DomainJob {
        target_queue: WorkerQueue::ProductListingTranslate,
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
