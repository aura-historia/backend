use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::event_id::EventId;
use platform_postgres::{SqlxTransaction, SqlxUnitOfWork};
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_postgres::{
    SqlxProductListingEmbeddingQueueFactory, SqlxProductListingEmbeddingQueueReader,
    SqlxProductListingEmbeddingWriterFactory, oldest_unembedded_age,
};
use product_listing_service::ports::{
    EmbeddingBatchId, ProductEmbeddingAssignment, ProductEmbeddingBatchKey, ProductEmbeddingClaim,
    ProductEmbeddingScheduleOutcome, ProductEmbeddingWorkRef, ProductListingEmbeddingQueue,
    ProductListingEmbeddingQueueFactory, ProductListingEmbeddingQueueReader,
    ProductListingEmbeddingWrite, ProductListingEmbeddingWriter,
    ProductListingEmbeddingWriterFactory,
};
use std::time::Duration;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
const PROVIDER_BATCH: &str = "projects/p/locations/eu/batchPredictionJobs/1";
type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_schedule_only_the_current_unembedded_revision_and_keep_redelivered_work() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let unit_of_work = SqlxUnitOfWork::new(pool.clone());
        let work = insert_listing(&pool, "Antiker Stuhl").await?;

        assert_eq!(
            ProductEmbeddingScheduleOutcome::Scheduled,
            schedule(&unit_of_work, work).await?
        );
        let key = submit(&unit_of_work, 10).await?;
        assert_eq!(
            ProductEmbeddingScheduleOutcome::Scheduled,
            schedule(&unit_of_work, work).await?,
            "a redelivered job of the same revision"
        );
        let reader = SqlxProductListingEmbeddingQueueReader::new(pool.clone());
        assert_eq!(1, reader.batch_items(key).await?.len(), "kept in its batch");

        let newer = advance_content(&pool, work.product_listing_id, "Antiker Tisch").await?;
        assert_eq!(
            ProductEmbeddingScheduleOutcome::Superseded,
            schedule(&unit_of_work, work).await?
        );
        assert_eq!(
            ProductEmbeddingScheduleOutcome::Scheduled,
            schedule(&unit_of_work, newer).await?
        );
        assert!(
            reader.batch_items(key).await?.is_empty(),
            "replaced work leaves its batch"
        );
        let pending = lock(&unit_of_work, 10).await?;
        assert_eq!(1, pending.len());
        assert_eq!(newer, pending[0].work);
        assert_eq!("Antiker Tisch", pending[0].title.as_ref());
        assert_eq!(
            Some("Bemaltes Holz"),
            pending[0].description.as_ref().map(AsRef::as_ref)
        );
        assert_eq!(
            Some("https://example.test/chair.jpg"),
            pending[0].image_url.as_ref().map(url::Url::as_str)
        );

        commit_embedding(&unit_of_work, newer).await?;
        assert_eq!(
            ProductEmbeddingScheduleOutcome::AlreadyEmbedded,
            schedule(&unit_of_work, newer).await?
        );
        assert_eq!(
            ProductEmbeddingScheduleOutcome::Superseded,
            schedule(
                &unit_of_work,
                work_ref(ProductListingId::new(), EventId::new())
            )
            .await?
        );
        Ok(())
    }
    .await;
    assert!(result.is_ok(), "{result:?}");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_carry_work_through_a_batch_and_settle_it_only_under_its_claim() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let unit_of_work = SqlxUnitOfWork::new(pool.clone());
        let reader = SqlxProductListingEmbeddingQueueReader::new(pool.clone());
        let first = insert_listing(&pool, "Antiker Stuhl").await?;
        let second = insert_listing(&pool, "Barocke Kommode").await?;
        let superseded = insert_listing(&pool, "Alte Vase").await?;
        for work in [first, second, superseded] {
            schedule(&unit_of_work, work).await?;
        }
        advance_content(&pool, superseded.product_listing_id, "Neue Vase").await?;

        let key = submit(&unit_of_work, 10).await?;

        let batches = reader.submitted_batches().await?;
        assert_eq!(
            vec![key],
            batches.iter().map(|batch| batch.key).collect::<Vec<_>>()
        );
        assert_eq!(PROVIDER_BATCH, batches[0].provider_batch_id.as_str());
        assert!(batches[0].age < Duration::from_secs(60));
        let items = reader.batch_items(key).await?;
        assert_eq!(
            vec![(1, first), (2, second)],
            items
                .iter()
                .map(|item| (item.position, item.embedding.work))
                .collect::<Vec<_>>(),
            "superseded work is discarded before submission"
        );
        assert!(
            lock(&unit_of_work, 10).await?.is_empty(),
            "batched work is not pending"
        );

        in_queue(&unit_of_work, |queue| {
            Box::pin(async move {
                let claim = ProductEmbeddingClaim::Batch(key);
                queue.requeue(first, claim, 1).await?;
                queue.park(second, claim, 3).await?;
                // A round that read the batch before the work moved on changes nothing.
                queue.park(first, claim, 3).await
            })
        })
        .await?;
        in_queue(&unit_of_work, |queue| Box::pin(queue.close_batch(key))).await?;

        assert!(reader.submitted_batches().await?.is_empty());
        let pending = lock(&unit_of_work, 10).await?;
        assert_eq!(
            vec![(first, 1)],
            pending
                .iter()
                .map(|item| (item.work, item.attempts))
                .collect::<Vec<_>>(),
            "parked work waits"
        );

        in_queue(&unit_of_work, |queue| {
            Box::pin(queue.complete(first, ProductEmbeddingClaim::Batch(key)))
        })
        .await?;
        assert_eq!(
            1,
            lock(&unit_of_work, 10).await?.len(),
            "another claim's work stays"
        );
        in_queue(&unit_of_work, |queue| {
            Box::pin(queue.complete(first, ProductEmbeddingClaim::Pending))
        })
        .await?;
        assert!(lock(&unit_of_work, 10).await?.is_empty());

        let newer = advance_content(&pool, second.product_listing_id, "Barocke Truhe").await?;
        schedule(&unit_of_work, newer).await?;
        let pending = lock(&unit_of_work, 10).await?;
        assert_eq!(
            vec![(newer, 0)],
            pending
                .iter()
                .map(|item| (item.work, item.attempts))
                .collect::<Vec<_>>(),
            "a newer revision replaces parked work"
        );
        Ok(())
    }
    .await;
    assert!(result.is_ok(), "{result:?}");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_lock_pending_work_for_one_submitter_and_release_abandoned_submissions() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let unit_of_work = SqlxUnitOfWork::new(pool.clone());
        let work = insert_listing(&pool, "Antiker Stuhl").await?;
        schedule(&unit_of_work, work).await?;

        let mut holder = unit_of_work.begin().await?;
        let held = SqlxProductListingEmbeddingQueueFactory::new()
            .in_transaction(&mut holder)
            .lock_pending(10)
            .await?;
        assert_eq!(1, held.len());
        assert!(lock(&unit_of_work, 10).await?.is_empty(), "locked work is skipped");
        let key = ProductEmbeddingBatchKey::new();
        SqlxProductListingEmbeddingQueueFactory::new()
            .in_transaction(&mut holder)
            .create_batch(key, &[ProductEmbeddingAssignment { work, position: 1 }])
            .await?;
        holder.commit().await?;

        let closed = in_queue(&unit_of_work, |queue| {
            Box::pin(queue.close_abandoned_submissions(Duration::from_secs(3600)))
        })
        .await?;
        assert_eq!(0, closed, "a recent submission may still be recorded");
        sqlx::query("UPDATE product_listing_embedding_batches SET created = now() - interval '3 hours' WHERE batch_id = $1")
            .bind(key.as_uuid())
            .execute(&pool)
            .await?;
        let closed = in_queue(&unit_of_work, |queue| {
            Box::pin(queue.close_abandoned_submissions(Duration::from_secs(7200)))
        })
        .await?;
        assert_eq!(1, closed);
        assert_eq!(1, lock(&unit_of_work, 10).await?.len(), "its work is pending again");

        sqlx::query("DELETE FROM product_listings WHERE product_listing_id = $1")
            .bind(work.product_listing_id.as_uuid())
            .execute(&pool)
            .await?;
        let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM product_listing_embedding_work")
            .fetch_one(&pool)
            .await?;
        assert_eq!(0, remaining, "work is deleted with its listing");
        Ok(())
    }
    .await;
    assert!(result.is_ok(), "{result:?}");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_park_work_with_an_invalid_document_without_holding_back_the_rest() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let unit_of_work = SqlxUnitOfWork::new(pool.clone());
        let invalid = insert_listing(&pool, "Antiker Stuhl").await?;
        let valid = insert_listing(&pool, "Barocke Kommode").await?;
        for work in [invalid, valid] {
            schedule(&unit_of_work, work).await?;
        }
        sqlx::query(r#"UPDATE product_listings SET product_images = '[{"url": "ftp://example.test/a.jpg"}]' WHERE product_listing_id = $1"#)
            .bind(invalid.product_listing_id.as_uuid())
            .execute(&pool)
            .await?;

        assert_eq!(
            vec![valid],
            lock(&unit_of_work, 10).await?.iter().map(|item| item.work).collect::<Vec<_>>()
        );
        let parked: bool = sqlx::query_scalar(
            "SELECT parked IS NOT NULL FROM product_listing_embedding_work WHERE product_listing_id = $1",
        )
        .bind(invalid.product_listing_id.as_uuid())
        .fetch_one(&pool)
        .await?;
        assert!(parked, "submission parks it");
        Ok(())
    }
    .await;
    assert!(result.is_ok(), "{result:?}");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_measure_the_oldest_unembedded_work_but_not_parked_work() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let unit_of_work = SqlxUnitOfWork::new(pool.clone());
        let mut connection = pool.acquire().await?;
        assert_eq!(None, oldest_unembedded_age(&mut connection).await?);

        let parked = insert_listing(&pool, "Antiker Stuhl").await?;
        let waiting = insert_listing(&pool, "Barocke Kommode").await?;
        for work in [parked, waiting] {
            schedule(&unit_of_work, work).await?;
        }
        sqlx::query("UPDATE product_listing_embedding_work SET enqueued = now() - interval '2 days', parked = now() WHERE product_listing_id = $1")
            .bind(parked.product_listing_id.as_uuid())
            .execute(&pool)
            .await?;
        sqlx::query("UPDATE product_listing_embedding_work SET enqueued = now() - interval '3 hours' WHERE product_listing_id = $1")
            .bind(waiting.product_listing_id.as_uuid())
            .execute(&pool)
            .await?;

        let age = oldest_unembedded_age(&mut connection).await?.unwrap();
        assert!(
            (Duration::from_secs(3 * 3600)..Duration::from_secs(4 * 3600)).contains(&age),
            "{age:?}"
        );
        Ok(())
    }
    .await;
    assert!(result.is_ok(), "{result:?}");
}

type BoxedFuture<'q, T> = std::pin::Pin<
    Box<
        dyn std::future::Future<
                Output = Result<
                    T,
                    product_listing_service::ports::ProductListingEmbeddingQueueError,
                >,
            > + Send
            + 'q,
    >,
>;

async fn in_queue<T>(
    unit_of_work: &SqlxUnitOfWork,
    change: impl for<'q> FnOnce(&'q mut (dyn ProductListingEmbeddingQueue + 'q)) -> BoxedFuture<'q, T>,
) -> Result<T, Box<dyn std::error::Error + Send + Sync>> {
    let mut tx: SqlxTransaction = unit_of_work.begin().await?;
    let value = {
        let factory = SqlxProductListingEmbeddingQueueFactory::new();
        let mut queue = factory.in_transaction(&mut tx);
        change(&mut queue).await?
    };
    tx.commit().await?;
    Ok(value)
}

async fn schedule(
    unit_of_work: &SqlxUnitOfWork,
    work: ProductEmbeddingWorkRef,
) -> Result<ProductEmbeddingScheduleOutcome, Box<dyn std::error::Error + Send + Sync>> {
    in_queue(unit_of_work, |queue| Box::pin(queue.schedule(work))).await
}

async fn lock(
    unit_of_work: &SqlxUnitOfWork,
    limit: u32,
) -> Result<
    Vec<product_listing_service::ports::PendingProductEmbedding>,
    Box<dyn std::error::Error + Send + Sync>,
> {
    in_queue(unit_of_work, |queue| Box::pin(queue.lock_pending(limit))).await
}

/// Discards superseded work, assigns every pending embedding to a new batch in order, and
/// records its submission.
async fn submit(
    unit_of_work: &SqlxUnitOfWork,
    limit: u32,
) -> Result<ProductEmbeddingBatchKey, Box<dyn std::error::Error + Send + Sync>> {
    let key = ProductEmbeddingBatchKey::new();
    let mut tx = unit_of_work.begin().await?;
    {
        let factory = SqlxProductListingEmbeddingQueueFactory::new();
        let mut queue = factory.in_transaction(&mut tx);
        queue.discard_superseded().await?;
        let assignments = (1..)
            .zip(queue.lock_pending(limit).await?)
            .map(|(position, item)| ProductEmbeddingAssignment {
                work: item.work,
                position,
            })
            .collect::<Vec<_>>();
        queue.create_batch(key, &assignments).await?;
        queue
            .record_submission(key, &EmbeddingBatchId::new(PROVIDER_BATCH)?)
            .await?;
    }
    tx.commit().await?;
    Ok(key)
}

async fn commit_embedding(
    unit_of_work: &SqlxUnitOfWork,
    work: ProductEmbeddingWorkRef,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut tx = unit_of_work.begin().await?;
    SqlxProductListingEmbeddingWriterFactory::new()
        .in_transaction(&mut tx)
        .apply(&ProductListingEmbeddingWrite {
            product_listing_id: work.product_listing_id,
            source_event_id: work.source_event_id,
            enrichment_event_id: EventId::new(),
            embedding: vec![0.5; 768],
        })
        .await?;
    tx.commit().await?;
    Ok(())
}

fn work_ref(
    product_listing_id: ProductListingId,
    source_event_id: EventId,
) -> ProductEmbeddingWorkRef {
    ProductEmbeddingWorkRef {
        product_listing_id,
        source_event_id,
    }
}

async fn insert_listing(
    pool: &sqlx::PgPool,
    title: &str,
) -> Result<ProductEmbeddingWorkRef, sqlx::Error> {
    let product_listing_id = ProductListingId::new();
    let event_id = EventId::new();
    let party_id = uuid::Uuid::now_v7();
    let listing_source_id = uuid::Uuid::now_v7();
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, 'Embedding party')",
    )
    .bind(party_id)
    .bind(format!("embedding-party-{party_id}"))
    .execute(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id, listing_source_type) VALUES ($1, $2, 'Embedding source', $3, 'DEALER')")
        .bind(listing_source_id)
        .bind(format!("embedding-source-{listing_source_id}"))
        .bind(party_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(r#"INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, title_text, title_language, description_text, description_language, availability, lifecycle, url, product_images) VALUES ($1, $2, $3, $3, $3, $4, $5, $6, 'de', 'Bemaltes Holz', 'de', 'AVAILABLE', 'ACTIVE', 'https://example.test/product', '[{"url": "https://example.test/chair.jpg"}]')"#)
        .bind(product_listing_id.as_uuid())
        .bind(format!("embedding-queue-{}", &product_listing_id.as_uuid().simple().to_string()[26..]))
        .bind(event_id.as_uuid())
        .bind(listing_source_id)
        .bind(product_listing_id.to_string())
        .bind(title)
        .execute(&mut *tx)
        .await?;
    insert_event(&mut tx, product_listing_id, event_id).await?;
    tx.commit().await?;
    Ok(work_ref(product_listing_id, event_id))
}

/// Records a text change: a new embedding revision with a new title.
async fn advance_content(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
    title: &str,
) -> Result<ProductEmbeddingWorkRef, sqlx::Error> {
    let event_id = EventId::new();
    let mut tx = pool.begin().await?;
    insert_event(&mut tx, product_listing_id, event_id).await?;
    sqlx::query("UPDATE product_listings SET content_source_event_id = $1, embedding_source_event_id = $1, current_event_id = $1, title_text = $3 WHERE product_listing_id = $2")
        .bind(event_id.as_uuid())
        .bind(product_listing_id.as_uuid())
        .bind(title)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(work_ref(product_listing_id, event_id))
}

async fn insert_event(
    tx: &mut sqlx::PgTransaction<'_>,
    product_listing_id: ProductListingId,
    event_id: EventId,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_CHANGED', 'DOMAIN', 1, $3, now())")
        .bind(event_id.as_uuid())
        .bind(product_listing_id.as_uuid())
        .bind(serde_json::json!({"title": {"previous": null, "current": null}}))
        .execute(&mut **tx)
        .await?;
    Ok(())
}
