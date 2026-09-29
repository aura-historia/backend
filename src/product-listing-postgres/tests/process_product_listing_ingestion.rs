use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use application::{
    operation_context::{
        CorrelationId, CredentialCapability, OperationContext, Principal, RequestId,
    },
    patch_field::PatchField,
};
use auction_core::AuctionId;
use auction_postgres::SqlxAuctionReferenceValidatorFactory;
use domain_primitives::{change_outcome::ChangeOutcome, event_id::EventId};
use listing_source_core::ListingSourceId;
use platform_postgres::{SqlxTransaction, SqlxUnitOfWork};
use product_listing_core::{
    product_listing::ProductListing,
    product_listing_id::{ProductListingId, ProductListingKey},
    source_listing_id::SourceListingId,
};
use product_listing_ingestion_sqs::codec;
use product_listing_normalization::{
    NormalizationContext, ProductListingNormalizationInput, RawProductListingOperation,
    RawProductListingPayloadFormat, RawProductListingProvenance, RawProductListingValues,
    SourcePayload,
};
use product_listing_postgres::{
    SqlxPartnerProductListingAuthorizerFactory, SqlxProductListingCommandReceiptStoreFactory,
    SqlxProductListingEventAppenderFactory, SqlxProductListingRawCaptureWriterFactory,
    SqlxProductListingRepositoryFactory,
};
use product_listing_service::{
    ports::{
        ProductListingCommandReceipt, ProductListingCommandReceiptError,
        ProductListingCommandReceiptStore, ProductListingCommandReceiptStoreFactory,
        ProductListingCommandReceiptWrite, ProductListingIngestionPublishError,
        ProductListingIngestionPublisher, ProductListingRawIngestionMethod,
        ProductListingRawProviderReceipt, ProductListingRepository, ProductListingRepositoryError,
        ProductListingRepositoryFactory, ProductListingStorageVersion, ProductListingWriteEffects,
        ProviderReceiptScope, SourceEvidenceSha256, VersionedProductListing,
    },
    product_listing_auction_patch::ProductListingAuctionPatch,
    use_cases::{
        CaptureProductListingRawObservationCommand, CaptureProductListingRawObservationError,
        CaptureProductListingRawObservationResult, CreateProductListingCommand,
        CreateProductListingError, CreateProductListingHandler, CreateProductListingUseCase,
        IndexedProductListingIngestionIntent, ProcessProductListingIngestionHandler,
        ProcessProductListingIngestionUseCase, ProductListingIngestionCompletion,
        ProductListingIngestionEffect, ProductListingIngestionEnvelope,
        ProductListingIngestionError, ProductListingIngestionFingerprint,
        ProductListingIngestionIdempotencyKey, ProductListingIngestionIntent,
        ProductListingIngestionItemOutcome, ProductListingIngestionMessage,
        ProductListingIngestionOutcome, ProductListingIngestionSubmission,
        SubmitInternalProductListingIngestionHandler, SubmitInternalProductListingIngestionUseCase,
        SubmitPartnerProductListingIngestionHandler, SubmitPartnerProductListingIngestionUseCase,
        UpdateProductListingCommand, UpdateProductListingError, UpsertProductListingCommand,
        UpsertProductListingResult, WithdrawProductListingError,
    },
};
use serde_json::json;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::OffsetDateTime;
use user_core::user_id::UserId;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[derive(Default, Clone)]
struct CapturingPublisher(Arc<Mutex<Vec<ProductListingIngestionMessage>>>);

#[async_trait::async_trait]
impl ProductListingIngestionPublisher for CapturingPublisher {
    async fn publish(
        &self,
        commands: Vec<ProductListingIngestionMessage>,
    ) -> Result<Vec<ProductListingIngestionItemOutcome>, ProductListingIngestionPublishError> {
        let outcomes = commands
            .iter()
            .map(|command| ProductListingIngestionItemOutcome {
                index: command.metadata.index,
                command_id: command.metadata.command_id.clone(),
                outcome: ProductListingIngestionOutcome::Accepted,
            })
            .collect();
        self.0.lock().unwrap().extend(commands);
        Ok(outcomes)
    }
}

fn handler(pool: &sqlx::PgPool) -> impl ProcessProductListingIngestionUseCase {
    ProcessProductListingIngestionHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxProductListingRepositoryFactory::new(),
        SqlxProductListingEventAppenderFactory::new(),
        SqlxPartnerProductListingAuthorizerFactory::new(),
        SqlxAuctionReferenceValidatorFactory::new(),
        SqlxProductListingRawCaptureWriterFactory::new(),
        SqlxProductListingCommandReceiptStoreFactory::new(),
    )
}

// Only the first insert is intercepted; every other operation uses the real SQLx adapters.
#[derive(Clone)]
#[allow(clippy::large_enum_variant)]
enum InsertInterception {
    SlugCollision,
    CompetingCreate {
        pool: sqlx::PgPool,
        command: CreateProductListingCommand,
        competing_id: Arc<Mutex<Option<ProductListingId>>>,
    },
}

#[derive(Clone)]
struct RacingRepositoryFactory {
    inner: SqlxProductListingRepositoryFactory,
    interception: InsertInterception,
    fired: Arc<AtomicBool>,
}

struct RacingRepository<R> {
    inner: R,
    interception: InsertInterception,
    fired: Arc<AtomicBool>,
}

impl ProductListingRepositoryFactory<SqlxTransaction> for RacingRepositoryFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl ProductListingRepository + 'tx {
        RacingRepository {
            inner: self.inner.in_transaction(tx),
            interception: self.interception.clone(),
            fired: Arc::clone(&self.fired),
        }
    }
}

#[async_trait::async_trait]
impl<R: ProductListingRepository> ProductListingRepository for RacingRepository<R> {
    async fn find_by_id(
        &mut self,
        id: ProductListingId,
    ) -> Result<Option<VersionedProductListing>, ProductListingRepositoryError> {
        self.inner.find_by_id(id).await
    }

    async fn find_by_key(
        &mut self,
        key: &ProductListingKey,
    ) -> Result<Option<VersionedProductListing>, ProductListingRepositoryError> {
        self.inner.find_by_key(key).await
    }

    async fn insert(
        &mut self,
        product: &ProductListing,
        current_event_id: EventId,
    ) -> Result<VersionedProductListing, ProductListingRepositoryError> {
        if !self.fired.swap(true, Ordering::SeqCst) {
            match &self.interception {
                InsertInterception::SlugCollision => {
                    return Err(
                        ProductListingRepositoryError::ProductListingTitleSlugAlreadyExists,
                    );
                }
                InsertInterception::CompetingCreate {
                    pool,
                    command,
                    competing_id,
                } => {
                    // Commit on another connection after UPSERT has read the absent key.
                    // Its real insert now encounters PostgreSQL's source-key unique constraint.
                    let competitor = CreateProductListingHandler::new(
                        SqlxUnitOfWork::new(pool.clone()),
                        SqlxProductListingRepositoryFactory::new(),
                        SqlxProductListingEventAppenderFactory::new(),
                        SqlxPartnerProductListingAuthorizerFactory::new(),
                        SqlxAuctionReferenceValidatorFactory::new(),
                    );
                    let result = competitor
                        .execute(&context(Principal::System), command.clone())
                        .await
                        .unwrap();
                    *competing_id.lock().unwrap() = Some(result.product_listing_id);
                }
            }
        }
        self.inner.insert(product, current_event_id).await
    }

    async fn update(
        &mut self,
        product: &ProductListing,
        expected_version: ProductListingStorageVersion,
        current_event_id: EventId,
        effects: ProductListingWriteEffects,
    ) -> Result<VersionedProductListing, ProductListingRepositoryError> {
        self.inner
            .update(product, expected_version, current_event_id, effects)
            .await
    }
}

#[derive(Clone, Default)]
struct TracingReceiptFactory {
    // A new txid means PostgreSQL began a new transaction, even if the pool reused a connection.
    lookups: Arc<Mutex<Vec<(i64, bool)>>>,
}

struct TracingReceiptStore<'tx> {
    tx: &'tx mut SqlxTransaction,
    lookups: Arc<Mutex<Vec<(i64, bool)>>>,
}

impl ProductListingCommandReceiptStoreFactory<SqlxTransaction> for TracingReceiptFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl ProductListingCommandReceiptStore + 'tx {
        TracingReceiptStore {
            tx,
            lookups: Arc::clone(&self.lookups),
        }
    }
}

#[async_trait::async_trait]
impl ProductListingCommandReceiptStore for TracingReceiptStore<'_> {
    async fn lock_and_find(
        &mut self,
        command_id: &str,
    ) -> Result<Option<ProductListingCommandReceipt>, ProductListingCommandReceiptError> {
        let txid: i64 = sqlx::query_scalar("SELECT txid_current()")
            .fetch_one(self.tx.connection())
            .await
            .unwrap();
        let factory = SqlxProductListingCommandReceiptStoreFactory::new();
        let found = factory
            .in_transaction(self.tx)
            .lock_and_find(command_id)
            .await?;
        self.lookups.lock().unwrap().push((txid, found.is_some()));
        Ok(found)
    }

    async fn insert(
        &mut self,
        receipt: &ProductListingCommandReceiptWrite,
    ) -> Result<(), ProductListingCommandReceiptError> {
        SqlxProductListingCommandReceiptStoreFactory::new()
            .in_transaction(self.tx)
            .insert(receipt)
            .await
    }
}

fn racing_handler(
    pool: &sqlx::PgPool,
    repository: RacingRepositoryFactory,
    receipts: TracingReceiptFactory,
) -> impl ProcessProductListingIngestionUseCase {
    ProcessProductListingIngestionHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        repository,
        SqlxProductListingEventAppenderFactory::new(),
        SqlxPartnerProductListingAuthorizerFactory::new(),
        SqlxAuctionReferenceValidatorFactory::new(),
        SqlxProductListingRawCaptureWriterFactory::new(),
        receipts,
    )
}

fn context(principal: Principal) -> OperationContext {
    OperationContext {
        principal,
        request_id: RequestId::new("ingestion-integration-request"),
        correlation_id: CorrelationId::new("ingestion-integration-correlation"),
    }
}

fn create(source: ListingSourceId, key: &str) -> ProductListingIngestionIntent {
    ProductListingIngestionIntent::Create(CreateProductListingCommand {
        listing_source_id: source,
        source_listing_id: SourceListingId::try_from(key).unwrap(),
        title: None,
        description: None,
        pricing: Default::default(),
        availability: None,
        url: url::Url::parse(&format!("https://example.test/listings/{key}")).unwrap(),
        images: Default::default(),
        auction: None,
    })
}

fn update(source: ListingSourceId, key: &str) -> ProductListingIngestionIntent {
    ProductListingIngestionIntent::Update {
        product_key: product_key(source, key),
        command: UpdateProductListingCommand::default(),
    }
}

fn product_key(source: ListingSourceId, key: &str) -> ProductListingKey {
    ProductListingKey::new(source, SourceListingId::try_from(key).unwrap())
}

fn url(value: &str) -> url::Url {
    url::Url::parse(value).unwrap()
}

fn update_url(source: ListingSourceId, key: &str, value: &str) -> ProductListingIngestionIntent {
    let ProductListingIngestionIntent::Update {
        product_key,
        mut command,
    } = update(source, key)
    else {
        unreachable!()
    };
    command.url = PatchField::Set(url(value));
    ProductListingIngestionIntent::Update {
        product_key,
        command,
    }
}

fn upsert(source: ListingSourceId, key: &str, value: &str) -> ProductListingIngestionIntent {
    ProductListingIngestionIntent::Upsert(UpsertProductListingCommand {
        listing_source_id: source,
        source_listing_id: SourceListingId::try_from(key).unwrap(),
        title: None,
        description: None,
        price: PatchField::Unchanged,
        price_estimate_min: PatchField::Unchanged,
        price_estimate_max: PatchField::Unchanged,
        availability: PatchField::Unchanged,
        url: Some(url(value)),
        images: PatchField::Unchanged,
        auction: PatchField::Unchanged,
    })
}

fn raw_capture(
    source: ListingSourceId,
    state: &str,
    delivery_id: &str,
    occurred_at: i64,
) -> ProductListingIngestionIntent {
    let source_payload = SourcePayload::new(json!({ "state": state })).unwrap();
    let evidence = source_payload.canonical_sha256().unwrap();
    let input = ProductListingNormalizationInput::new(
        RawProductListingOperation::Upsert,
        RawProductListingPayloadFormat::ShopifyProduct,
        1,
        1,
        source_payload,
        RawProductListingValues::new(json!({})).unwrap(),
        NormalizationContext::new(json!({})).unwrap(),
    )
    .unwrap();
    ProductListingIngestionIntent::CaptureRaw(CaptureProductListingRawObservationCommand {
        listing_source_id: source,
        ingestion_method: ProductListingRawIngestionMethod::Shopify,
        source_record_key: "provider-product-1".into(),
        input,
        provenance: RawProductListingProvenance::new(json!({ "deliveryId": delivery_id })).unwrap(),
        source_event_id: Some(delivery_id.into()),
        source_occurred_at: Some(OffsetDateTime::from_unix_timestamp(occurred_at).unwrap()),
        provider_receipt: Some(
            ProductListingRawProviderReceipt::new(
                ProviderReceiptScope::new("test-provider:products".into()).unwrap(),
                delivery_id.into(),
                SourceEvidenceSha256::new(*evidence.as_bytes()),
            )
            .unwrap(),
        ),
    })
}

fn woocommerce_raw_capture(
    source: ListingSourceId,
    title: &str,
    delivery_id: &str,
    occurred_at: i64,
) -> ProductListingIngestionIntent {
    let timestamp = OffsetDateTime::from_unix_timestamp(occurred_at).unwrap();
    let source_payload = SourcePayload::new(json!({
        "id": 123,
        "status": "publish",
        "name": title,
        "permalink": "https://example.test/products/123",
        "date_modified_gmt": timestamp.format(&time::format_description::well_known::Rfc3339).unwrap(),
    }))
    .unwrap();
    let evidence = source_payload.canonical_sha256().unwrap();
    let input = ProductListingNormalizationInput::new(
        RawProductListingOperation::Upsert,
        RawProductListingPayloadFormat::WoocommerceProduct,
        1,
        1,
        source_payload,
        RawProductListingValues::new(json!({
            "sourceListingId": "123",
            "priceFormat": "MACHINE_DECIMAL",
            "title": {"action": "SET", "value": title},
            "description": {"action": "CLEAR"},
            "price": {"action": "CLEAR"},
            "priceEstimateMin": {"action": "UNCHANGED"},
            "priceEstimateMax": {"action": "UNCHANGED"},
            "availability": {"action": "UNCHANGED"},
            "url": {"action": "SET", "value": "https://example.test/products/123"},
            "images": {"action": "SET", "value": []},
        }))
        .unwrap(),
        NormalizationContext::new(json!({
            "baseUrl": "https://example.test/products/123",
            "fallbackCurrency": "USD",
            "fallbackLanguage": "en",
        }))
        .unwrap(),
    )
    .unwrap();
    ProductListingIngestionIntent::CaptureRaw(CaptureProductListingRawObservationCommand {
        listing_source_id: source,
        ingestion_method: ProductListingRawIngestionMethod::Woocommerce,
        source_record_key: "123".into(),
        input,
        provenance: RawProductListingProvenance::new(json!({
            "topic": "product.updated",
            "deliveryId": delivery_id,
        }))
        .unwrap(),
        source_event_id: Some(delivery_id.into()),
        source_occurred_at: Some(timestamp),
        provider_receipt: Some(
            ProductListingRawProviderReceipt::new(
                ProviderReceiptScope::new("product.updated".into()).unwrap(),
                delivery_id.into(),
                SourceEvidenceSha256::new(*evidence.as_bytes()),
            )
            .unwrap(),
        ),
    })
}

fn verified(envelope: ProductListingIngestionEnvelope) -> ProductListingIngestionEnvelope {
    codec::decode(&codec::encode(&envelope.message).unwrap())
        .unwrap()
        .into_service_envelope()
        .unwrap()
}

async fn prepared(
    source: ListingSourceId,
    principal: Principal,
    idempotency_key: &str,
    intents: Vec<ProductListingIngestionIntent>,
) -> Vec<ProductListingIngestionEnvelope> {
    let publisher = CapturingPublisher::default();
    let submission = ProductListingIngestionSubmission {
        listing_source_id: source,
        original_input_count: intents.len(),
        idempotency_key: Some(ProductListingIngestionIdempotencyKey::new(idempotency_key).unwrap()),
        items: intents
            .into_iter()
            .enumerate()
            .map(|(index, intent)| IndexedProductListingIngestionIntent { index, intent })
            .collect(),
    };
    match &principal {
        Principal::User(_) | Principal::DelegatedUser { .. } => {
            SubmitPartnerProductListingIngestionHandler::new(publisher.clone())
                .execute(&context(principal), submission)
                .await
                .unwrap();
        }
        Principal::System | Principal::Service(_) => {
            SubmitInternalProductListingIngestionHandler::new(publisher.clone())
                .execute(&context(principal), submission)
                .await
                .unwrap();
        }
        Principal::Anonymous => panic!("test requires an authenticated actor"),
    }
    publisher
        .0
        .lock()
        .unwrap()
        .drain(..)
        .map(|message| ProductListingIngestionEnvelope {
            message,
            fingerprint: ProductListingIngestionFingerprint::from_digest([17; 32]),
        })
        .collect()
}

async fn seed_source(pool: &sqlx::PgPool) -> ListingSourceId {
    let party = uuid::Uuid::now_v7();
    let source = ListingSourceId::new();
    sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
        .bind(party)
        .bind(format!("ingestion-{party}"))
        .bind("Ingestion integration test")
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, $3, $4)")
        .bind(source.into_uuid())
        .bind(format!("ingestion-{}", source.into_uuid().simple()))
        .bind("Ingestion integration source")
        .bind(party)
        .execute(pool)
        .await
        .unwrap();
    source
}

async fn seed_partner(pool: &sqlx::PgPool, source: ListingSourceId) -> (UserId, uuid::Uuid) {
    let actor = UserId::new();
    sqlx::query("INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'FREE', 'USER')")
        .bind(actor.into_uuid())
        .bind(format!("{actor}@example.test"))
        .execute(pool)
        .await
        .unwrap();
    let party: uuid::Uuid = sqlx::query_scalar(
        "SELECT operator_party_id FROM listing_sources WHERE listing_source_id = $1",
    )
    .bind(source.as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    let partnership = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO partnerships (partnership_id, party_id) VALUES ($1, $2)")
        .bind(partnership)
        .bind(party)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO partnership_members (user_id, partnership_id) VALUES ($1, $2)")
        .bind(actor.into_uuid())
        .bind(partnership)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO partnership_listing_source_grants (partnership_id, listing_source_id) VALUES ($1, $2)")
        .bind(partnership)
        .bind(source.into_uuid())
        .execute(pool)
        .await
        .unwrap();
    (actor, partnership)
}

async fn raw_counts(pool: &sqlx::PgPool) -> (i64, i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM product_listing_raw_streams), \
                (SELECT count(*) FROM product_listing_raw_revisions), \
                (SELECT count(*) FROM product_listing_raw_provider_observation_receipts)",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn counts(pool: &sqlx::PgPool) -> (i64, i64, i64) {
    let products = sqlx::query_scalar("SELECT count(*) FROM product_listings")
        .fetch_one(pool)
        .await
        .unwrap();
    let events = sqlx::query_scalar("SELECT count(*) FROM product_listing_events")
        .fetch_one(pool)
        .await
        .unwrap();
    let receipts = sqlx::query_scalar("SELECT count(*) FROM product_listing_command_receipts")
        .fetch_one(pool)
        .await
        .unwrap();
    (products, events, receipts)
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn duplicate_create_returns_already_completed_without_second_effect() {
    let pool = get_postgres_client().await;
    let source = seed_source(&pool).await;
    let envelope = prepared(
        source,
        Principal::System,
        "duplicate",
        vec![create(source, "one")],
    )
    .await
    .remove(0);
    let processor = handler(&pool);
    let first = processor.execute(envelope.clone()).await.unwrap();
    let ProductListingIngestionCompletion::Applied(ProductListingIngestionEffect::Created(created)) =
        first
    else {
        panic!("first delivery must create the listing");
    };
    assert_eq!(counts(&pool).await, (1, 1, 1));
    assert_eq!(
        processor.execute(envelope).await.unwrap(),
        ProductListingIngestionCompletion::AlreadyCompleted
    );
    assert_eq!(counts(&pool).await, (1, 1, 1));
    let persisted: uuid::Uuid = sqlx::query_scalar(
        "SELECT product_listing_id FROM product_listings WHERE source_listing_id = 'one'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(persisted, created.product_listing_id.into_uuid());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn concurrent_deliveries_commit_one_listing_event_and_receipt() {
    let pool = get_postgres_client().await;
    let source = seed_source(&pool).await;
    let envelope = prepared(
        source,
        Principal::System,
        "concurrent",
        vec![create(source, "one")],
    )
    .await
    .remove(0);
    let processor = handler(&pool);
    let (left, right) = tokio::join!(
        processor.execute(envelope.clone()),
        processor.execute(envelope)
    );
    let completions = [left.unwrap(), right.unwrap()];
    assert_eq!(
        completions
            .iter()
            .filter(|result| matches!(
                result,
                ProductListingIngestionCompletion::Applied(ProductListingIngestionEffect::Created(
                    _
                ))
            ))
            .count(),
        1
    );
    assert_eq!(
        completions
            .iter()
            .filter(|result| **result == ProductListingIngestionCompletion::AlreadyCompleted)
            .count(),
        1
    );
    assert_eq!(counts(&pool).await, (1, 1, 1));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn reused_command_identity_with_different_fingerprint_is_rejected_without_writes() {
    let pool = get_postgres_client().await;
    let source = seed_source(&pool).await;
    let envelope = prepared(
        source,
        Principal::System,
        "conflict",
        vec![create(source, "one")],
    )
    .await
    .remove(0);
    let processor = handler(&pool);
    assert!(matches!(
        processor.execute(envelope.clone()).await,
        Ok(ProductListingIngestionCompletion::Applied(_))
    ));
    let mut conflicting = prepared(
        source,
        Principal::System,
        "conflict",
        vec![create(source, "different")],
    )
    .await
    .remove(0);
    assert_eq!(
        conflicting.message.metadata.command_id,
        envelope.message.metadata.command_id
    );
    conflicting.fingerprint = ProductListingIngestionFingerprint::from_digest([18; 32]);
    assert!(matches!(
        processor.execute(conflicting).await,
        Err(ProductListingIngestionError::FingerprintConflict)
    ));
    assert_eq!(counts(&pool).await, (1, 1, 1));
    let rejected_listing: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM product_listings WHERE source_listing_id = 'different'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(rejected_listing, 0);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn failed_receipt_insert_rolls_back_listing_and_event_then_can_retry() {
    let pool = get_postgres_client().await;
    let source = seed_source(&pool).await;
    let envelope = prepared(
        source,
        Principal::System,
        "receipt-failure",
        vec![create(source, "one")],
    )
    .await
    .remove(0);
    sqlx::query("CREATE FUNCTION reject_ingestion_test_receipt() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'test receipt insert failure'; END $$")
        .execute(&pool).await.unwrap();
    sqlx::query("CREATE TRIGGER reject_ingestion_test_receipt BEFORE INSERT ON product_listing_command_receipts FOR EACH ROW EXECUTE FUNCTION reject_ingestion_test_receipt()")
        .execute(&pool).await.unwrap();
    let processor = handler(&pool);
    assert!(matches!(
        processor.execute(envelope.clone()).await,
        Err(ProductListingIngestionError::Receipt(_))
    ));
    assert_eq!(counts(&pool).await, (0, 0, 0));
    sqlx::query("DROP TRIGGER reject_ingestion_test_receipt ON product_listing_command_receipts")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DROP FUNCTION reject_ingestion_test_receipt()")
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        processor.execute(envelope).await,
        Ok(ProductListingIngestionCompletion::Applied(
            ProductListingIngestionEffect::Created(_)
        ))
    ));
    assert_eq!(counts(&pool).await, (1, 1, 1));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn later_failed_command_does_not_undo_previously_committed_command() {
    let pool = get_postgres_client().await;
    let source = seed_source(&pool).await;
    let mut envelopes = prepared(
        source,
        Principal::System,
        "batch",
        vec![create(source, "one"), update(source, "missing")],
    )
    .await;
    let processor = handler(&pool);
    let first = envelopes.remove(0);
    let second = envelopes.remove(0);
    assert!(matches!(
        processor.execute(first.clone()).await,
        Ok(ProductListingIngestionCompletion::Applied(
            ProductListingIngestionEffect::Created(_)
        ))
    ));
    assert!(matches!(
        processor.execute(second).await,
        Err(ProductListingIngestionError::Update(
            UpdateProductListingError::NotFound
        ))
    ));
    assert_eq!(counts(&pool).await, (1, 1, 1));
    assert_eq!(
        processor.execute(first).await.unwrap(),
        ProductListingIngestionCompletion::AlreadyCompleted
    );
    assert_eq!(counts(&pool).await, (1, 1, 1));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn duplicate_create_fails_without_receipt_and_original_completion_is_preserved() {
    let pool = get_postgres_client().await;
    let source = seed_source(&pool).await;
    let processor = handler(&pool);
    let first = prepared(
        source,
        Principal::System,
        "first",
        vec![create(source, "one")],
    )
    .await
    .remove(0);
    let second = prepared(
        source,
        Principal::System,
        "second",
        vec![create(source, "one")],
    )
    .await
    .remove(0);
    assert!(matches!(
        processor.execute(first.clone()).await,
        Ok(ProductListingIngestionCompletion::Applied(_))
    ));
    assert!(matches!(
        processor.execute(second).await,
        Err(ProductListingIngestionError::Create(
            CreateProductListingError::SourceListingAlreadyExists
        ))
    ));
    assert_eq!(counts(&pool).await, (1, 1, 1));
    assert_eq!(
        processor.execute(first).await.unwrap(),
        ProductListingIngestionCompletion::AlreadyCompleted
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn user_and_delegated_user_with_same_key_replay_one_receipt() {
    let pool = get_postgres_client().await;
    let source = seed_source(&pool).await;
    let actor = UserId::new();
    sqlx::query("INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'FREE', 'USER')")
        .bind(actor.into_uuid())
        .bind(format!("{actor}@example.test"))
        .execute(&pool)
        .await
        .unwrap();
    let party: uuid::Uuid = sqlx::query_scalar(
        "SELECT operator_party_id FROM listing_sources WHERE listing_source_id = $1",
    )
    .bind(source.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    let partnership = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO partnerships (partnership_id, party_id) VALUES ($1, $2)")
        .bind(partnership)
        .bind(party)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO partnership_members (user_id, partnership_id) VALUES ($1, $2)")
        .bind(actor.into_uuid())
        .bind(partnership)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO partnership_listing_source_grants (partnership_id, listing_source_id) VALUES ($1, $2)")
        .bind(partnership)
        .bind(source.into_uuid())
        .execute(&pool)
        .await
        .unwrap();

    let user = prepared(
        source,
        Principal::User(actor),
        "same-key",
        vec![upsert(source, "one", "https://example.test/user")],
    )
    .await
    .remove(0);
    let delegated = prepared(
        source,
        Principal::DelegatedUser {
            user_id: actor,
            capabilities: [CredentialCapability::ProductListingsWrite]
                .into_iter()
                .collect(),
        },
        "same-key",
        vec![upsert(source, "one", "https://example.test/user")],
    )
    .await
    .remove(0);
    // These integration envelopes must carry the actual verified codec fingerprint, unlike
    // the constant-fingerprint fixtures that isolate receipt/transaction behavior elsewhere.
    let user_wire = codec::decode(&codec::encode(&user.message).unwrap()).unwrap();
    let delegated_wire = codec::decode(&codec::encode(&delegated.message).unwrap()).unwrap();
    assert_eq!(
        codec::semantic_fingerprint(&user_wire).unwrap(),
        codec::semantic_fingerprint(&delegated_wire).unwrap()
    );
    assert_eq!(
        codec::fifo_deduplication_id(&user_wire).unwrap(),
        codec::fifo_deduplication_id(&delegated_wire).unwrap()
    );
    let user = user_wire.into_service_envelope().unwrap();
    let delegated = delegated_wire.into_service_envelope().unwrap();
    assert_eq!(
        user.message.metadata.submission_id,
        delegated.message.metadata.submission_id
    );
    assert_eq!(
        user.message.metadata.command_id,
        delegated.message.metadata.command_id
    );
    assert_ne!(
        user.message.metadata.actor,
        delegated.message.metadata.actor
    );
    let processor = handler(&pool);
    assert!(matches!(
        processor.execute(user.clone()).await.unwrap(),
        ProductListingIngestionCompletion::Applied(_)
    ));
    assert_eq!(
        processor.execute(delegated.clone()).await.unwrap(),
        ProductListingIngestionCompletion::AlreadyCompleted
    );
    assert_eq!(counts(&pool).await, (1, 1, 1));

    // A changed command with the same ID is a conflict, not a replay or second write.
    let changed = prepared(
        source,
        Principal::User(actor),
        "same-key",
        vec![upsert(source, "one", "https://example.test/changed")],
    )
    .await
    .remove(0);
    let changed = codec::decode(&codec::encode(&changed.message).unwrap())
        .unwrap()
        .into_service_envelope()
        .unwrap();
    assert_eq!(
        changed.message.metadata.command_id,
        user.message.metadata.command_id
    );
    assert_ne!(changed.fingerprint, user.fingerprint);
    assert!(matches!(
        processor.execute(changed).await,
        Err(ProductListingIngestionError::FingerprintConflict)
    ));
    assert_eq!(
        processor.execute(user).await.unwrap(),
        ProductListingIngestionCompletion::AlreadyCompleted
    );
    let url: String =
        sqlx::query_scalar("SELECT url FROM product_listings WHERE source_listing_id = 'one'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(url, "https://example.test/user");
    assert_eq!(counts(&pool).await, (1, 1, 1));

    let delegated_first = prepared(
        source,
        Principal::DelegatedUser {
            user_id: actor,
            capabilities: [CredentialCapability::ProductListingsWrite]
                .into_iter()
                .collect(),
        },
        "second-key",
        vec![upsert(source, "two", "https://example.test/two")],
    )
    .await
    .remove(0);
    let user_second = prepared(
        source,
        Principal::User(actor),
        "second-key",
        vec![upsert(source, "two", "https://example.test/two")],
    )
    .await
    .remove(0);
    let delegated_wire = codec::decode(&codec::encode(&delegated_first.message).unwrap()).unwrap();
    let user_wire = codec::decode(&codec::encode(&user_second.message).unwrap()).unwrap();
    assert_eq!(delegated_wire.command_id(), user_wire.command_id());
    assert_eq!(
        codec::semantic_fingerprint(&delegated_wire),
        codec::semantic_fingerprint(&user_wire)
    );
    assert_eq!(
        codec::fifo_deduplication_id(&delegated_wire),
        codec::fifo_deduplication_id(&user_wire)
    );
    assert!(matches!(
        processor
            .execute(delegated_wire.into_service_envelope().unwrap())
            .await
            .unwrap(),
        ProductListingIngestionCompletion::Applied(_)
    ));
    assert_eq!(
        processor
            .execute(user_wire.into_service_envelope().unwrap())
            .await
            .unwrap(),
        ProductListingIngestionCompletion::AlreadyCompleted
    );
    assert_eq!(counts(&pool).await, (2, 2, 2));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn revoked_partner_cannot_apply_new_command_or_insert_receipt() {
    let pool = get_postgres_client().await;
    let source = seed_source(&pool).await;
    let actor = UserId::new();
    sqlx::query("INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'FREE', 'USER')")
        .bind(actor.into_uuid())
        .bind(format!("{actor}@example.test"))
        .execute(&pool)
        .await
        .unwrap();
    let party: uuid::Uuid = sqlx::query_scalar(
        "SELECT operator_party_id FROM listing_sources WHERE listing_source_id = $1",
    )
    .bind(source.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    let partnership = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO partnerships (partnership_id, party_id) VALUES ($1, $2)")
        .bind(partnership)
        .bind(party)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO partnership_members (user_id, partnership_id) VALUES ($1, $2)")
        .bind(actor.into_uuid())
        .bind(partnership)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO partnership_listing_source_grants (partnership_id, listing_source_id) VALUES ($1, $2)")
        .bind(partnership).bind(source.into_uuid()).execute(&pool).await.unwrap();
    let processor = handler(&pool);
    let mut envelopes = prepared(
        source,
        Principal::User(actor),
        "revoked",
        vec![create(source, "one"), create(source, "two")],
    )
    .await;
    let first = envelopes.remove(0);
    let second = envelopes.remove(0);
    assert!(matches!(
        processor.execute(first.clone()).await,
        Ok(ProductListingIngestionCompletion::Applied(_))
    ));
    sqlx::query("DELETE FROM partnership_listing_source_grants WHERE partnership_id = $1 AND listing_source_id = $2")
        .bind(partnership).bind(source.into_uuid()).execute(&pool).await.unwrap();
    assert!(matches!(
        processor.execute(second).await,
        Err(ProductListingIngestionError::Create(
            CreateProductListingError::Forbidden
        ))
    ));
    assert_eq!(counts(&pool).await, (1, 1, 1));
    assert_eq!(
        processor.execute(first).await.unwrap(),
        ProductListingIngestionCompletion::AlreadyCompleted
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn completed_update_replay_after_newer_update_does_not_revert_state() {
    let pool = get_postgres_client().await;
    let source = seed_source(&pool).await;
    let processor = handler(&pool);
    let create = prepared(
        source,
        Principal::System,
        "update-seed",
        vec![create(source, "one")],
    )
    .await
    .remove(0);
    let ProductListingIngestionCompletion::Applied(ProductListingIngestionEffect::Created(created)) =
        processor.execute(create).await.unwrap()
    else {
        panic!("expected created listing");
    };
    let mut updates = prepared(
        source,
        Principal::System,
        "ordered-updates",
        vec![
            update_url(source, "one", "https://example.test/older"),
            update_url(source, "one", "https://example.test/newer"),
        ],
    )
    .await;
    let old = updates.remove(0);
    let new = updates.remove(0);
    assert!(matches!(
        processor.execute(old.clone()).await.unwrap(),
        ProductListingIngestionCompletion::Applied(ProductListingIngestionEffect::Updated(result))
            if result.product_listing_id == created.product_listing_id && result.outcome == ChangeOutcome::Changed
    ));
    assert!(matches!(
        processor.execute(new).await.unwrap(),
        ProductListingIngestionCompletion::Applied(ProductListingIngestionEffect::Updated(result))
            if result.product_listing_id == created.product_listing_id && result.outcome == ChangeOutcome::Changed
    ));
    let state: (String, i64) =
        sqlx::query_as("SELECT url, version FROM product_listings WHERE product_listing_id = $1")
            .bind(created.product_listing_id.into_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(state, ("https://example.test/newer".into(), 3));
    assert_eq!(counts(&pool).await, (1, 3, 3));

    // The first commit succeeded; a late redelivery cannot reapply the old URL.
    assert_eq!(
        handler(&pool).execute(old).await.unwrap(),
        ProductListingIngestionCompletion::AlreadyCompleted
    );
    let after: (String, i64) =
        sqlx::query_as("SELECT url, version FROM product_listings WHERE product_listing_id = $1")
            .bind(created.product_listing_id.into_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(after, state);
    assert_eq!(counts(&pool).await, (1, 3, 3));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn distinct_commands_with_one_submission_id_both_execute() {
    let pool = get_postgres_client().await;
    let source = seed_source(&pool).await;
    let mut commands = prepared(
        source,
        Principal::System,
        "two-in-one-submission",
        vec![create(source, "one"), create(source, "two")],
    )
    .await;
    let second = commands.remove(1);
    let first = commands.remove(0);
    assert_eq!(
        first.message.metadata.submission_id,
        second.message.metadata.submission_id
    );
    assert_ne!(
        first.message.metadata.command_id,
        second.message.metadata.command_id
    );
    let processor = handler(&pool);
    let ProductListingIngestionCompletion::Applied(ProductListingIngestionEffect::Created(
        first_result,
    )) = processor.execute(first.clone()).await.unwrap()
    else {
        panic!("first command must create a listing");
    };
    let ProductListingIngestionCompletion::Applied(ProductListingIngestionEffect::Created(
        second_result,
    )) = processor.execute(second.clone()).await.unwrap()
    else {
        panic!("second command must create its own listing");
    };
    assert_ne!(
        first_result.product_listing_id,
        second_result.product_listing_id
    );
    assert_eq!(counts(&pool).await, (2, 2, 2));
    let receipt_ids: Vec<String> = sqlx::query_scalar(
        "SELECT command_id FROM product_listing_command_receipts WHERE submission_id = $1 ORDER BY command_id",
    ).bind(first.message.metadata.submission_id.as_str()).fetch_all(&pool).await.unwrap();
    assert_eq!(receipt_ids.len(), 2);
    assert!(receipt_ids.contains(&first.message.metadata.command_id.as_str().to_owned()));
    assert!(receipt_ids.contains(&second.message.metadata.command_id.as_str().to_owned()));
    assert_eq!(
        processor.execute(first).await.unwrap(),
        ProductListingIngestionCompletion::AlreadyCompleted
    );
    assert_eq!(
        processor.execute(second).await.unwrap(),
        ProductListingIngestionCompletion::AlreadyCompleted
    );
    assert_eq!(counts(&pool).await, (2, 2, 2));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn committed_command_with_lost_ack_is_recognized_by_new_handler() {
    let pool = get_postgres_client().await;
    let source = seed_source(&pool).await;
    let envelope = prepared(
        source,
        Principal::System,
        "lost-ack",
        vec![create(source, "one")],
    )
    .await
    .remove(0);
    {
        let first_worker = handler(&pool);
        assert!(matches!(
            first_worker.execute(envelope.clone()).await.unwrap(),
            ProductListingIngestionCompletion::Applied(ProductListingIngestionEffect::Created(_))
        ));
        // Simulate a lost reply after commit: the first worker/result are discarded.
    }
    assert_eq!(counts(&pool).await, (1, 1, 1));
    let restarted_worker = handler(&pool);
    assert_eq!(
        restarted_worker.execute(envelope).await.unwrap(),
        ProductListingIngestionCompletion::AlreadyCompleted
    );
    assert_eq!(counts(&pool).await, (1, 1, 1));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn new_noop_update_commits_receipt_without_new_event() {
    let pool = get_postgres_client().await;
    let source = seed_source(&pool).await;
    let create = prepared(
        source,
        Principal::System,
        "noop-seed",
        vec![create(source, "one")],
    )
    .await
    .remove(0);
    let processor = handler(&pool);
    let ProductListingIngestionCompletion::Applied(ProductListingIngestionEffect::Created(created)) =
        processor.execute(create).await.unwrap()
    else {
        panic!("expected created listing");
    };
    let noop = prepared(
        source,
        Principal::System,
        "noop-update",
        vec![update(source, "one")],
    )
    .await
    .remove(0);
    assert!(matches!(processor.execute(noop.clone()).await.unwrap(),
        ProductListingIngestionCompletion::Applied(ProductListingIngestionEffect::Updated(result))
            if result.product_listing_id == created.product_listing_id && result.outcome == ChangeOutcome::Unchanged));
    assert_eq!(counts(&pool).await, (1, 1, 2));
    let version: i64 =
        sqlx::query_scalar("SELECT version FROM product_listings WHERE product_listing_id = $1")
            .bind(created.product_listing_id.into_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(version, 1);
    assert_eq!(
        processor.execute(noop).await.unwrap(),
        ProductListingIngestionCompletion::AlreadyCompleted
    );
    assert_eq!(counts(&pool).await, (1, 1, 2));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn upsert_update_withdraw_and_restore_preserve_one_listing_and_command_receipts() {
    let pool = get_postgres_client().await;
    let source = seed_source(&pool).await;
    let processor = handler(&pool);
    let first = prepared(
        source,
        Principal::System,
        "upsert-create",
        vec![upsert(source, "one", "https://example.test/first")],
    )
    .await
    .remove(0);
    let ProductListingIngestionCompletion::Applied(ProductListingIngestionEffect::Upserted(
        UpsertProductListingResult::Created(created),
    )) = processor.execute(first).await.unwrap()
    else {
        panic!("upsert must create missing listing");
    };
    let second = prepared(
        source,
        Principal::System,
        "upsert-update",
        vec![upsert(source, "one", "https://example.test/second")],
    )
    .await
    .remove(0);
    assert!(matches!(processor.execute(second).await.unwrap(),
        ProductListingIngestionCompletion::Applied(ProductListingIngestionEffect::Upserted(UpsertProductListingResult::Updated(result)))
            if result.product_listing_id == created.product_listing_id && result.outcome == ChangeOutcome::Changed));
    assert_eq!(counts(&pool).await, (1, 2, 2));
    let withdraw = prepared(
        source,
        Principal::System,
        "withdraw",
        vec![ProductListingIngestionIntent::Withdraw(product_key(
            source, "one",
        ))],
    )
    .await
    .remove(0);
    assert!(matches!(processor.execute(withdraw).await.unwrap(),
        ProductListingIngestionCompletion::Applied(ProductListingIngestionEffect::Withdrawn(result))
            if result.product_listing_id == created.product_listing_id && result.outcome == ChangeOutcome::Changed));
    let rejected = prepared(
        source,
        Principal::System,
        "update-while-withdrawn",
        vec![update_url(source, "one", "https://example.test/rejected")],
    )
    .await
    .remove(0);
    assert!(matches!(
        processor.execute(rejected).await,
        Err(ProductListingIngestionError::Update(
            UpdateProductListingError::ListingWithdrawn
        ))
    ));
    assert_eq!(counts(&pool).await, (1, 3, 3));
    let repeat = prepared(
        source,
        Principal::System,
        "withdraw-again",
        vec![ProductListingIngestionIntent::Withdraw(product_key(
            source, "one",
        ))],
    )
    .await
    .remove(0);
    assert!(matches!(processor.execute(repeat).await.unwrap(),
        ProductListingIngestionCompletion::Applied(ProductListingIngestionEffect::Withdrawn(result))
            if result.outcome == ChangeOutcome::Unchanged));
    assert_eq!(counts(&pool).await, (1, 3, 4));
    let restore = prepared(
        source,
        Principal::System,
        "restore",
        vec![upsert(source, "one", "https://example.test/restored")],
    )
    .await
    .remove(0);
    assert!(matches!(processor.execute(restore).await.unwrap(),
        ProductListingIngestionCompletion::Applied(ProductListingIngestionEffect::Upserted(UpsertProductListingResult::Updated(result)))
            if result.product_listing_id == created.product_listing_id && result.outcome == ChangeOutcome::Changed));
    let state: (String, String, i64) = sqlx::query_as(
        "SELECT lifecycle, url, version FROM product_listings WHERE product_listing_id = $1",
    )
    .bind(created.product_listing_id.into_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        state,
        ("ACTIVE".into(), "https://example.test/restored".into(), 4)
    );
    assert_eq!(counts(&pool).await, (1, 4, 5));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn queued_withdrawal_keeps_missing_target_unmodified_and_replays_once_per_command() {
    let pool = get_postgres_client().await;
    let source = seed_source(&pool).await;
    let processor = handler(&pool);
    let missing = prepared(
        source,
        Principal::System,
        "withdraw-missing",
        vec![ProductListingIngestionIntent::Withdraw(product_key(
            source, "missing",
        ))],
    )
    .await
    .remove(0);
    assert!(matches!(
        processor.execute(missing).await,
        Err(ProductListingIngestionError::Withdraw(
            WithdrawProductListingError::NotFound
        ))
    ));
    assert_eq!(counts(&pool).await, (0, 0, 0));

    let created = prepared(
        source,
        Principal::System,
        "create-before-withdraw",
        vec![create(source, "one")],
    )
    .await
    .remove(0);
    assert!(matches!(
        processor.execute(created).await.unwrap(),
        ProductListingIngestionCompletion::Applied(ProductListingIngestionEffect::Created(_))
    ));
    // Seed an otherwise valid observed-sale fixture to check that withdrawal only
    // clears the current availability assertion, not historical evidence.
    let fx_id = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO fx_rates (fx_rate_id, captured_at, source, source_event_id) VALUES ($1, $2, 'test', $3)")
        .bind(fx_id)
        .bind(OffsetDateTime::UNIX_EPOCH)
        .bind(format!("withdraw-observation-{fx_id}"))
        .execute(&pool).await.unwrap();
    sqlx::query("UPDATE product_listings SET availability = 'SOLD_OUT', sale_observation_fx_rate_id = $1, sale_observed_at = $2 WHERE listing_source_id = $3 AND source_listing_id = 'one'")
        .bind(fx_id)
        .bind(OffsetDateTime::UNIX_EPOCH)
        .bind(source.into_uuid())
        .execute(&pool).await.unwrap();
    let withdraw = prepared(
        source,
        Principal::System,
        "withdraw-one",
        vec![ProductListingIngestionIntent::Withdraw(product_key(
            source, "one",
        ))],
    )
    .await
    .remove(0);
    let original = withdraw.clone();
    assert!(matches!(
        processor.execute(withdraw).await.unwrap(),
        ProductListingIngestionCompletion::Applied(ProductListingIngestionEffect::Withdrawn(result))
            if result.outcome == ChangeOutcome::Changed
    ));
    assert_eq!(counts(&pool).await, (1, 2, 2));
    let state: (String, Option<String>, Option<uuid::Uuid>, Option<OffsetDateTime>) =
        sqlx::query_as("SELECT lifecycle, availability, sale_observation_fx_rate_id, sale_observed_at FROM product_listings WHERE listing_source_id = $1 AND source_listing_id = 'one'")
            .bind(source.into_uuid())
            .fetch_one(&pool).await.unwrap();
    assert_eq!(
        state,
        (
            "WITHDRAWN".into(),
            None,
            Some(fx_id),
            Some(OffsetDateTime::UNIX_EPOCH)
        )
    );
    assert_eq!(
        processor.execute(original).await.unwrap(),
        ProductListingIngestionCompletion::AlreadyCompleted
    );
    assert_eq!(counts(&pool).await, (1, 2, 2));
    let repeat = prepared(
        source,
        Principal::System,
        "withdraw-one-again",
        vec![ProductListingIngestionIntent::Withdraw(product_key(
            source, "one",
        ))],
    )
    .await
    .remove(0);
    assert!(matches!(
        processor.execute(repeat).await.unwrap(),
        ProductListingIngestionCompletion::Applied(ProductListingIngestionEffect::Withdrawn(result))
            if result.outcome == ChangeOutcome::Unchanged
    ));
    assert_eq!(counts(&pool).await, (1, 2, 3));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn raw_capture_distinguishes_noop_stale_provider_duplicate_and_command_replay() {
    let pool = get_postgres_client().await;
    let source = seed_source(&pool).await;
    let processor = handler(&pool);
    let newer = prepared(
        source,
        Principal::System,
        "raw-newer",
        vec![raw_capture(source, "newer", "delivery-newer", 2)],
    )
    .await
    .remove(0);
    let same = prepared(
        source,
        Principal::System,
        "raw-same",
        vec![raw_capture(source, "newer", "delivery-same", 3)],
    )
    .await
    .remove(0);
    let stale = prepared(
        source,
        Principal::System,
        "raw-stale",
        vec![raw_capture(source, "older", "delivery-older", 1)],
    )
    .await
    .remove(0);
    let duplicate = prepared(
        source,
        Principal::System,
        "raw-duplicate",
        vec![raw_capture(source, "older", "delivery-older", 1)],
    )
    .await
    .remove(0);
    assert!(matches!(
        processor.execute(newer).await.unwrap(),
        ProductListingIngestionCompletion::Applied(ProductListingIngestionEffect::RawCaptured(
            CaptureProductListingRawObservationResult::Changed { revision: 1, .. }
        ))
    ));
    assert!(matches!(
        processor.execute(same).await.unwrap(),
        ProductListingIngestionCompletion::Applied(ProductListingIngestionEffect::RawCaptured(
            CaptureProductListingRawObservationResult::Unchanged {
                latest_revision: 1,
                ..
            }
        ))
    ));
    assert!(matches!(
        processor.execute(stale.clone()).await.unwrap(),
        ProductListingIngestionCompletion::Applied(ProductListingIngestionEffect::RawCaptured(
            CaptureProductListingRawObservationResult::Stale {
                latest_revision: 1,
                ..
            }
        ))
    ));
    assert!(matches!(
        processor.execute(duplicate).await.unwrap(),
        ProductListingIngestionCompletion::Applied(ProductListingIngestionEffect::RawCaptured(
            CaptureProductListingRawObservationResult::Duplicate {
                latest_revision: 1,
                ..
            }
        ))
    ));
    assert_eq!(
        processor.execute(stale).await.unwrap(),
        ProductListingIngestionCompletion::AlreadyCompleted
    );
    assert_eq!(counts(&pool).await, (0, 0, 4));
    let revisions: i64 = sqlx::query_scalar("SELECT count(*) FROM product_listing_raw_revisions")
        .fetch_one(&pool)
        .await
        .unwrap();
    let provider_receipts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM product_listing_raw_provider_observation_receipts",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let head: i64 = sqlx::query_scalar(
        "SELECT latest_revision FROM product_listing_raw_streams WHERE listing_source_id = $1",
    )
    .bind(source.into_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((revisions, provider_receipts, head), (1, 3, 1));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn woocommerce_raw_command_replay_commits_only_one_capture_and_receipt() {
    let pool = get_postgres_client().await;
    let source = seed_source(&pool).await;
    let (actor, _) = seed_partner(&pool, source).await;
    let command = verified(
        prepared(
            source,
            Principal::User(actor),
            "woo-delivery-one",
            vec![woocommerce_raw_capture(source, "First", "delivery-one", 20)],
        )
        .await
        .remove(0),
    );

    assert!(matches!(
        handler(&pool).execute(command.clone()).await.unwrap(),
        ProductListingIngestionCompletion::Applied(ProductListingIngestionEffect::RawCaptured(
            CaptureProductListingRawObservationResult::Changed { revision: 1, .. }
        ))
    ));
    assert_eq!(counts(&pool).await, (0, 0, 1));
    assert_eq!(raw_counts(&pool).await, (1, 1, 1));
    let stored: (String, String, i64, Option<i64>) = sqlx::query_as(
        "SELECT ingestion_method, source_record_key, latest_revision, latest_provider_source_epoch_seconds \
         FROM product_listing_raw_streams WHERE listing_source_id = $1",
    )
    .bind(source.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(stored, ("WOOCOMMERCE".into(), "123".into(), 1, Some(20)));
    let revision: (String, String, Option<String>, serde_json::Value) = sqlx::query_as(
        "SELECT payload_format, operation, source_event_id, provenance FROM product_listing_raw_revisions",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(revision.0, "WOOCOMMERCE_PRODUCT");
    assert_eq!(revision.1, "UPSERT");
    assert_eq!(revision.2.as_deref(), Some("delivery-one"));
    assert_eq!(revision.3["topic"], "product.updated");
    let receipt: (String, String) = sqlx::query_as(
        "SELECT provider_scope, provider_delivery_id FROM product_listing_raw_provider_observation_receipts",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(receipt, ("product.updated".into(), "delivery-one".into()));

    assert_eq!(
        handler(&pool).execute(command).await.unwrap(),
        ProductListingIngestionCompletion::AlreadyCompleted
    );
    assert_eq!(counts(&pool).await, (0, 0, 1));
    assert_eq!(raw_counts(&pool).await, (1, 1, 1));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn woocommerce_distinct_commands_keep_stale_and_conflicting_evidence_out_of_revisions() {
    let pool = get_postgres_client().await;
    let source = seed_source(&pool).await;
    let (actor, _) = seed_partner(&pool, source).await;
    let processor = handler(&pool);
    let first = verified(
        prepared(
            source,
            Principal::User(actor),
            "woo-newer",
            vec![woocommerce_raw_capture(
                source,
                "Newer",
                "delivery-newer",
                20,
            )],
        )
        .await
        .remove(0),
    );
    let stale = verified(
        prepared(
            source,
            Principal::User(actor),
            "woo-stale",
            vec![woocommerce_raw_capture(
                source,
                "Older",
                "delivery-older",
                10,
            )],
        )
        .await
        .remove(0),
    );
    let provider_duplicate = verified(
        prepared(
            source,
            Principal::User(actor),
            "woo-stale-new-command",
            vec![woocommerce_raw_capture(
                source,
                "Older",
                "delivery-older",
                10,
            )],
        )
        .await
        .remove(0),
    );
    let equal_time_conflict = verified(
        prepared(
            source,
            Principal::User(actor),
            "woo-equal-time-conflict",
            vec![woocommerce_raw_capture(
                source,
                "Different",
                "delivery-different",
                20,
            )],
        )
        .await
        .remove(0),
    );
    let delivery_conflict = verified(
        prepared(
            source,
            Principal::User(actor),
            "woo-delivery-conflict",
            vec![woocommerce_raw_capture(
                source,
                "Changed",
                "delivery-newer",
                30,
            )],
        )
        .await
        .remove(0),
    );
    assert_ne!(
        first.message.metadata.command_id,
        stale.message.metadata.command_id
    );
    assert_ne!(
        stale.message.metadata.command_id,
        provider_duplicate.message.metadata.command_id
    );

    assert!(matches!(
        processor.execute(first).await.unwrap(),
        ProductListingIngestionCompletion::Applied(ProductListingIngestionEffect::RawCaptured(
            CaptureProductListingRawObservationResult::Changed { revision: 1, .. }
        ))
    ));
    assert!(matches!(
        processor.execute(stale.clone()).await.unwrap(),
        ProductListingIngestionCompletion::Applied(ProductListingIngestionEffect::RawCaptured(
            CaptureProductListingRawObservationResult::Stale {
                latest_revision: 1,
                ..
            }
        ))
    ));
    assert!(matches!(
        processor.execute(provider_duplicate).await.unwrap(),
        ProductListingIngestionCompletion::Applied(ProductListingIngestionEffect::RawCaptured(
            CaptureProductListingRawObservationResult::Duplicate {
                latest_revision: 1,
                ..
            }
        ))
    ));
    assert_eq!(
        processor.execute(stale).await.unwrap(),
        ProductListingIngestionCompletion::AlreadyCompleted
    );
    assert!(matches!(
        processor.execute(equal_time_conflict.clone()).await,
        Err(ProductListingIngestionError::CaptureRaw(
            CaptureProductListingRawObservationError::ProviderSourceOrderConflict
        ))
    ));
    assert!(matches!(
        processor.execute(delivery_conflict.clone()).await,
        Err(ProductListingIngestionError::CaptureRaw(
            CaptureProductListingRawObservationError::ProviderReceiptDigestConflict
        ))
    ));
    assert_eq!(counts(&pool).await, (0, 0, 3));
    assert_eq!(raw_counts(&pool).await, (1, 1, 2));
    let head: (i64, Option<i64>) = sqlx::query_as(
        "SELECT latest_revision, latest_provider_source_epoch_seconds \
         FROM product_listing_raw_streams WHERE listing_source_id = $1",
    )
    .bind(source.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(head, (1, Some(20)));
    for conflict in [equal_time_conflict, delivery_conflict] {
        let receipt_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM product_listing_command_receipts WHERE command_id = $1",
        )
        .bind(conflict.message.metadata.command_id.as_str())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(receipt_count, 0);
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn revoked_woocommerce_partner_cannot_mutate_raw_or_insert_new_receipts() {
    let pool = get_postgres_client().await;
    let source = seed_source(&pool).await;
    let (actor, partnership) = seed_partner(&pool, source).await;
    let mut commands = prepared(
        source,
        Principal::User(actor),
        "woo-revoke-batch",
        vec![
            woocommerce_raw_capture(source, "Before", "delivery-before", 20),
            woocommerce_raw_capture(source, "After", "delivery-after", 30),
        ],
    )
    .await;
    let first = verified(commands.remove(0));
    let second = verified(commands.remove(0));
    let processor = handler(&pool);
    assert!(matches!(
        processor.execute(first.clone()).await.unwrap(),
        ProductListingIngestionCompletion::Applied(ProductListingIngestionEffect::RawCaptured(
            CaptureProductListingRawObservationResult::Changed { revision: 1, .. }
        ))
    ));
    sqlx::query("DELETE FROM partnership_listing_source_grants WHERE partnership_id = $1 AND listing_source_id = $2")
        .bind(partnership)
        .bind(source.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        processor.execute(second.clone()).await,
        Err(ProductListingIngestionError::CaptureRaw(
            CaptureProductListingRawObservationError::Forbidden
        ))
    ));
    assert_eq!(counts(&pool).await, (0, 0, 1));
    assert_eq!(raw_counts(&pool).await, (1, 1, 1));
    let head: i64 = sqlx::query_scalar(
        "SELECT latest_revision FROM product_listing_raw_streams WHERE listing_source_id = $1",
    )
    .bind(source.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(head, 1);
    let new_receipt: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM product_listing_command_receipts WHERE command_id = $1",
    )
    .bind(second.message.metadata.command_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(new_receipt, 0);
    assert_eq!(
        processor.execute(first).await.unwrap(),
        ProductListingIngestionCompletion::AlreadyCompleted
    );
    assert_eq!(counts(&pool).await, (0, 0, 1));
    assert_eq!(raw_counts(&pool).await, (1, 1, 1));
}

fn assert_fresh_receipt_lookup(receipts: &TracingReceiptFactory) {
    let lookups = receipts.lookups.lock().unwrap();
    assert_eq!(lookups.len(), 2, "each attempt must look up the receipt");
    assert!(lookups.iter().all(|(_, found)| !found));
    assert_ne!(
        lookups[0].0, lookups[1].0,
        "retry must start a new PostgreSQL transaction"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn create_slug_collision_restarts_with_fresh_receipt_lookup() {
    let pool = get_postgres_client().await;
    let source = seed_source(&pool).await;
    let envelope = prepared(
        source,
        Principal::System,
        "create-slug-race",
        vec![create(source, "one")],
    )
    .await
    .remove(0);
    let repository = RacingRepositoryFactory {
        inner: SqlxProductListingRepositoryFactory::new(),
        interception: InsertInterception::SlugCollision,
        fired: Arc::new(AtomicBool::new(false)),
    };
    let receipts = TracingReceiptFactory::default();
    let processor = racing_handler(&pool, repository.clone(), receipts.clone());
    assert!(matches!(
        processor.execute(envelope.clone()).await.unwrap(),
        ProductListingIngestionCompletion::Applied(ProductListingIngestionEffect::Created(_))
    ));
    assert!(repository.fired.load(Ordering::SeqCst));
    assert_fresh_receipt_lookup(&receipts);
    assert_eq!(counts(&pool).await, (1, 1, 1));
    assert_eq!(
        handler(&pool).execute(envelope).await.unwrap(),
        ProductListingIngestionCompletion::AlreadyCompleted
    );
    assert_eq!(counts(&pool).await, (1, 1, 1));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn upsert_source_race_restarts_and_updates_competing_committed_listing() {
    let pool = get_postgres_client().await;
    let source = seed_source(&pool).await;
    let envelope = prepared(
        source,
        Principal::System,
        "upsert-source-race",
        vec![upsert(source, "one", "https://example.test/from-upsert")],
    )
    .await
    .remove(0);
    let ProductListingIngestionIntent::Create(competing_command) = create(source, "one") else {
        unreachable!()
    };
    let competing_id = Arc::new(Mutex::new(None));
    let repository = RacingRepositoryFactory {
        inner: SqlxProductListingRepositoryFactory::new(),
        interception: InsertInterception::CompetingCreate {
            pool: pool.clone(),
            command: competing_command,
            competing_id: Arc::clone(&competing_id),
        },
        fired: Arc::new(AtomicBool::new(false)),
    };
    let receipts = TracingReceiptFactory::default();
    let processor = racing_handler(&pool, repository.clone(), receipts.clone());
    let ProductListingIngestionCompletion::Applied(ProductListingIngestionEffect::Upserted(
        UpsertProductListingResult::Updated(updated),
    )) = processor.execute(envelope.clone()).await.unwrap()
    else {
        panic!("upsert must load and update the competing listing after a source race");
    };
    assert_eq!(updated.outcome, ChangeOutcome::Changed);
    assert_eq!(
        Some(updated.product_listing_id),
        *competing_id.lock().unwrap()
    );
    assert!(repository.fired.load(Ordering::SeqCst));
    assert_fresh_receipt_lookup(&receipts);
    let state: (String, i64) =
        sqlx::query_as("SELECT url, version FROM product_listings WHERE product_listing_id = $1")
            .bind(updated.product_listing_id.into_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(state, ("https://example.test/from-upsert".into(), 2));
    assert_eq!(counts(&pool).await, (1, 2, 1));
    assert_eq!(
        handler(&pool).execute(envelope).await.unwrap(),
        ProductListingIngestionCompletion::AlreadyCompleted
    );
    assert_eq!(counts(&pool).await, (1, 2, 1));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn upsert_slug_collision_restarts_with_fresh_receipt_lookup() {
    let pool = get_postgres_client().await;
    let source = seed_source(&pool).await;
    let envelope = prepared(
        source,
        Principal::System,
        "upsert-slug-race",
        vec![upsert(source, "one", "https://example.test/from-upsert")],
    )
    .await
    .remove(0);
    let repository = RacingRepositoryFactory {
        inner: SqlxProductListingRepositoryFactory::new(),
        interception: InsertInterception::SlugCollision,
        fired: Arc::new(AtomicBool::new(false)),
    };
    let receipts = TracingReceiptFactory::default();
    let processor = racing_handler(&pool, repository.clone(), receipts.clone());
    assert!(matches!(
        processor.execute(envelope.clone()).await.unwrap(),
        ProductListingIngestionCompletion::Applied(ProductListingIngestionEffect::Upserted(
            UpsertProductListingResult::Created(_)
        ))
    ));
    assert!(repository.fired.load(Ordering::SeqCst));
    assert_fresh_receipt_lookup(&receipts);
    assert_eq!(counts(&pool).await, (1, 1, 1));
    assert_eq!(
        handler(&pool).execute(envelope).await.unwrap(),
        ProductListingIngestionCompletion::AlreadyCompleted
    );
    assert_eq!(counts(&pool).await, (1, 1, 1));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn internal_service_needs_no_partnership_and_invalid_metadata_cannot_execute() {
    let pool = get_postgres_client().await;
    let source = seed_source(&pool).await;
    let envelope = prepared(
        source,
        Principal::Service("ingestion-worker".into()),
        "service-actor",
        vec![create(source, "one")],
    )
    .await
    .remove(0);
    let mut invalid = envelope.clone();
    invalid.message.metadata.listing_source_id = ListingSourceId::new();
    assert!(matches!(
        handler(&pool).execute(invalid).await,
        Err(ProductListingIngestionError::InvalidMetadata)
    ));
    assert_eq!(counts(&pool).await, (0, 0, 0));
    assert!(matches!(
        handler(&pool).execute(envelope).await,
        Ok(ProductListingIngestionCompletion::Applied(
            ProductListingIngestionEffect::Created(_)
        ))
    ));
    assert_eq!(counts(&pool).await, (1, 1, 1));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn create_with_missing_auction_reference_does_not_commit_or_write_receipt() {
    let pool = get_postgres_client().await;
    let source = seed_source(&pool).await;
    let ProductListingIngestionIntent::Create(mut command) = create(source, "one") else {
        unreachable!()
    };
    command.auction = Some(ProductListingAuctionPatch {
        auction_id: PatchField::Set(AuctionId::new()),
        ..Default::default()
    });
    let envelope = prepared(
        source,
        Principal::System,
        "missing-auction",
        vec![ProductListingIngestionIntent::Create(command)],
    )
    .await
    .remove(0);
    assert!(matches!(
        handler(&pool).execute(envelope).await,
        Err(ProductListingIngestionError::Create(
            CreateProductListingError::AuctionNotFound
        ))
    ));
    assert_eq!(counts(&pool).await, (0, 0, 0));
}
