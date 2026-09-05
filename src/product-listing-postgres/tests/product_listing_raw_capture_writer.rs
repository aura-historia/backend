use application::transaction::{Transaction, UnitOfWork};
use listing_source_core::ListingSourceId;
use platform_postgres::SqlxUnitOfWork;
use product_listing_normalization::{
    NormalizationContext, ProductListingNormalizationInput, RawProductListingOperation,
    RawProductListingPayloadFormat, RawProductListingProvenance, RawProductListingValues,
    SourcePayload,
};
use product_listing_postgres::SqlxProductListingRawCaptureWriterFactory;
use product_listing_service::ports::{
    ProductListingRawCaptureWrite, ProductListingRawCaptureWriteOutcome,
    ProductListingRawCaptureWriter, ProductListingRawCaptureWriterFactory,
    ProductListingRawIngestionMethod, SourceRecordKeySha256,
};
use serde_json::{Value, json};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_append_only_material_raw_input_changes() {
    let pool = get_postgres_client().await;
    let listing_source_id = seed_listing_source(&pool, "raw-capture-source").await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let factory = SqlxProductListingRawCaptureWriterFactory::new();

    let first = capture(
        &unit_of_work,
        &factory,
        write(
            listing_source_id,
            json!({"unknown": {"number": 42, "array": [true, null]}}),
            json!({"title": "Vase"}),
            json!({"baseUrl": "https://example.test"}),
            "delivery-1",
        ),
    )
    .await;
    let unchanged = capture(
        &unit_of_work,
        &factory,
        write(
            listing_source_id,
            json!({"unknown": {"array": [true, null], "number": 42}}),
            json!({"title": "Vase"}),
            json!({"baseUrl": "https://example.test"}),
            "delivery-2",
        ),
    )
    .await;
    let second = capture(
        &unit_of_work,
        &factory,
        write(
            listing_source_id,
            json!({"unknown": "changed"}),
            json!({"title": "Vase"}),
            json!({"baseUrl": "https://example.test"}),
            "delivery-3",
        ),
    )
    .await;
    let third = capture(
        &unit_of_work,
        &factory,
        write(
            listing_source_id,
            json!({"unknown": {"number": 42, "array": [true, null]}}),
            json!({"title": "Vase"}),
            json!({"baseUrl": "https://example.test"}),
            "delivery-4",
        ),
    )
    .await;

    assert!(matches!(
        first,
        ProductListingRawCaptureWriteOutcome::Changed { revision: 1, .. }
    ));
    assert!(matches!(
        unchanged,
        ProductListingRawCaptureWriteOutcome::Unchanged {
            latest_revision: 1,
            ..
        }
    ));
    assert!(matches!(
        second,
        ProductListingRawCaptureWriteOutcome::Changed { revision: 2, .. }
    ));
    assert!(matches!(
        third,
        ProductListingRawCaptureWriteOutcome::Changed { revision: 3, .. }
    ));

    let revisions: Vec<(i64, Value, Value, Value, String)> = sqlx::query_as(
        "SELECT revision, source_payload, raw_values, normalization_context, source_event_id FROM product_listing_raw_revisions ORDER BY revision",
    )
    .fetch_all(&pool)
    .await
    .unwrap_or_else(|error| panic!("read raw revisions: {error}"));
    assert_eq!(3, revisions.len());
    assert_eq!(
        json!({"unknown": {"number": 42, "array": [true, null]}}),
        revisions[0].1
    );
    assert_eq!(json!({"title": "Vase"}), revisions[0].2);
    assert_eq!(json!({"baseUrl": "https://example.test"}), revisions[0].3);
    assert_eq!("delivery-1", revisions[0].4);

    let product_listing_count: i64 = sqlx::query_scalar("SELECT count(*) FROM product_listings")
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("count product listings: {error}"));
    let event_count: i64 = sqlx::query_scalar("SELECT count(*) FROM product_listing_events")
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("count product listing events: {error}"));
    assert_eq!(0, product_listing_count);
    assert_eq!(0, event_count);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_serialize_concurrent_equal_captures_into_one_revision() {
    let pool = get_postgres_client().await;
    let listing_source_id = seed_listing_source(&pool, "raw-capture-concurrent-source").await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let factory = SqlxProductListingRawCaptureWriterFactory::new();

    let (first, second) = tokio::join!(
        capture(
            &unit_of_work,
            &factory,
            write(
                listing_source_id,
                json!({"title": "same"}),
                json!({}),
                json!({}),
                "delivery-1"
            ),
        ),
        capture(
            &unit_of_work,
            &factory,
            write(
                listing_source_id,
                json!({"title": "same"}),
                json!({}),
                json!({}),
                "delivery-2"
            ),
        )
    );
    let outcomes = [first, second];
    let changed_count = outcomes
        .iter()
        .filter(|outcome| {
            matches!(
                outcome,
                ProductListingRawCaptureWriteOutcome::Changed { .. }
            )
        })
        .count();
    let unchanged_count = outcomes
        .iter()
        .filter(|outcome| {
            matches!(
                outcome,
                ProductListingRawCaptureWriteOutcome::Unchanged { .. }
            )
        })
        .count();
    assert_eq!(1, changed_count);
    assert_eq!(1, unchanged_count);

    let revision_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM product_listing_raw_revisions")
            .fetch_one(&pool)
            .await
            .unwrap_or_else(|error| panic!("count raw revisions: {error}"));
    assert_eq!(1, revision_count);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_serialize_concurrent_different_captures_into_ordered_revisions() {
    let pool = get_postgres_client().await;
    let listing_source_id =
        seed_listing_source(&pool, "raw-capture-concurrent-different-source").await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let factory = SqlxProductListingRawCaptureWriterFactory::new();

    let (first, second) = tokio::join!(
        capture(
            &unit_of_work,
            &factory,
            write(
                listing_source_id,
                json!({"title": "first"}),
                json!({}),
                json!({}),
                "delivery-1"
            ),
        ),
        capture(
            &unit_of_work,
            &factory,
            write(
                listing_source_id,
                json!({"title": "second"}),
                json!({}),
                json!({}),
                "delivery-2"
            ),
        )
    );
    let revisions = [first, second]
        .into_iter()
        .map(|outcome| match outcome {
            ProductListingRawCaptureWriteOutcome::Changed { revision, .. } => revision,
            ProductListingRawCaptureWriteOutcome::Unchanged { .. } => {
                panic!("different inputs must create revisions")
            }
        })
        .collect::<Vec<_>>();
    assert!(revisions.contains(&1));
    assert!(revisions.contains(&2));

    let stored_revisions: Vec<i64> =
        sqlx::query_scalar("SELECT revision FROM product_listing_raw_revisions ORDER BY revision")
            .fetch_all(&pool)
            .await
            .unwrap_or_else(|error| panic!("read raw revisions: {error}"));
    assert_eq!(vec![1, 2], stored_revisions);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_detect_source_record_key_hash_collision() {
    let pool = get_postgres_client().await;
    let listing_source_id = seed_listing_source(&pool, "raw-capture-collision-source").await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let factory = SqlxProductListingRawCaptureWriterFactory::new();

    let first = capture(
        &unit_of_work,
        &factory,
        write(
            listing_source_id,
            json!({}),
            json!({}),
            json!({}),
            "delivery-1",
        ),
    )
    .await;
    assert!(matches!(
        first,
        ProductListingRawCaptureWriteOutcome::Changed { .. }
    ));

    let mut collision = write(
        listing_source_id,
        json!({"changed": true}),
        json!({}),
        json!({}),
        "delivery-2",
    );
    collision.source_record_key = "https://example.test/other".to_owned();
    let result = capture_result(&unit_of_work, &factory, collision).await;
    assert!(matches!(
        result,
        Err(product_listing_service::ports::ProductListingRawCaptureWriteError::SourceRecordKeyHashCollision)
    ));
}

fn write(
    listing_source_id: ListingSourceId,
    source_payload: Value,
    raw_values: Value,
    context: Value,
    delivery_id: &str,
) -> ProductListingRawCaptureWrite {
    let input = ProductListingNormalizationInput::new(
        RawProductListingOperation::Upsert,
        RawProductListingPayloadFormat::ShopifyProduct,
        1,
        1,
        SourcePayload::new(source_payload)
            .unwrap_or_else(|error| panic!("source payload: {error}")),
        RawProductListingValues::new(raw_values)
            .unwrap_or_else(|error| panic!("raw values: {error}")),
        NormalizationContext::new(context).unwrap_or_else(|error| panic!("context: {error}")),
    )
    .unwrap_or_else(|error| panic!("normalization input: {error}"));
    let input_sha256 = input
        .hash()
        .unwrap_or_else(|error| panic!("input hash: {error}"));
    ProductListingRawCaptureWrite {
        listing_source_id,
        ingestion_method: ProductListingRawIngestionMethod::Shopify,
        source_record_key: "123".to_owned(),
        source_record_key_sha256: SourceRecordKeySha256::new([7; 32]),
        input,
        input_sha256,
        provenance: RawProductListingProvenance::new(json!({"deliveryId": delivery_id}))
            .unwrap_or_else(|error| panic!("provenance: {error}")),
        source_event_id: Some(delivery_id.to_owned()),
        source_occurred_at: None,
    }
}

async fn capture(
    unit_of_work: &SqlxUnitOfWork,
    factory: &SqlxProductListingRawCaptureWriterFactory,
    write: ProductListingRawCaptureWrite,
) -> ProductListingRawCaptureWriteOutcome {
    capture_result(unit_of_work, factory, write)
        .await
        .unwrap_or_else(|error| panic!("capture raw input: {error}"))
}

async fn capture_result(
    unit_of_work: &SqlxUnitOfWork,
    factory: &SqlxProductListingRawCaptureWriterFactory,
    write: ProductListingRawCaptureWrite,
) -> Result<
    ProductListingRawCaptureWriteOutcome,
    product_listing_service::ports::ProductListingRawCaptureWriteError,
> {
    let mut tx = unit_of_work
        .begin()
        .await
        .unwrap_or_else(|error| panic!("begin transaction: {error}"));
    let outcome = factory.in_transaction(&mut tx).capture(write).await;
    match outcome {
        Ok(outcome) => {
            tx.commit()
                .await
                .unwrap_or_else(|error| panic!("commit transaction: {error}"));
            Ok(outcome)
        }
        Err(error) => Err(error),
    }
}

async fn seed_listing_source(pool: &sqlx::PgPool, slug: &str) -> ListingSourceId {
    let party_id = uuid::Uuid::new_v4();
    let listing_source_id = ListingSourceId::new();
    sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
        .bind(party_id)
        .bind(format!("{slug}-party"))
        .bind(format!("{slug} party"))
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("seed party: {error}"));
    sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, $3, $4)")
        .bind(uuid::Uuid::from(listing_source_id))
        .bind(slug)
        .bind(slug)
        .bind(party_id)
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("seed listing source: {error}"));
    listing_source_id
}
