use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::event_id::EventId;
use indexmap::IndexMap;
use localization::Language;
use platform_postgres::{SqlxTransaction, SqlxUnitOfWork};
use product_listing_core::{product_listing_id::ProductListingId, title::Title};
use product_listing_postgres::{
    SqlxProductListingTitleTranslationQueueFactory, SqlxProductListingTitleTranslationQueueReader,
    SqlxProductListingTranslationWriterFactory,
};
use product_listing_service::ports::{
    ProductListingTitleTranslationQueue, ProductListingTitleTranslationQueueFactory,
    ProductListingTitleTranslationQueueReader, ProductListingTranslationWrite,
    ProductListingTranslationWriter, ProductListingTranslationWriterFactory,
    TitleTranslationAssignment, TitleTranslationBatchId, TitleTranslationBatchKey,
    TitleTranslationClaim, TitleTranslationMode, TitleTranslationScheduleOutcome,
    TitleTranslationWorkRef,
};
use std::time::Duration;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_schedule_only_the_current_untranslated_revision_and_keep_redelivered_work() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let unit_of_work = SqlxUnitOfWork::new(pool.clone());
        let work = insert_listing(&pool, "Antiker Stuhl").await?;

        assert_eq!(
            TitleTranslationScheduleOutcome::Scheduled,
            schedule(&unit_of_work, work).await?
        );
        let key = submit(&unit_of_work, 10).await?;
        assert_eq!(
            TitleTranslationScheduleOutcome::Scheduled,
            schedule(&unit_of_work, work).await?,
            "a redelivered job of the same revision"
        );
        let reader = SqlxProductListingTitleTranslationQueueReader::new(pool.clone());
        assert_eq!(1, reader.batch_items(key).await?.len(), "kept in its batch");

        let newer = advance_content(&pool, work.product_listing_id, "Antiker Tisch").await?;
        assert_eq!(
            TitleTranslationScheduleOutcome::Superseded,
            schedule(&unit_of_work, work).await?
        );
        assert_eq!(
            TitleTranslationScheduleOutcome::Scheduled,
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

        commit_translation(&unit_of_work, newer).await?;
        assert_eq!(
            TitleTranslationScheduleOutcome::AlreadyTranslated,
            schedule(&unit_of_work, newer).await?
        );
        assert_eq!(
            TitleTranslationScheduleOutcome::Superseded,
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
async fn should_carry_work_through_a_batch_and_back_to_the_queue() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let unit_of_work = SqlxUnitOfWork::new(pool.clone());
        let reader = SqlxProductListingTitleTranslationQueueReader::new(pool.clone());
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
        assert_eq!("msgbatch_test", batches[0].provider_batch_id.as_str());
        let items = reader.batch_items(key).await?;
        assert_eq!(
            vec![(1, 1, first), (1, 2, second)],
            items
                .iter()
                .map(|item| (item.group, item.position, item.translation.work))
                .collect::<Vec<_>>(),
            "superseded work is discarded before submission"
        );
        assert!(
            lock(&unit_of_work, 10).await?.is_empty(),
            "batched work is not pending"
        );

        in_queue(&unit_of_work, |queue| {
            Box::pin(async move {
                let claim = TitleTranslationClaim::Batch(key);
                queue
                    .requeue(first, claim, TitleTranslationMode::RefusalFallback, 0)
                    .await?;
                queue.park(second, claim, 3).await?;
                // A round that read the batch before the work moved on changes nothing.
                queue.park(first, claim, 3).await
            })
        })
        .await?;
        in_queue(&unit_of_work, |queue| Box::pin(queue.close_batch(key))).await?;

        assert!(reader.submitted_batches().await?.is_empty());
        let fallbacks = reader.refusal_fallbacks(10).await?;
        assert_eq!(
            vec![first],
            fallbacks.iter().map(|item| item.work).collect::<Vec<_>>()
        );
        assert!(
            lock(&unit_of_work, 10).await?.is_empty(),
            "fallback and parked work wait"
        );

        in_queue(&unit_of_work, |queue| {
            Box::pin(queue.complete(first, TitleTranslationClaim::Batch(key)))
        })
        .await?;
        assert_eq!(
            1,
            reader.refusal_fallbacks(10).await?.len(),
            "another claim's work stays"
        );
        in_queue(&unit_of_work, |queue| {
            Box::pin(queue.complete(first, TitleTranslationClaim::RefusalFallback))
        })
        .await?;
        assert!(reader.refusal_fallbacks(10).await?.is_empty());
        let newer = advance_content(&pool, second.product_listing_id, "Barocke Truhe").await?;
        schedule(&unit_of_work, newer).await?;
        let pending = lock(&unit_of_work, 10).await?;
        assert_eq!(
            vec![(newer, TitleTranslationMode::Grouped, 0)],
            pending
                .iter()
                .map(|item| (item.work, item.mode, item.attempts))
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
        let held = SqlxProductListingTitleTranslationQueueFactory::new()
            .in_transaction(&mut holder)
            .lock_pending(10)
            .await?;
        assert_eq!(1, held.len());
        assert!(lock(&unit_of_work, 10).await?.is_empty(), "locked work is skipped");
        let key = TitleTranslationBatchKey::new();
        SqlxProductListingTitleTranslationQueueFactory::new()
            .in_transaction(&mut holder)
            .create_batch(key, &[TitleTranslationAssignment { work, group: 1, position: 1 }])
            .await?;
        holder.commit().await?;

        let closed = in_queue(&unit_of_work, |queue| {
            Box::pin(queue.close_abandoned_submissions(Duration::from_secs(3600)))
        })
        .await?;
        assert_eq!(0, closed, "a recent submission may still be recorded");
        sqlx::query("UPDATE product_listing_title_translation_batches SET created = now() - interval '1 hour' WHERE batch_id = $1")
            .bind(key.as_uuid())
            .execute(&pool)
            .await?;
        let closed = in_queue(&unit_of_work, |queue| {
            Box::pin(queue.close_abandoned_submissions(Duration::from_secs(600)))
        })
        .await?;
        assert_eq!(1, closed);
        assert_eq!(1, lock(&unit_of_work, 10).await?.len(), "its work is pending again");

        sqlx::query("DELETE FROM product_listings WHERE product_listing_id = $1")
            .bind(work.product_listing_id.as_uuid())
            .execute(&pool)
            .await?;
        let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM product_listing_title_translation_work")
            .fetch_one(&pool)
            .await?;
        assert_eq!(0, remaining, "work is deleted with its listing");
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
                    product_listing_service::ports::ProductListingTitleTranslationQueueError,
                >,
            > + Send
            + 'q,
    >,
>;

async fn in_queue<T>(
    unit_of_work: &SqlxUnitOfWork,
    change: impl for<'q> FnOnce(
        &'q mut (dyn ProductListingTitleTranslationQueue + 'q),
    ) -> BoxedFuture<'q, T>,
) -> Result<T, Box<dyn std::error::Error + Send + Sync>> {
    let mut tx: SqlxTransaction = unit_of_work.begin().await?;
    let value = {
        let factory = SqlxProductListingTitleTranslationQueueFactory::new();
        let mut queue = factory.in_transaction(&mut tx);
        change(&mut queue).await?
    };
    tx.commit().await?;
    Ok(value)
}

async fn schedule(
    unit_of_work: &SqlxUnitOfWork,
    work: TitleTranslationWorkRef,
) -> Result<TitleTranslationScheduleOutcome, Box<dyn std::error::Error + Send + Sync>> {
    in_queue(unit_of_work, |queue| Box::pin(queue.schedule(work))).await
}

async fn lock(
    unit_of_work: &SqlxUnitOfWork,
    limit: u32,
) -> Result<
    Vec<product_listing_service::ports::PendingTitleTranslation>,
    Box<dyn std::error::Error + Send + Sync>,
> {
    in_queue(unit_of_work, |queue| Box::pin(queue.lock_pending(limit))).await
}

/// Discards superseded work, assigns every pending title to group 1 of a new batch in order,
/// and records its submission.
async fn submit(
    unit_of_work: &SqlxUnitOfWork,
    limit: u32,
) -> Result<TitleTranslationBatchKey, Box<dyn std::error::Error + Send + Sync>> {
    let key = TitleTranslationBatchKey::new();
    let mut tx = unit_of_work.begin().await?;
    {
        let factory = SqlxProductListingTitleTranslationQueueFactory::new();
        let mut queue = factory.in_transaction(&mut tx);
        queue.discard_superseded().await?;
        let assignments = (1..)
            .zip(queue.lock_pending(limit).await?)
            .map(|(position, item)| TitleTranslationAssignment {
                work: item.work,
                group: 1,
                position,
            })
            .collect::<Vec<_>>();
        queue.create_batch(key, &assignments).await?;
        queue
            .record_submission(key, &TitleTranslationBatchId::new("msgbatch_test")?)
            .await?;
    }
    tx.commit().await?;
    Ok(key)
}

async fn commit_translation(
    unit_of_work: &SqlxUnitOfWork,
    work: TitleTranslationWorkRef,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut tx = unit_of_work.begin().await?;
    SqlxProductListingTranslationWriterFactory::new()
        .in_transaction(&mut tx)
        .apply(&ProductListingTranslationWrite {
            product_listing_id: work.product_listing_id,
            source_event_id: work.source_event_id,
            enrichment_event_id: EventId::new(),
            source_language: Language::De,
            titles: IndexMap::from([(Language::En, Title::from("Antique table"))]),
        })
        .await?;
    tx.commit().await?;
    Ok(())
}

fn work_ref(
    product_listing_id: ProductListingId,
    source_event_id: EventId,
) -> TitleTranslationWorkRef {
    TitleTranslationWorkRef {
        product_listing_id,
        source_event_id,
    }
}

async fn insert_listing(
    pool: &sqlx::PgPool,
    title: &str,
) -> Result<TitleTranslationWorkRef, sqlx::Error> {
    let product_listing_id = ProductListingId::new();
    let event_id = EventId::new();
    let party_id = uuid::Uuid::now_v7();
    let listing_source_id = uuid::Uuid::now_v7();
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, 'Translation party')",
    )
    .bind(party_id)
    .bind(format!("translation-party-{party_id}"))
    .execute(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id, listing_source_type) VALUES ($1, $2, 'Translation source', $3, 'DEALER')")
        .bind(listing_source_id)
        .bind(format!("translation-source-{listing_source_id}"))
        .bind(party_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, title_text, title_language, availability, lifecycle, url, product_images) VALUES ($1, $2, $3, $3, $3, $4, $5, $6, 'de', 'AVAILABLE', 'ACTIVE', 'https://example.test/product', '[]')")
        .bind(product_listing_id.as_uuid())
        .bind(format!("translation-queue-{}", &product_listing_id.as_uuid().simple().to_string()[26..]))
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

/// Records a text change: a new content revision with a new title.
async fn advance_content(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
    title: &str,
) -> Result<TitleTranslationWorkRef, sqlx::Error> {
    let event_id = EventId::new();
    let mut tx = pool.begin().await?;
    insert_event(&mut tx, product_listing_id, event_id).await?;
    sqlx::query("UPDATE product_listings SET content_source_event_id = $1, current_event_id = $1, title_text = $3 WHERE product_listing_id = $2")
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
