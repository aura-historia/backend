use std::time::Duration;

use application::transaction::{Transaction, UnitOfWork};
use listing_source_core::ListingSourceId;
use platform_postgres::SqlxUnitOfWork;
use product_listing_postgres::SqlxProductListingCommandReceiptStoreFactory;
use product_listing_service::{
    ports::{
        ProductListingCommandCompletionCode, ProductListingCommandReceiptError,
        ProductListingCommandReceiptStore, ProductListingCommandReceiptStoreFactory,
        ProductListingCommandReceiptWrite,
    },
    use_cases::commands::product_listing_ingestion::{
        ProductListingIngestionFingerprint, ProductListingIngestionOperation,
    },
};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::OffsetDateTime;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
const COMMAND_ID: &str = "plic1_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SUBMISSION_ID: &str =
    "plis1_bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn sample() -> ProductListingCommandReceiptWrite {
    ProductListingCommandReceiptWrite {
        command_id: COMMAND_ID.into(),
        submission_id: SUBMISSION_ID.into(),
        listing_source_id: ListingSourceId::new(),
        operation: ProductListingIngestionOperation::CaptureRaw,
        fingerprint: ProductListingIngestionFingerprint::from_digest([42; 32]),
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn receipt_is_atomic_immutable_and_retained_without_business_rows() {
    let pool = get_postgres_client().await;
    let uow = SqlxUnitOfWork::new(pool.clone());
    let factory = SqlxProductListingCommandReceiptStoreFactory::new();
    let receipt = sample();

    let mut tx = uow.begin().await.unwrap();
    assert!(
        factory
            .in_transaction(&mut tx)
            .lock_and_find(COMMAND_ID)
            .await
            .unwrap()
            .is_none()
    );
    factory
        .in_transaction(&mut tx)
        .insert(&receipt)
        .await
        .unwrap();
    drop(tx);
    assert!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM product_listing_command_receipts")
            .fetch_one(&pool)
            .await
            .unwrap()
            == 0
    );

    let started_at = OffsetDateTime::now_utc();
    let mut tx = uow.begin().await.unwrap();
    factory
        .in_transaction(&mut tx)
        .insert(&receipt)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let finished_at = OffsetDateTime::now_utc();

    let mut tx = uow.begin().await.unwrap();
    let read = factory
        .in_transaction(&mut tx)
        .lock_and_find(COMMAND_ID)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(read.submission_id, SUBMISSION_ID);
    assert_eq!(read.listing_source_id, receipt.listing_source_id);
    assert_eq!(read.operation, receipt.operation);
    assert_eq!(read.fingerprint, receipt.fingerprint);
    assert_eq!(
        read.completion_code,
        ProductListingCommandCompletionCode::Applied
    );
    assert!(read.completed_at >= started_at && read.completed_at <= finished_at);
    assert!(matches!(
        factory.in_transaction(&mut tx).insert(&receipt).await,
        Err(ProductListingCommandReceiptError::AlreadyExists)
    ));
    drop(tx);

    let (fingerprint, code, completed_at): (Vec<u8>, String, OffsetDateTime) = sqlx::query_as(
        "SELECT semantic_fingerprint, completion_code, completed_at FROM product_listing_command_receipts WHERE command_id = $1",
    )
    .bind(COMMAND_ID).fetch_one(&pool).await.unwrap();
    assert_eq!(fingerprint, vec![42; 32]);
    assert_eq!(code, "APPLIED");
    assert_eq!(completed_at, read.completed_at);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn advisory_lock_waits_for_commit_and_reads_committed_receipt() {
    let pool = get_postgres_client().await;
    let uow = SqlxUnitOfWork::new(pool.clone());
    let factory = SqlxProductListingCommandReceiptStoreFactory::new();
    let receipt = sample();
    let mut first = uow.begin().await.unwrap();
    assert!(
        factory
            .in_transaction(&mut first)
            .lock_and_find(COMMAND_ID)
            .await
            .unwrap()
            .is_none()
    );
    factory
        .in_transaction(&mut first)
        .insert(&receipt)
        .await
        .unwrap();

    let (attempting, attempted) = tokio::sync::oneshot::channel();
    let waiter = tokio::spawn(async move {
        let mut second = uow.begin().await.unwrap();
        attempting.send(()).unwrap();
        let result = factory
            .in_transaction(&mut second)
            .lock_and_find(COMMAND_ID)
            .await
            .unwrap();
        second.commit().await.unwrap();
        result
    });
    attempted.await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !waiter.is_finished(),
        "second lookup must wait for first transaction"
    );
    first.commit().await.unwrap();
    let read = tokio::time::timeout(Duration::from_secs(5), waiter)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        read.completion_code,
        ProductListingCommandCompletionCode::Applied
    );
    assert_eq!(read.fingerprint, receipt.fingerprint);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn invalid_identity_is_rejected_before_sql_and_schema_bounds_persisted_data() {
    let pool = get_postgres_client().await;
    let uow = SqlxUnitOfWork::new(pool.clone());
    let factory = SqlxProductListingCommandReceiptStoreFactory::new();
    let mut tx = uow.begin().await.unwrap();
    assert!(matches!(
        factory
            .in_transaction(&mut tx)
            .lock_and_find("plic1_invalid")
            .await,
        Err(ProductListingCommandReceiptError::InvalidIdentity)
    ));
    let mut receipt = sample();
    receipt.submission_id = "plis1_invalid".into();
    assert!(matches!(
        factory.in_transaction(&mut tx).insert(&receipt).await,
        Err(ProductListingCommandReceiptError::InvalidIdentity)
    ));
    drop(tx);

    let invalid = sqlx::query(
        "INSERT INTO product_listing_command_receipts (command_id, submission_id, listing_source_id, operation, semantic_fingerprint, completion_code) VALUES ($1, $2, $3, $4, $5, $6)",
    ).bind(COMMAND_ID).bind(SUBMISSION_ID).bind(receipt.listing_source_id.as_uuid())
    .bind("CAPTURE_RAW").bind(vec![0; 31]).bind("APPLIED").execute(&pool).await;
    assert!(
        invalid.is_err(),
        "schema must reject truncated fingerprints"
    );
    let rejected = sqlx::query(
        "INSERT INTO product_listing_command_receipts (command_id, submission_id, listing_source_id, operation, semantic_fingerprint, completion_code) VALUES ($1, $2, $3, $4, $5, $6)",
    ).bind(COMMAND_ID).bind(SUBMISSION_ID).bind(receipt.listing_source_id.as_uuid())
    .bind("CAPTURE_RAW").bind(vec![0; 32]).bind("REJECTED").execute(&pool).await;
    assert!(rejected.is_err(), "schema must forbid rejected receipts");
}
