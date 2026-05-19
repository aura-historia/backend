use aws_lambda_events::dynamodb::{EventRecord, StreamRecord};
use aws_lambda_events::eventbridge::EventBridgeEvent;
use aws_lambda_events::sqs::{SqsEvent, SqsMessage};
use common::event::Event;
use common::event_id::EventId;
use common::language::domain::Language;
use common::language::record::{LanguageRecord, TextRecord};
use common::price::domain::FixedFxRate;
use fake::{Fake, Faker};
use fxrate::dynamodb::record::FxRatesRecord;
use fxrate::service::MockFxRateService;
use lambda_runtime::{Context, LambdaEvent};
use product::core::product_event::domain::{
    ProductCreatedDomainEventPayload, ProductDomainEventPayload,
};
use product::core::title::Title;
use product::dynamodb::product_event_record::ProductEventRecord;
use product::dynamodb::product_event_record::domain::ProductDomainEventRecord;
use product::dynamodb::product_event_type_record::enrichment::ProductEnrichmentEventTypeRecord;
use product::dynamodb::product_record::ProductRecord;
use product::dynamodb::repository::{ProductDynamoDbRepository, ProductDynamoDbRepositoryImpl};
use product::service::command_service::CommandProductServiceImpl;
use product_pipeline_embed_text::{handler, service::MockMultimodalEmbeddingService};
use shop::dynamodb::repository::ShopDynamoDbRepositoryImpl;
use shop::service::get_service::GetShopServiceImpl;
use shop::service::seller_service::MockSellerService;
use std::time::SystemTime;
use test_api::*;
use time::OffsetDateTime;
use uuid::Uuid;

async fn mk_command_service<'a>(
    repository: &'a ProductDynamoDbRepositoryImpl<'a>,
    get_shop_service: &'a GetShopServiceImpl<'a>,
    fx_rate_service: &'a MockFxRateService,
    seller_service: &'a MockSellerService,
) -> CommandProductServiceImpl<'a> {
    CommandProductServiceImpl::new(
        repository,
        fx_rate_service,
        get_shop_service,
        seller_service,
    )
    .await
    .expect("shouldn't fail creating CommandProductServiceImpl")
}

fn mk_event_bridge_payload(event_record: &impl serde::Serialize) -> String {
    let mut stream_record = StreamRecord::default();
    stream_record.approximate_creation_date_time = SystemTime::now().into();
    stream_record.new_image = serde_dynamo::to_item(event_record).unwrap();
    stream_record.size_bytes = 42;

    let mut event = EventRecord::default();
    event.aws_region = "eu-central-1".to_string();
    event.change = stream_record;
    event.event_id = Uuid::new_v4().to_string();
    event.event_name = "INSERT".to_string();

    let mut eb_event = EventBridgeEvent::<EventRecord>::default();
    eb_event.detail_type = "DynamoDBStreamRecord".to_string();
    eb_event.source = "test-table".to_string();
    eb_event.detail = event;

    serde_json::to_string(&eb_event).unwrap()
}

fn mk_sqs_message(record: &impl serde::Serialize) -> SqsMessage {
    let mut msg = SqsMessage::default();
    msg.message_id = Some(Faker.fake());
    msg.body = Some(mk_event_bridge_payload(record));
    msg
}

fn mk_lambda_event(messages: Vec<SqsMessage>) -> LambdaEvent<SqsEvent> {
    let mut sqs_event = SqsEvent::default();
    sqs_event.records = messages;
    LambdaEvent {
        payload: sqs_event,
        context: Context::default(),
    }
}

/// Creates a DOMAIN_CREATED event record from a ProductRecord, so both share the same key.
fn mk_domain_event_record_for_product(product_record: &ProductRecord) -> ProductDomainEventRecord {
    let mut payload: ProductCreatedDomainEventPayload = Faker.fake();
    payload.shop_id = product_record.shop_id;
    payload.seller_id = product_record.shop_id;
    payload.shops_product_id = product_record.shops_product_id.clone();
    payload.native_title =
        common::localized::Localized::new(Language::De, Title::from("Antiker Stuhl"));

    let event = Event {
        aggregate_id: product_record.product_id,
        event_id: EventId::new(),
        timestamp: OffsetDateTime::now_utc(),
        payload: ProductDomainEventPayload::Created(payload),
    };
    event.into()
}

#[localstack_test(services = [DynamoDB()])]
async fn should_embed_product_when_domain_created_event_triggers_pipeline() {
    let client = get_dynamodb_client().await;
    let repository = ProductDynamoDbRepositoryImpl::new(client, "table_1");
    let shop_repository = ShopDynamoDbRepositoryImpl::new(client, "table_1");
    let get_shop_service = GetShopServiceImpl::new(&shop_repository);
    let mut fx_rate_service = MockFxRateService::new();
    fx_rate_service
        .expect_get_current()
        .returning(|| Box::pin(async { Ok(FxRatesRecord::from(FixedFxRate())) }));
    let seller_service = MockSellerService::default();

    // Pre-populate a ProductRecord so CommandProductService::update can find it.
    let mut product_record: ProductRecord = Faker.fake();
    product_record.embedding = None;
    let shop_id = product_record.shop_id;
    let shops_product_id = product_record.shops_product_id.clone();

    repository
        .put_product_records([product_record.clone()].into())
        .await
        .expect("shouldn't fail inserting product record");

    let domain_record = mk_domain_event_record_for_product(&product_record);

    let mut mock_embedding_service = MockMultimodalEmbeddingService::new();
    mock_embedding_service
        .expect_embed()
        .once()
        .returning(|_, _, _| Box::pin(async { Ok(vec![0.1f32, 0.2f32, 0.3f32]) }));

    let command_service = mk_command_service(
        &repository,
        &get_shop_service,
        &fx_rate_service,
        &seller_service,
    )
    .await;
    let event = mk_lambda_event(vec![mk_sqs_message(&ProductEventRecord::Domain(
        domain_record,
    ))]);
    let result = handler(&mock_embedding_service, &command_service, event)
        .await
        .unwrap();

    assert!(
        result.batch_item_failures.is_empty(),
        "Expected no batch item failures but got: {:?}",
        result.batch_item_failures
    );

    // Verify the embedding is persisted in the materialized product record.
    let updated_record = repository
        .get_product_record(&shop_id, &shops_product_id)
        .await
        .expect("shouldn't fail fetching updated product record")
        .expect("product record should exist");

    assert!(
        updated_record.embedding.is_some(),
        "Expected product record to have an embedding after processing"
    );
    assert_eq!(
        Some(vec![0.1f32, 0.2f32, 0.3f32]),
        updated_record.embedding,
        "Expected embedding to match the mocked value"
    );

    // Verify the enrichment event record was written via the transaction.
    let enrichment_events = repository
        .query_product_enrichment_event_records(&shop_id, &shops_product_id)
        .await
        .expect("shouldn't fail querying enrichment event records");

    assert_eq!(
        1,
        enrichment_events.len(),
        "Expected exactly one ENRICHMENT_EMBEDDED event record"
    );
    assert_eq!(
        ProductEnrichmentEventTypeRecord::EnrichmentEmbedded,
        enrichment_events[0].event_type,
        "Expected ENRICHMENT_EMBEDDED event type in written event record"
    );
    assert_eq!(
        Some(vec![0.1f32, 0.2f32, 0.3f32]),
        enrichment_events[0].embedding,
        "Expected embedding to match in written enrichment event record"
    );
}

#[localstack_test(services = [DynamoDB()])]
async fn should_process_multiple_products_in_single_handler_invocation() {
    let client = get_dynamodb_client().await;
    let repository = ProductDynamoDbRepositoryImpl::new(client, "table_1");
    let shop_repository = ShopDynamoDbRepositoryImpl::new(client, "table_1");
    let get_shop_service = GetShopServiceImpl::new(&shop_repository);
    let mut fx_rate_service = MockFxRateService::new();
    fx_rate_service
        .expect_get_current()
        .returning(|| Box::pin(async { Ok(FxRatesRecord::from(FixedFxRate())) }));
    let seller_service = MockSellerService::default();

    let mut messages = Vec::new();
    let mut product_keys = Vec::new();

    for _ in 0..3 {
        let mut product_record: ProductRecord = Faker.fake();
        product_record.embedding = None;
        let shop_id = product_record.shop_id;
        let shops_product_id = product_record.shops_product_id.clone();
        product_keys.push((shop_id, shops_product_id.clone()));

        repository
            .put_product_records([product_record.clone()].into())
            .await
            .expect("shouldn't fail inserting product record");

        let domain_record = mk_domain_event_record_for_product(&product_record);
        messages.push(mk_sqs_message(&ProductEventRecord::Domain(domain_record)));
    }

    let mut mock_embedding_service = MockMultimodalEmbeddingService::new();
    mock_embedding_service
        .expect_embed()
        .times(3)
        .returning(|_, _, _| Box::pin(async { Ok(vec![0.42f32; 768]) }));

    let command_service = mk_command_service(
        &repository,
        &get_shop_service,
        &fx_rate_service,
        &seller_service,
    )
    .await;
    let event = mk_lambda_event(messages);
    let result = handler(&mock_embedding_service, &command_service, event)
        .await
        .unwrap();

    assert!(
        result.batch_item_failures.is_empty(),
        "Expected no batch item failures but got: {:?}",
        result.batch_item_failures
    );

    // Verify one enrichment event record was written per product via the transaction.
    for (shop_id, shops_product_id) in &product_keys {
        let enrichment_events = repository
            .query_product_enrichment_event_records(shop_id, shops_product_id)
            .await
            .expect("shouldn't fail querying enrichment event records");

        assert_eq!(
            1,
            enrichment_events.len(),
            "Expected exactly one ENRICHMENT_EMBEDDED event record per product"
        );
        assert_eq!(
            ProductEnrichmentEventTypeRecord::EnrichmentEmbedded,
            enrichment_events[0].event_type,
            "Expected ENRICHMENT_EMBEDDED event type"
        );
        assert_eq!(
            Some(vec![0.42f32; 768]),
            enrichment_events[0].embedding,
            "Expected embedding to match in written enrichment event record"
        );
    }
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_failure_when_product_not_found_in_dynamodb() {
    let client = get_dynamodb_client().await;
    let repository = ProductDynamoDbRepositoryImpl::new(client, "table_1");
    let shop_repository = ShopDynamoDbRepositoryImpl::new(client, "table_1");
    let get_shop_service = GetShopServiceImpl::new(&shop_repository);
    let mut fx_rate_service = MockFxRateService::new();
    fx_rate_service
        .expect_get_current()
        .returning(|| Box::pin(async { Ok(FxRatesRecord::from(FixedFxRate())) }));
    let seller_service = MockSellerService::default();

    // Create a domain event for a product that does NOT exist in DynamoDB.
    // Explicitly set title_native so the handler attempts to embed and then update the product.
    let mut domain_record: ProductDomainEventRecord = Faker.fake();
    domain_record.title_native = Some(TextRecord::new("Antiker Stuhl", LanguageRecord::De));

    let mut mock_embedding_service = MockMultimodalEmbeddingService::new();
    mock_embedding_service
        .expect_embed()
        .once()
        .returning(|_, _, _| Box::pin(async { Ok(vec![0.1f32]) }));

    let command_service = mk_command_service(
        &repository,
        &get_shop_service,
        &fx_rate_service,
        &seller_service,
    )
    .await;
    let event = mk_lambda_event(vec![mk_sqs_message(&ProductEventRecord::Domain(
        domain_record,
    ))]);
    let result = handler(&mock_embedding_service, &command_service, event)
        .await
        .unwrap();

    assert_eq!(
        1,
        result.batch_item_failures.len(),
        "Expected product-not-found to produce a batch item failure"
    );
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_no_failures_when_record_has_no_title() {
    let client = get_dynamodb_client().await;
    let repository = ProductDynamoDbRepositoryImpl::new(client, "table_1");
    let shop_repository = ShopDynamoDbRepositoryImpl::new(client, "table_1");
    let get_shop_service = GetShopServiceImpl::new(&shop_repository);
    let mut fx_rate_service = MockFxRateService::new();
    fx_rate_service
        .expect_get_current()
        .returning(|| Box::pin(async { Ok(FxRatesRecord::from(FixedFxRate())) }));
    let seller_service = MockSellerService::default();

    let mut domain_record: ProductDomainEventRecord = Faker.fake();
    domain_record.title_native = None;

    let mock_embedding_service = MockMultimodalEmbeddingService::new();
    let command_service = mk_command_service(
        &repository,
        &get_shop_service,
        &fx_rate_service,
        &seller_service,
    )
    .await;
    let event = mk_lambda_event(vec![mk_sqs_message(&ProductEventRecord::Domain(
        domain_record,
    ))]);
    let result = handler(&mock_embedding_service, &command_service, event)
        .await
        .unwrap();

    // Record with no title is skipped — no failure.
    assert!(result.batch_item_failures.is_empty());
}
