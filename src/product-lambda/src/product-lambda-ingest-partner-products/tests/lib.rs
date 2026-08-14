use aws_lambda_events::sqs::{SqsEvent, SqsMessage};
use common::currency::data::CurrencyData;
use common::price::data::PriceData;
use common::shops_product_id::ShopsProductId;
use fake::{Fake, Faker};
use lambda_runtime::{Context, LambdaEvent};
use product::dynamodb::repository::{ProductDynamoDbRepository, ProductDynamoDbRepositoryImpl};
use product::service::command_service::CommandProductServiceImpl;
use product_lambda_ingest_partner_products::{
    AsyncProductCommandData, CreateAsyncProductCommandData, UpdateAsyncProductCommandData,
    UpsertAsyncProductCommandData, handler,
};
use shop::dynamodb::repository::{ShopDynamoDbRepository, ShopDynamoDbRepositoryImpl};
use shop::dynamodb::shop_record::ShopRecord;
use shop::dynamodb::shop_type_record::ShopTypeRecord;
use shop::service::get_service::GetShopServiceImpl;
use test_api::*;

fn sqs_event(command: AsyncProductCommandData) -> LambdaEvent<SqsEvent> {
    let mut message = SqsMessage::default();
    message.message_id = Some("msg-1".to_string());
    message.body = Some(serde_json::to_string(&command).unwrap());
    let mut event = SqsEvent::default();
    event.records = vec![message];
    LambdaEvent::new(event, Context::default())
}

#[aura_integration_test(services = [DynamoDB()])]
async fn should_create_product_when_create_command_is_received_for_partner_ingest_lambda() {
    let ddb_client = get_dynamodb_client().await;
    let shop_repository = ShopDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let product_repository = ProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let get_shop_service = GetShopServiceImpl::new(&shop_repository);
    let product_service = CommandProductServiceImpl::new(&product_repository, &get_shop_service);

    let mut shop_record: ShopRecord = Faker.fake();
    shop_record.shop_type = ShopTypeRecord::AuctionHouse;
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
        images: Default::default(),
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

#[aura_integration_test(services = [DynamoDB()])]
async fn should_update_product_when_update_command_is_received_for_partner_ingest_lambda() {
    let ddb_client = get_dynamodb_client().await;
    let shop_repository = ShopDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let product_repository = ProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let get_shop_service = GetShopServiceImpl::new(&shop_repository);
    let product_service = CommandProductServiceImpl::new(&product_repository, &get_shop_service);

    let mut shop_record: ShopRecord = Faker.fake();
    shop_record.shop_type = ShopTypeRecord::AuctionHouse;
    let shop_id = shop_record.shop_id;
    shop_repository.put_shop_record(shop_record).await.unwrap();

    let shops_product_id = ShopsProductId::from("partner-ingest-update-1".to_string());
    let initial_url = url::Url::parse("https://example.com/product/async-update-initial").unwrap();
    let create_response = handler(
        sqs_event(AsyncProductCommandData::Create(
            CreateAsyncProductCommandData {
                shop_id,
                shops_product_id: shops_product_id.clone(),
                title: common::language::data::LocalizedTextData::new(
                    "Initial Product",
                    common::language::data::LanguageData::En,
                ),
                description: common::language::data::LocalizedTextData::new(
                    "Created before async update",
                    common::language::data::LanguageData::En,
                ),
                price: Some(PriceData::new(CurrencyData::Eur, 4200)),
                price_estimate_min: None,
                price_estimate_max: None,
                state: product::data::product_state_data::ProductStateData::Available,
                url: initial_url,
                images: Default::default(),
                auction_start: None,
                auction_end: None,
                seller_name: None,
                structured_address: None,
                geo_address: None,
            },
        )),
        &product_service,
    )
    .await
    .unwrap();
    assert!(create_response.batch_item_failures.is_empty());

    let updated_url = url::Url::parse("https://example.com/product/async-update-final").unwrap();
    let updated_image_url =
        url::Url::parse("https://example.com/product/async-update-final/image-1.jpg").unwrap();
    let response = handler(
        sqs_event(AsyncProductCommandData::Update(
            UpdateAsyncProductCommandData {
                shop_id,
                shops_product_id: shops_product_id.clone(),
                price: Some(PriceData::new(CurrencyData::Eur, 5200)),
                state: Some(product::data::product_state_data::ProductStateData::Sold),
                price_estimate_min: Some(PriceData::new(CurrencyData::Eur, 5000)),
                price_estimate_max: Some(PriceData::new(CurrencyData::Eur, 5400)),
                url: Some(updated_url.clone()),
                images: Some(vec![updated_image_url.clone()].into_iter().collect()),
                auction_start: None,
                auction_end: None,
            },
        )),
        &product_service,
    )
    .await
    .unwrap();

    assert!(response.batch_item_failures.is_empty());
    let product = product_repository
        .get_product_record(&shop_id, &shops_product_id)
        .await
        .unwrap()
        .expect("product should exist after update command");

    assert_eq!("Initial Product", product.title_native.text.as_str());
    assert_eq!(
        Some("Created before async update"),
        product
            .description_native
            .as_ref()
            .map(|description| description.text.as_str())
    );
    assert_eq!(
        Some(common::currency::record::CurrencyRecord::Eur),
        product.price_native.as_ref().map(|price| price.currency)
    );
    assert_eq!(
        Some(5200),
        product.price_native.as_ref().map(|price| price.amount)
    );
    assert_eq!(
        Some(5000),
        product
            .price_estimate_min_native
            .as_ref()
            .map(|price| price.amount)
    );
    assert_eq!(
        Some(5400),
        product
            .price_estimate_max_native
            .as_ref()
            .map(|price| price.amount)
    );
    assert_eq!(
        product::dynamodb::product_state_record::ProductStateRecord::Sold,
        product.state
    );
    assert_eq!(updated_url, product.url);
    assert_eq!(
        vec![updated_image_url],
        product
            .images
            .iter()
            .map(|image| image.url.clone())
            .collect::<Vec<_>>()
    );
}

#[aura_integration_test(services = [DynamoDB()])]
async fn should_create_product_when_upsert_command_is_received_for_new_product_for_partner_ingest_lambda()
 {
    let ddb_client = get_dynamodb_client().await;
    let shop_repository = ShopDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let product_repository = ProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let get_shop_service = GetShopServiceImpl::new(&shop_repository);
    let product_service = CommandProductServiceImpl::new(&product_repository, &get_shop_service);

    let mut shop_record: ShopRecord = Faker.fake();
    shop_record.shop_type = ShopTypeRecord::AuctionHouse;
    let shop_id = shop_record.shop_id;
    shop_repository.put_shop_record(shop_record).await.unwrap();

    let shops_product_id = ShopsProductId::from("partner-ingest-upsert-create-1".to_string());
    let upsert_url = url::Url::parse("https://example.com/product/async-upsert-create").unwrap();
    let upsert_image_url =
        url::Url::parse("https://example.com/product/async-upsert-create/image-1.jpg").unwrap();
    let response = handler(
        sqs_event(AsyncProductCommandData::Upsert(
            UpsertAsyncProductCommandData {
                shop_id,
                shops_product_id: shops_product_id.clone(),
                title: Some(common::language::data::LocalizedTextData::new(
                    "Upserted Product",
                    common::language::data::LanguageData::En,
                )),
                description: Some(common::language::data::LocalizedTextData::new(
                    "Created from async upsert queue",
                    common::language::data::LanguageData::En,
                )),
                price: Some(PriceData::new(CurrencyData::Eur, 6100)),
                price_estimate_min: Some(PriceData::new(CurrencyData::Eur, 5900)),
                price_estimate_max: Some(PriceData::new(CurrencyData::Eur, 6300)),
                state: Some(product::data::product_state_data::ProductStateData::Available),
                url: Some(upsert_url.clone()),
                images: Some(vec![upsert_image_url.clone()].into_iter().collect()),
                auction_start: None,
                auction_end: None,
                seller_name: None,
                structured_address: None,
                geo_address: None,
            },
        )),
        &product_service,
    )
    .await
    .unwrap();

    assert!(response.batch_item_failures.is_empty());
    let product = product_repository
        .get_product_record(&shop_id, &shops_product_id)
        .await
        .unwrap()
        .expect("product should exist after upsert create command");

    assert_eq!("Upserted Product", product.title_native.text.as_str());
    assert_eq!(
        Some("Created from async upsert queue"),
        product
            .description_native
            .as_ref()
            .map(|description| description.text.as_str())
    );
    assert_eq!(
        Some(common::currency::record::CurrencyRecord::Eur),
        product.price_native.as_ref().map(|price| price.currency)
    );
    assert_eq!(
        Some(6100),
        product.price_native.as_ref().map(|price| price.amount)
    );
    assert_eq!(
        Some(5900),
        product
            .price_estimate_min_native
            .as_ref()
            .map(|price| price.amount)
    );
    assert_eq!(
        Some(6300),
        product
            .price_estimate_max_native
            .as_ref()
            .map(|price| price.amount)
    );
    assert_eq!(
        product::dynamodb::product_state_record::ProductStateRecord::Available,
        product.state
    );
    assert_eq!(upsert_url, product.url);
    assert_eq!(
        vec![upsert_image_url],
        product
            .images
            .iter()
            .map(|image| image.url.clone())
            .collect::<Vec<_>>()
    );
}

#[aura_integration_test(services = [DynamoDB()])]
async fn should_update_existing_product_when_upsert_command_is_received_for_partner_ingest_lambda()
{
    let ddb_client = get_dynamodb_client().await;
    let shop_repository = ShopDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let product_repository = ProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let get_shop_service = GetShopServiceImpl::new(&shop_repository);
    let product_service = CommandProductServiceImpl::new(&product_repository, &get_shop_service);

    let mut shop_record: ShopRecord = Faker.fake();
    shop_record.shop_type = ShopTypeRecord::AuctionHouse;
    let shop_id = shop_record.shop_id;
    shop_repository.put_shop_record(shop_record).await.unwrap();

    let shops_product_id = ShopsProductId::from("partner-ingest-upsert-update-1".to_string());
    let initial_url =
        url::Url::parse("https://example.com/product/async-upsert-update-initial").unwrap();
    let create_response = handler(
        sqs_event(AsyncProductCommandData::Create(
            CreateAsyncProductCommandData {
                shop_id,
                shops_product_id: shops_product_id.clone(),
                title: common::language::data::LocalizedTextData::new(
                    "Existing Product",
                    common::language::data::LanguageData::En,
                ),
                description: common::language::data::LocalizedTextData::new(
                    "Created before async upsert update",
                    common::language::data::LanguageData::En,
                ),
                price: Some(PriceData::new(CurrencyData::Eur, 7000)),
                price_estimate_min: None,
                price_estimate_max: None,
                state: product::data::product_state_data::ProductStateData::Available,
                url: initial_url,
                images: Default::default(),
                auction_start: None,
                auction_end: None,
                seller_name: None,
                structured_address: None,
                geo_address: None,
            },
        )),
        &product_service,
    )
    .await
    .unwrap();
    assert!(create_response.batch_item_failures.is_empty());

    let updated_url =
        url::Url::parse("https://example.com/product/async-upsert-update-final").unwrap();
    let updated_image_url =
        url::Url::parse("https://example.com/product/async-upsert-update-final/image-1.jpg")
            .unwrap();
    let response = handler(
        sqs_event(AsyncProductCommandData::Upsert(
            UpsertAsyncProductCommandData {
                shop_id,
                shops_product_id: shops_product_id.clone(),
                title: None,
                description: None,
                price: Some(PriceData::new(CurrencyData::Eur, 7800)),
                price_estimate_min: Some(PriceData::new(CurrencyData::Eur, 7500)),
                price_estimate_max: Some(PriceData::new(CurrencyData::Eur, 8100)),
                state: Some(product::data::product_state_data::ProductStateData::Reserved),
                url: Some(updated_url.clone()),
                images: Some(vec![updated_image_url.clone()].into_iter().collect()),
                auction_start: None,
                auction_end: None,
                seller_name: None,
                structured_address: None,
                geo_address: None,
            },
        )),
        &product_service,
    )
    .await
    .unwrap();

    assert!(response.batch_item_failures.is_empty());
    let product = product_repository
        .get_product_record(&shop_id, &shops_product_id)
        .await
        .unwrap()
        .expect("product should exist after upsert update command");

    assert_eq!("Existing Product", product.title_native.text.as_str());
    assert_eq!(
        Some("Created before async upsert update"),
        product
            .description_native
            .as_ref()
            .map(|description| description.text.as_str())
    );
    assert_eq!(
        Some(common::currency::record::CurrencyRecord::Eur),
        product.price_native.as_ref().map(|price| price.currency)
    );
    assert_eq!(
        Some(7800),
        product.price_native.as_ref().map(|price| price.amount)
    );
    assert_eq!(
        Some(7500),
        product
            .price_estimate_min_native
            .as_ref()
            .map(|price| price.amount)
    );
    assert_eq!(
        Some(8100),
        product
            .price_estimate_max_native
            .as_ref()
            .map(|price| price.amount)
    );
    assert_eq!(
        product::dynamodb::product_state_record::ProductStateRecord::Reserved,
        product.state
    );
    assert_eq!(updated_url, product.url);
    assert_eq!(
        vec![updated_image_url],
        product
            .images
            .iter()
            .map(|image| image.url.clone())
            .collect::<Vec<_>>()
    );
}
