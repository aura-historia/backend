use aws_lambda_events::sqs::{SqsEvent, SqsMessage};
use common::currency::data::CurrencyData;
use common::price::data::PriceData;
use common::price::domain::FixedFxRate;
use common::shops_product_id::ShopsProductId;
use fake::{Fake, Faker};
use fxrate::dynamodb::record::FxRatesRecord;
use fxrate::service::MockFxRateService;
use lambda_runtime::{Context, LambdaEvent};
use product::dynamodb::repository::{ProductDynamoDbRepository, ProductDynamoDbRepositoryImpl};
use product::service::command_service::CommandProductServiceImpl;
use product_lambda_ingest_partner_products::{
    AsyncProductCommandData, CreateAsyncProductCommandData, handler,
};
use shop::dynamodb::repository::{ShopDynamoDbRepository, ShopDynamoDbRepositoryImpl};
use shop::dynamodb::shop_record::ShopRecord;
use shop::service::get_service::GetShopServiceImpl;
use shop::service::seller_service::MockSellerService;
use test_api::*;

fn sqs_event(command: AsyncProductCommandData) -> LambdaEvent<SqsEvent> {
    let mut message = SqsMessage::default();
    message.message_id = Some("msg-1".to_string());
    message.body = Some(serde_json::to_string(&command).unwrap());
    let mut event = SqsEvent::default();
    event.records = vec![message];
    LambdaEvent::new(event, Context::default())
}

#[localstack_test(services = [DynamoDB()])]
async fn should_create_product_when_create_command_is_received_for_partner_ingest_lambda() {
    let ddb_client = get_dynamodb_client().await;
    let shop_repository = ShopDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let product_repository = ProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let get_shop_service = GetShopServiceImpl::new(&shop_repository);
    let seller_service = MockSellerService::default();
    let mut fx_rate_service = MockFxRateService::new();
    fx_rate_service
        .expect_get_current()
        .returning(|| Box::pin(async { Ok(FxRatesRecord::from(FixedFxRate())) }));
    let product_service = CommandProductServiceImpl::new(
        &product_repository,
        &fx_rate_service,
        &get_shop_service,
        &seller_service,
    )
    .await
    .unwrap();

    let shop_record: ShopRecord = Faker.fake();
    let shop_id = shop_record.shop_id;
    shop_repository
        .put_shop_record(shop_record.clone())
        .await
        .unwrap();

    let shops_product_id = ShopsProductId::from("partner-ingest-create-1".to_string());
    let command = AsyncProductCommandData::Create(CreateAsyncProductCommandData {
        shop_id,
        shops_product_id: shops_product_id.clone(),
        title: common::language::data::LocalizedTextData::new(
            "Ingested Product",
            common::language::data::LanguageData::En,
        ),
        description: common::language::data::LocalizedTextData::new(
            "Created from async queue",
            common::language::data::LanguageData::En,
        ),
        price: Some(PriceData::new(CurrencyData::Eur, 4200)),
        price_estimate_min: None,
        price_estimate_max: None,
        state: product::data::product_state_data::ProductStateData::Available,
        url: url::Url::parse("https://example.com/product/async-create").unwrap(),
        images: vec![],
        auction_start: None,
        auction_end: None,
        seller_name: None,
        structured_address: None,
        geo_address: None,
    });

    let response = handler(sqs_event(command), &product_service).await.unwrap();

    assert!(response.batch_item_failures.is_empty());
    let product = product_repository
        .get_product_record(&shop_id, &shops_product_id)
        .await
        .unwrap();
    assert!(product.is_some());
}
