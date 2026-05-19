use aws_lambda_events::dynamodb::{EventRecord, StreamRecord};
use aws_lambda_events::eventbridge::EventBridgeEvent;
use aws_lambda_events::sqs::{SqsEvent, SqsMessage};
use common::event_id::EventId;
use common::language::{domain::Language, record::LanguageRecord};
use common::price::domain::FixedFxRate;
use common::product_id::ProductId;
use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;
use fake::{Fake, Faker};
use fxrate::dynamodb::record::FxRatesRecord;
use fxrate::service::MockFxRateService;
use lambda_runtime::{Context, LambdaEvent};
use product::dynamodb::product_event_record::ProductEventRecord;
use product::dynamodb::product_event_record::enrichment::{
    ProductEnrichmentEventRecord, mk_pk as mk_enrichment_pk, mk_sk as mk_enrichment_sk,
};
use product::dynamodb::product_event_type_record::enrichment::ProductEnrichmentEventTypeRecord;
use product::dynamodb::product_record::ProductRecord;
use product::dynamodb::repository::{ProductDynamoDbRepository, ProductDynamoDbRepositoryImpl};
use product::service::command_service::CommandProductServiceImpl;
use product_pipeline_translate::handler;
use product_pipeline_translate::service::MockTranslationService;
use shop::dynamodb::repository::ShopDynamoDbRepositoryImpl;
use shop::service::get_service::GetShopServiceImpl;
use shop::service::seller_service::MockSellerService;
use std::collections::HashMap;
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

/// Creates a `ProductEnrichmentEventRecord` for an ENRICHMENT_EMBEDDED event.
fn mk_embedded_record(
    shop_id: ShopId,
    shops_product_id: ShopsProductId,
    product_id: ProductId,
    native_title: &str,
    source_language: Language,
) -> ProductEnrichmentEventRecord {
    let event_id = EventId::new();
    ProductEnrichmentEventRecord {
        pk: mk_enrichment_pk(&shop_id, &shops_product_id),
        sk: mk_enrichment_sk(&event_id),
        product_id,
        event_id,
        event_type: ProductEnrichmentEventTypeRecord::EnrichmentEmbedded,
        event_type_schema_version: 0,
        shop_id,
        seller_id: shop_id,
        shops_product_id,
        source_language: None,
        target_language: None,
        target: None,
        embedding: Some(vec![0.1, 0.2, 0.3]),
        native_title: Some(native_title.to_string()),
        native_title_language: Some(LanguageRecord::from(source_language)),
        timestamp: OffsetDateTime::now_utc(),
    }
}

#[localstack_test(services = [DynamoDB()])]
async fn should_translate_title_when_enrichment_embedded_event_triggers_pipeline() {
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
    product_record.title_de = None;
    product_record.title_en = None;
    product_record.title_fr = None;
    product_record.title_es = None;
    product_record.title_it = None;
    let shop_id = product_record.shop_id;
    let shops_product_id = product_record.shops_product_id.clone();
    let product_id = product_record.product_id;

    repository
        .put_product_records([product_record].into())
        .await
        .expect("shouldn't fail inserting product record");

    let embedded_record = mk_embedded_record(
        shop_id,
        shops_product_id.clone(),
        product_id,
        "Antiker Eichenstuhl",
        Language::De,
    );

    let mut mock_service = MockTranslationService::new();
    mock_service
        .expect_translate()
        .once()
        .returning(|titles, _| {
            let count = titles.len();
            Box::pin(async move {
                vec![
                    Some(HashMap::from([
                        (Language::En, "Antique oak chair".to_string()),
                        (Language::Fr, "Chaise en chêne ancienne".to_string()),
                        (Language::Es, "Silla de roble antigua".to_string()),
                        (Language::It, "Sedia in rovere antico".to_string()),
                    ]));
                    count
                ]
            })
        });

    let command_service = mk_command_service(
        &repository,
        &get_shop_service,
        &fx_rate_service,
        &seller_service,
    )
    .await;
    let event = mk_lambda_event(vec![mk_sqs_message(&ProductEventRecord::Enrichment(
        embedded_record,
    ))]);
    let result = handler(&mock_service, &command_service, event)
        .await
        .unwrap();

    assert!(
        result.batch_item_failures.is_empty(),
        "Expected no batch item failures but got: {:?}",
        result.batch_item_failures
    );

    // Verify translated titles are persisted in the product record.
    let updated_record = repository
        .get_product_record(&shop_id, &shops_product_id)
        .await
        .expect("shouldn't fail fetching updated product record")
        .expect("product record should exist");

    assert_eq!(
        Some("Antique oak chair".to_string()),
        updated_record.title_en,
        "Expected English translation"
    );
    assert_eq!(
        Some("Chaise en chêne ancienne".to_string()),
        updated_record.title_fr,
        "Expected French translation"
    );
    assert_eq!(
        Some("Silla de roble antigua".to_string()),
        updated_record.title_es,
        "Expected Spanish translation"
    );
    assert_eq!(
        Some("Sedia in rovere antico".to_string()),
        updated_record.title_it,
        "Expected Italian translation"
    );

    // Verify the enrichment event records were written via the transaction (one per target language).
    let mut enrichment_events = repository
        .query_product_enrichment_event_records(&shop_id, &shops_product_id)
        .await
        .expect("shouldn't fail querying enrichment event records");

    assert_eq!(
        4,
        enrichment_events.len(),
        "Expected 4 ENRICHMENT_TRANSLATED_TITLE event records (one per target language)"
    );
    assert!(
        enrichment_events
            .iter()
            .all(|e| e.event_type == ProductEnrichmentEventTypeRecord::EnrichmentTranslatedTitle),
        "Expected all enrichment event records to be ENRICHMENT_TRANSLATED_TITLE"
    );

    enrichment_events.sort_by_key(|e| e.target_language.map(|l| l.as_str()));
    let translations_by_language: HashMap<LanguageRecord, String> = enrichment_events
        .iter()
        .filter_map(|e| e.target_language.zip(e.target.clone()))
        .collect();

    assert_eq!(
        Some("Antique oak chair".to_string()),
        translations_by_language.get(&LanguageRecord::En).cloned(),
        "Expected English translation in written event record"
    );
    assert_eq!(
        Some("Chaise en chêne ancienne".to_string()),
        translations_by_language.get(&LanguageRecord::Fr).cloned(),
        "Expected French translation in written event record"
    );
    assert_eq!(
        Some("Silla de roble antigua".to_string()),
        translations_by_language.get(&LanguageRecord::Es).cloned(),
        "Expected Spanish translation in written event record"
    );
    assert_eq!(
        Some("Sedia in rovere antico".to_string()),
        translations_by_language.get(&LanguageRecord::It).cloned(),
        "Expected Italian translation in written event record"
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

    let titles = [
        "Victorian silver candlestick",
        "Antique mahogany writing desk",
        "Georgian silver tea service",
    ];

    let mut messages = Vec::new();
    let mut product_keys = Vec::new();

    for title in &titles {
        let mut product_record: ProductRecord = Faker.fake();
        product_record.title_en = None;
        product_record.title_de = None;
        product_record.title_fr = None;
        let shop_id = product_record.shop_id;
        let shops_product_id = product_record.shops_product_id.clone();
        let product_id = product_record.product_id;
        product_keys.push((shop_id, shops_product_id.clone()));

        repository
            .put_product_records([product_record].into())
            .await
            .expect("shouldn't fail inserting product record");

        let embedded_record = mk_embedded_record(
            shop_id,
            shops_product_id.clone(),
            product_id,
            title,
            Language::En,
        );
        messages.push(mk_sqs_message(&ProductEventRecord::Enrichment(
            embedded_record,
        )));
    }

    let mut mock_service = MockTranslationService::new();
    mock_service
        .expect_translate()
        .once()
        .returning(|titles, _| {
            let count = titles.len();
            Box::pin(async move {
                vec![
                    Some(HashMap::from([
                        (Language::De, "Viktorianischer Silberleuchter".to_string()),
                        (Language::Fr, "Chandelier en argent victorien".to_string()),
                    ]));
                    count
                ]
            })
        });

    let command_service = mk_command_service(
        &repository,
        &get_shop_service,
        &fx_rate_service,
        &seller_service,
    )
    .await;
    let event = mk_lambda_event(messages);
    let result = handler(&mock_service, &command_service, event)
        .await
        .unwrap();

    assert!(
        result.batch_item_failures.is_empty(),
        "Expected no batch item failures but got: {:?}",
        result.batch_item_failures
    );

    // Verify enrichment event records were written for each product via the transaction.
    for (shop_id, shops_product_id) in &product_keys {
        let enrichment_events = repository
            .query_product_enrichment_event_records(shop_id, shops_product_id)
            .await
            .expect("shouldn't fail querying enrichment event records");

        assert_eq!(
            2,
            enrichment_events.len(),
            "Expected 2 ENRICHMENT_TRANSLATED_TITLE event records per product (De and Fr)"
        );
        assert!(
            enrichment_events.iter().all(|e| {
                e.event_type == ProductEnrichmentEventTypeRecord::EnrichmentTranslatedTitle
            }),
            "Expected all enrichment records to be ENRICHMENT_TRANSLATED_TITLE"
        );

        let translations_by_language: HashMap<LanguageRecord, String> = enrichment_events
            .iter()
            .filter_map(|e| e.target_language.zip(e.target.clone()))
            .collect();

        assert_eq!(
            Some("Viktorianischer Silberleuchter".to_string()),
            translations_by_language.get(&LanguageRecord::De).cloned(),
            "Expected German translation in written event record"
        );
        assert_eq!(
            Some("Chandelier en argent victorien".to_string()),
            translations_by_language.get(&LanguageRecord::Fr).cloned(),
            "Expected French translation in written event record"
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

    // Create an embedded record for a product that does NOT exist in DynamoDB.
    let shop_id: ShopId = Faker.fake();
    let shops_product_id: ShopsProductId = Faker.fake();
    let embedded_record = mk_embedded_record(
        shop_id,
        shops_product_id,
        ProductId::new(),
        "Antiker Stuhl",
        Language::De,
    );

    let mut mock_service = MockTranslationService::new();
    mock_service.expect_translate().once().returning(|_, _| {
        Box::pin(async {
            vec![Some(HashMap::from([(
                Language::En,
                "Antique chair".to_string(),
            )]))]
        })
    });

    let command_service = mk_command_service(
        &repository,
        &get_shop_service,
        &fx_rate_service,
        &seller_service,
    )
    .await;
    let event = mk_lambda_event(vec![mk_sqs_message(&ProductEventRecord::Enrichment(
        embedded_record,
    ))]);
    // Product not found in DynamoDB → command_service.update returns the command as failed.
    let result = handler(&mock_service, &command_service, event)
        .await
        .unwrap();

    assert_eq!(
        1,
        result.batch_item_failures.len(),
        "Expected product-not-found to produce a batch item failure"
    );
}
