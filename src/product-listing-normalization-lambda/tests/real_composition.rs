use application::transaction::{Transaction, UnitOfWork};
use aura_historia_jobs::{
    DomainJob, DomainJobPayload, IdempotencyKey, OrderingKey, ProductListingRawRevisionJob,
    WorkerQueue, encode,
};
use aws_lambda_events::sqs::{SqsBatchResponse, SqsEvent, SqsMessage};
use lambda_runtime::{Context, LambdaEvent};
use listing_source_core::ListingSourceId;
use platform_postgres::SqlxUnitOfWork;
use product_listing_normalization::{
    NormalizationContext, ProductListingNormalizationInput, RawProductListingOperation,
    RawProductListingPayloadFormat, RawProductListingProvenance, RawProductListingValues,
    SourcePayload,
};
use product_listing_normalization_lambda::{
    MAX_REVISIONS_PER_STREAM, compose_normalization_use_case, handler,
};
use product_listing_postgres::SqlxProductListingRawCaptureWriterFactory;
use product_listing_service::ports::{
    ProductListingRawCaptureWrite, ProductListingRawCaptureWriteOutcome,
    ProductListingRawCaptureWriter, ProductListingRawCaptureWriterFactory,
    ProductListingRawIngestionMethod, ProductListingRawRevisionId, ProductListingRawStreamId,
    SourceRecordKeySha256,
};
use serde_json::json;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;
type Revision = (ProductListingRawStreamId, ProductListingRawRevisionId, u64);

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn reversed_duplicate_wakeups_drain_only_their_stream_from_persisted_progress() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let source = seed_listing_source(&pool).await?;
        let first = capture(&pool, raw_write(source, "ordered", 1, 100)).await?;
        let second = capture(&pool, raw_write(source, "ordered", 1, 120)).await?;
        let unrelated_source = seed_listing_source(&pool).await?;
        let unrelated = capture(&pool, raw_write(unrelated_source, "other", 2, 200)).await?;
        assert_eq!(first.0, second.0);
        assert_eq!((first.2, second.2, unrelated.2), (1, 2, 1));
        let use_case = compose_normalization_use_case(pool.clone());

        // There is no startup reconciliation: merely composing the Lambda does not touch either stream.
        assert_eq!(0, count(&pool, "product_listing_raw_normalizations").await?);
        assert_eq!(0, count(&pool, "product_listing_raw_normalization_heads").await?);
        assert!(failure_ids(handler(event([("newer", job_body(second)?)]), use_case.as_ref()).await?).is_empty());
        assert_eq!(2, head(&pool, first.0).await?);
        assert_eq!(None, optional_head(&pool, unrelated.0).await?);
        let rows: Vec<(i64, String)> = sqlx::query_as("SELECT revision, outcome FROM product_listing_raw_normalizations WHERE product_listing_raw_stream_id = $1 ORDER BY revision")
            .bind(first.0.as_uuid()).fetch_all(&pool).await?;
        assert_eq!(vec![(1, "APPLIED".to_owned()), (2, "APPLIED".to_owned())], rows);
        let product: (i64, String, String) = sqlx::query_as("SELECT price_amount, price_currency, lifecycle FROM product_listings WHERE listing_source_id = $1 AND source_listing_id = 'lambda-source-123'")
            .bind(source.as_uuid()).fetch_one(&pool).await?;
        assert_eq!((12_000, "EUR".to_owned(), "ACTIVE".to_owned()), product);
        assert_eq!(2, count(&pool, "product_listing_events").await?);

        assert!(failure_ids(handler(event([("older", job_body(first)?), ("duplicate", job_body(second)?)]), use_case.as_ref()).await?).is_empty());
        assert_eq!(2, head(&pool, first.0).await?);
        assert_eq!(2, count(&pool, "product_listing_raw_normalizations").await?);
        assert_eq!(2, count(&pool, "product_listing_events").await?);
        assert_eq!(None, optional_head(&pool, unrelated.0).await?);

        assert!(failure_ids(handler(event([("other", job_body(unrelated)?)]), use_case.as_ref()).await?).is_empty());
        assert_eq!(1, head(&pool, unrelated.0).await?);
        assert_eq!(3, count(&pool, "product_listing_raw_normalizations").await?);
        assert_eq!(3, count(&pool, "product_listing_events").await?);
        Ok(())
    }.await;
    assert!(
        result.is_ok(),
        "ordered Lambda composition failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn cancellation_before_commit_leaves_no_progress_and_redelivery_replays_once() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let source = seed_listing_source(&pool).await?;
        let revision = capture(&pool, raw_write(source, "cancel", 3, 100)).await?;
        let body = job_body(revision)?;
        let use_case = compose_normalization_use_case(pool.clone());

        let mut barrier = pool.begin().await?;
        sqlx::query("LOCK TABLE product_listing_raw_normalizations IN ACCESS EXCLUSIVE MODE")
            .execute(&mut *barrier)
            .await?;
        let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *barrier)
            .await?;
        let pending = tokio::spawn({
            let use_case = use_case.clone();
            let body = body.clone();
            async move { handler(event([("cancelled", body)]), use_case.as_ref()).await }
        });
        wait_for_blocked_normalization(&pool, blocker_pid).await?;
        pending.abort();
        assert!(
            pending.await.is_err(),
            "attempt must be cancelled before commit"
        );
        barrier.commit().await?;
        for table in [
            "product_listing_raw_normalizations",
            "product_listing_raw_normalization_heads",
            "product_listings",
            "product_listing_events",
        ] {
            assert_eq!(
                0,
                count(&pool, table).await?,
                "uncommitted {table} survived cancellation"
            );
        }

        let restarted = compose_normalization_use_case(pool.clone());
        assert!(
            failure_ids(handler(event([("redelivery", body.clone())]), restarted.as_ref()).await?)
                .is_empty()
        );
        assert_eq!(1, head(&pool, revision.0).await?);
        for table in [
            "product_listing_raw_normalizations",
            "product_listing_raw_normalization_heads",
            "product_listings",
            "product_listing_events",
        ] {
            assert_eq!(
                1,
                count(&pool, table).await?,
                "redelivery must commit one {table}"
            );
        }
        assert!(
            failure_ids(handler(event([("duplicate", body)]), use_case.as_ref()).await?).is_empty()
        );
        assert_eq!(1, head(&pool, revision.0).await?);
        assert_eq!(1, count(&pool, "product_listing_events").await?);
        assert_eq!(1, count(&pool, "product_listing_raw_normalizations").await?);
        Ok(())
    }
    .await;
    assert!(
        result.is_ok(),
        "cancel/replay Lambda composition failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn postgres_write_failure_retains_item_and_replay_resumes_from_committed_head() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let source = seed_listing_source(&pool).await?;
        let first = capture(&pool, raw_write(source, "retry", 5, 100)).await?;
        let use_case = compose_normalization_use_case(pool.clone());
        assert!(failure_ids(handler(event([("first", job_body(first)?)]), use_case.as_ref()).await?).is_empty());
        let second = capture(&pool, raw_write(source, "retry", 5, 120)).await?;
        let third = capture(&pool, raw_write(source, "retry", 5, 130)).await?;
        assert_eq!((first.0, first.0), (second.0, third.0));

        // Fail after the canonical write, but before the normalization head and event commit.
        sqlx::query("CREATE FUNCTION reject_raw_normalization_test_write() RETURNS trigger LANGUAGE plpgsql AS 'BEGIN RAISE EXCEPTION ''test-only normalization write failure''; END'")
            .execute(&pool).await?;
        sqlx::query("CREATE TRIGGER reject_raw_normalization_test_write BEFORE INSERT ON product_listing_raw_normalizations FOR EACH ROW EXECUTE FUNCTION reject_raw_normalization_test_write()")
            .execute(&pool).await?;
        let body = job_body(third)?;
        let response = handler(event([("failed-write", body.clone())]), use_case.as_ref()).await?;
        assert_eq!(vec!["failed-write"], failure_ids(response));
        assert_eq!(1, head(&pool, first.0).await?);
        assert_eq!(1, count(&pool, "product_listing_raw_normalizations").await?);
        assert_eq!(1, count(&pool, "product_listing_events").await?);
        let price: i64 = sqlx::query_scalar("SELECT price_amount FROM product_listings WHERE listing_source_id = $1 AND source_listing_id = 'lambda-source-123'")
            .bind(source.as_uuid()).fetch_one(&pool).await?;
        assert_eq!(10_000, price, "failed transaction must not change the canonical listing");

        sqlx::query("DROP TRIGGER reject_raw_normalization_test_write ON product_listing_raw_normalizations")
            .execute(&pool).await?;
        sqlx::query("DROP FUNCTION reject_raw_normalization_test_write()")
            .execute(&pool).await?;
        assert!(failure_ids(handler(event([("replay", body.clone())]), use_case.as_ref()).await?).is_empty());
        assert_eq!(3, head(&pool, first.0).await?);
        let revisions: Vec<i64> = sqlx::query_scalar("SELECT revision FROM product_listing_raw_normalizations WHERE product_listing_raw_stream_id = $1 ORDER BY revision")
            .bind(first.0.as_uuid()).fetch_all(&pool).await?;
        assert_eq!(vec![1, 2, 3], revisions);
        assert_eq!(3, count(&pool, "product_listing_events").await?);
        assert!(failure_ids(handler(event([("duplicate", body)]), use_case.as_ref()).await?).is_empty());
        assert_eq!(3, count(&pool, "product_listing_raw_normalizations").await?);
        assert_eq!(3, count(&pool, "product_listing_events").await?);
        Ok(())
    }.await;
    assert!(
        result.is_ok(),
        "PostgreSQL failure/replay Lambda composition failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn capped_stream_retains_sqs_item_until_a_later_invocation_drains_remaining_revisions() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let source = seed_listing_source(&pool).await?;
        let mut latest = None;
        for index in 0..=MAX_REVISIONS_PER_STREAM {
            latest = Some(capture(&pool, raw_write(source, "capped", 4, 100 + index)).await?);
        }
        let latest = latest.ok_or("missing captured revision")?;
        assert_eq!(u64::from(MAX_REVISIONS_PER_STREAM) + 1, latest.2);
        let body = job_body(latest)?;
        let use_case = compose_normalization_use_case(pool.clone());

        let first = handler(event([("capped", body.clone())]), use_case.as_ref()).await?;
        assert_eq!(
            i64::from(MAX_REVISIONS_PER_STREAM),
            head(&pool, latest.0).await?
        );
        assert_eq!(
            i64::from(MAX_REVISIONS_PER_STREAM),
            count(&pool, "product_listing_raw_normalizations").await?
        );
        let capped_failures = failure_ids(first);

        let restarted = compose_normalization_use_case(pool.clone());
        assert!(
            failure_ids(
                handler(event([("continuation", body.clone())]), restarted.as_ref()).await?
            )
            .is_empty()
        );
        assert_eq!(
            i64::from(MAX_REVISIONS_PER_STREAM) + 1,
            head(&pool, latest.0).await?
        );
        assert_eq!(
            i64::from(MAX_REVISIONS_PER_STREAM) + 1,
            count(&pool, "product_listing_raw_normalizations").await?
        );
        let events = count(&pool, "product_listing_events").await?;
        assert!(
            failure_ids(handler(event([("duplicate", body)]), use_case.as_ref()).await?).is_empty()
        );
        assert_eq!(events, count(&pool, "product_listing_events").await?);
        assert_eq!(
            vec!["capped"],
            capped_failures,
            "the SQS item must remain retryable while a revision is pending"
        );
        Ok(())
    }
    .await;
    assert!(
        result.is_ok(),
        "bounded Lambda continuation failed: {result:?}"
    );
}

async fn wait_for_blocked_normalization(pool: &sqlx::PgPool, blocker_pid: i32) -> TestResult {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let blocked: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE $1 = ANY(pg_blocking_pids(pid)) AND query LIKE '%product_listing_raw_normalizations%'")
                .bind(blocker_pid).fetch_one(pool).await?;
            if blocked > 0 { return Ok::<(), sqlx::Error>(()); }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }).await??;
    Ok(())
}

fn failure_ids(response: SqsBatchResponse) -> Vec<String> {
    response
        .batch_item_failures
        .into_iter()
        .map(|item| item.item_identifier)
        .collect()
}

fn event<const N: usize>(records: [(&str, String); N]) -> LambdaEvent<SqsEvent> {
    let mut payload = SqsEvent::default();
    payload.records = records
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
        .expect("clock after epoch")
        .as_millis() as u64
        + 60_000;
    LambdaEvent::new(payload, context)
}

fn job_body(
    (stream, revision_id, revision): Revision,
) -> Result<String, aura_historia_jobs::WireError> {
    encode(&DomainJob {
        target_queue: WorkerQueue::ProductListingRawNormalization,
        idempotency_key: IdempotencyKey::new(format!("product-listing-raw-revision:{revision_id}")),
        ordering_key: OrderingKey::new(format!("product-listing-raw-stream:{stream}")),
        payload:
            DomainJobPayload::<aura_historia_jobs::SearchFilterOperation>::ProductListingRawRevision(
                ProductListingRawRevisionJob {
                    product_listing_raw_stream_id: stream,
                    product_listing_raw_revision_id: revision_id,
                    revision,
                },
            ),
    })
}

async fn optional_head(
    pool: &sqlx::PgPool,
    stream: ProductListingRawStreamId,
) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar("SELECT last_processed_revision FROM product_listing_raw_normalization_heads WHERE product_listing_raw_stream_id = $1")
        .bind(stream.as_uuid()).fetch_optional(pool).await
}

async fn head(
    pool: &sqlx::PgPool,
    stream: ProductListingRawStreamId,
) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
    optional_head(pool, stream)
        .await?
        .ok_or_else(|| "missing normalization head".into())
}

async fn count(pool: &sqlx::PgPool, table: &str) -> Result<i64, sqlx::Error> {
    let query = match table {
        "product_listing_raw_normalizations" => {
            "SELECT count(*) FROM product_listing_raw_normalizations"
        }
        "product_listing_raw_normalization_heads" => {
            "SELECT count(*) FROM product_listing_raw_normalization_heads"
        }
        "product_listings" => "SELECT count(*) FROM product_listings",
        "product_listing_events" => "SELECT count(*) FROM product_listing_events",
        _ => unreachable!("only known fixture tables may be counted"),
    };
    sqlx::query_scalar(query).fetch_one(pool).await
}

async fn seed_listing_source(pool: &sqlx::PgPool) -> Result<ListingSourceId, sqlx::Error> {
    let party_id = uuid::Uuid::now_v7();
    let source = ListingSourceId::new();
    let slug = format!("raw-lambda-{}", source.as_uuid());
    sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, 'Normalization Lambda test')")
        .bind(party_id).bind(format!("{slug}-party")).execute(pool).await?;
    sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, 'Normalization Lambda source', $3)")
        .bind(source.as_uuid()).bind(slug).bind(party_id).execute(pool).await?;
    Ok(source)
}

async fn capture(
    pool: &sqlx::PgPool,
    write: ProductListingRawCaptureWrite,
) -> Result<Revision, Box<dyn std::error::Error + Send + Sync>> {
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await?;
    let outcome = SqlxProductListingRawCaptureWriterFactory::new()
        .in_transaction(&mut tx)
        .capture(write)
        .await?;
    tx.commit().await?;
    match outcome {
        ProductListingRawCaptureWriteOutcome::Changed {
            product_listing_raw_stream_id,
            product_listing_raw_revision_id,
            revision,
        } => Ok((
            product_listing_raw_stream_id,
            product_listing_raw_revision_id,
            revision,
        )),
        other => Err(format!("expected a changed revision, got {other:?}").into()),
    }
}

fn raw_write(
    source: ListingSourceId,
    key: &str,
    hash_byte: u8,
    euros: u32,
) -> ProductListingRawCaptureWrite {
    let input = ProductListingNormalizationInput::new(
        RawProductListingOperation::Upsert,
        RawProductListingPayloadFormat::WoocommerceProduct,
        1,
        1,
        SourcePayload::new(json!({"retainedUnknown": key})).expect("source payload"),
        RawProductListingValues::new(json!({
            "sourceListingId": "lambda-source-123",
            "priceFormat": "DISPLAY_TEXT",
            "title": {"action": "SET", "value": "An antique ceramic vase from an English collection"},
            "description": {"action": "SET", "value": ["This antique ceramic vase has documented provenance and careful restoration history."]},
            "price": {"action": "SET", "value": format!("EUR {euros}")},
            "priceEstimateMin": {"action": "CLEAR"},
            "priceEstimateMax": {"action": "CLEAR"},
            "availability": {"action": "SET", "value": "in stock"},
            "url": {"action": "SET", "value": "https://example.test/listings/lambda-source-123"},
            "images": {"action": "SET", "value": ["/images/lambda-source-123.jpg"]},
            "attributes": {}
        })).expect("raw values"),
        NormalizationContext::new(json!({
            "baseUrl": "https://example.test/listings/lambda-source-123",
            "fallbackCurrency": "EUR"
        })).expect("normalization context"),
    ).expect("normalization input");
    let input_sha256 = input.hash().expect("normalization input hash");
    ProductListingRawCaptureWrite {
        listing_source_id: source,
        ingestion_method: ProductListingRawIngestionMethod::Woocommerce,
        source_record_key: key.to_owned(),
        source_record_key_sha256: SourceRecordKeySha256::new([hash_byte; 32]),
        input,
        input_sha256,
        provenance: RawProductListingProvenance::new(json!({"deliveryId": key}))
            .expect("provenance"),
        source_event_id: Some(key.to_owned()),
        source_occurred_at: None,
        provider_receipt: None,
    }
}
