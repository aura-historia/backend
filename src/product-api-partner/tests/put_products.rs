use common::price::domain::FixedFxRate;
use fake::{Fake, Faker};
use fxrate::dynamodb::record::FxRatesRecord;
use fxrate::service::MockFxRateService;
use lambda_runtime::LambdaEvent;
use product::dynamodb::product_record::{self, ProductRecord};
use product::dynamodb::repository::ProductDynamoDbRepositoryImpl;
use product::dynamodb::test_utils::ProductRecordSeedExt;
use product::service::command_service::CommandProductServiceImpl;
use shop::core::partner_shop_api_key::{HashedPartnerShopApiKey, PartnerShopApiKey};
use shop::dynamodb::repository::{ShopDynamoDbRepository, ShopDynamoDbRepositoryImpl};
use shop::dynamodb::shop_record::ShopRecord;
use shop::service::get_service::GetShopServiceImpl;
use shop::service::seller_service::MockSellerService;
use test_api::*;

fn make_partner_shop_record(api_key: &PartnerShopApiKey) -> ShopRecord {
    let hashed: HashedPartnerShopApiKey = api_key.clone().into();
    let mut record: ShopRecord = Faker.fake();
    record.partner_api_key_short = Some(hashed.short_token().to_string());
    record.partner_api_key_long_hash = Some(hashed.long_token_hash().to_string());
    record.partner_user_id = Some(Faker.fake());
    record
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_200_with_empty_errors_when_upserting_new_product() {
    let ddb_client = get_dynamodb_client().await;
    let shop_repository = ShopDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let product_repository = ProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let get_shop_service = GetShopServiceImpl::new(&shop_repository);
    let seller_service = MockSellerService::default();
    let mut fx_rate_service = MockFxRateService::new();
    fx_rate_service
        .expect_get_current()
        .returning(|| Box::pin(async { Ok(FxRatesRecord::from(FixedFxRate())) }));
    let command_product_service = CommandProductServiceImpl::new(
        &product_repository,
        &fx_rate_service,
        &get_shop_service,
        &seller_service,
    )
    .await
    .expect("shouldn't fail creating CommandProductServiceImpl");

    let api_key = PartnerShopApiKey::new();
    let shop_record = make_partner_shop_record(&api_key);
    let shop_id = shop_record.shop_id;
    shop_repository.put_shop_record(shop_record).await.unwrap();

    let api_key_str: String = api_key.into();
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::PUT)
            .route_key("PUT /api/v1/shops/{shopId}/products")
            .path_parameter("shopId", shop_id.to_string())
            .header("x-api-key", api_key_str)
            .body_serde(&vec![serde_json::json!({
                "shopsProductId": "integration-put-new-product-1",
                "title": { "text": "New Product via PUT", "language": "en" },
                "description": { "text": "A new product created via upsert", "language": "en" },
                "state": "AVAILABLE",
                "url": "https://example.com/product/1",
                "images": ["https://example.com/img.jpg"]
            })])
            .build(),
        context: Default::default(),
    };

    let response =
        product_api_partner::handle(lambda_event, &get_shop_service, &command_product_service)
            .await
            .unwrap();
    assert_eq!(200, response.status_code);

    let body: serde_json::Value = serde_json::from_str(
        response
            .body
            .as_ref()
            .and_then(|b| match b {
                aws_lambda_events::encodings::Body::Text(s) => Some(s.as_str()),
                _ => None,
            })
            .unwrap(),
    )
    .unwrap();
    assert!(body["errors"].as_object().unwrap().is_empty());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_200_with_empty_errors_when_upserting_existing_product() {
    let ddb_client = get_dynamodb_client().await;
    let shop_repository = ShopDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let product_repository = ProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let get_shop_service = GetShopServiceImpl::new(&shop_repository);
    let seller_service = MockSellerService::default();
    let mut fx_rate_service = MockFxRateService::new();
    fx_rate_service
        .expect_get_current()
        .returning(|| Box::pin(async { Ok(FxRatesRecord::from(FixedFxRate())) }));
    let command_product_service = CommandProductServiceImpl::new(
        &product_repository,
        &fx_rate_service,
        &get_shop_service,
        &seller_service,
    )
    .await
    .expect("shouldn't fail creating CommandProductServiceImpl");

    let api_key = PartnerShopApiKey::new();
    let shop_record = make_partner_shop_record(&api_key);
    let shop_id = shop_record.shop_id;
    shop_repository.put_shop_record(shop_record).await.unwrap();

    // First create the product so we can update it via upsert
    let mut product_record = Faker.fake::<ProductRecord>();
    product_record.shop_id = shop_id;
    product_record.shops_product_id = "integration-put-existing-product-1".into();
    product_record.pk = product_record::mk_pk(&shop_id, &product_record.shops_product_id);
    product_record.sk = product_record::mk_sk().to_owned();
    product_record.state = product::dynamodb::product_state_record::ProductStateRecord::Available;
    product_repository
        .transact_write_product_records_as_events([product_record])
        .await
        .unwrap();

    let api_key_str: String = api_key.into();
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::PUT)
            .route_key("PUT /api/v1/shops/{shopId}/products")
            .path_parameter("shopId", shop_id.to_string())
            .header("x-api-key", api_key_str)
            .body_serde(&vec![serde_json::json!({
                "shopsProductId": "integration-put-existing-product-1",
                "state": "SOLD"
            })])
            .build(),
        context: Default::default(),
    };

    let response =
        product_api_partner::handle(lambda_event, &get_shop_service, &command_product_service)
            .await
            .unwrap();
    assert_eq!(200, response.status_code);

    let body: serde_json::Value = serde_json::from_str(
        response
            .body
            .as_ref()
            .and_then(|b| match b {
                aws_lambda_events::encodings::Body::Text(s) => Some(s.as_str()),
                _ => None,
            })
            .unwrap(),
    )
    .unwrap();
    assert!(body["errors"].as_object().unwrap().is_empty());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_401_when_api_key_does_not_match_for_put() {
    let ddb_client = get_dynamodb_client().await;
    let shop_repository = ShopDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let product_repository = ProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let get_shop_service = GetShopServiceImpl::new(&shop_repository);
    let seller_service = MockSellerService::default();
    let mut fx_rate_service = MockFxRateService::new();
    fx_rate_service
        .expect_get_current()
        .returning(|| Box::pin(async { Ok(FxRatesRecord::from(FixedFxRate())) }));
    let command_product_service = CommandProductServiceImpl::new(
        &product_repository,
        &fx_rate_service,
        &get_shop_service,
        &seller_service,
    )
    .await
    .expect("shouldn't fail creating CommandProductServiceImpl");

    let correct_key = PartnerShopApiKey::new();
    let wrong_key = PartnerShopApiKey::new();
    let shop_record = make_partner_shop_record(&correct_key);
    let shop_id = shop_record.shop_id;
    shop_repository.put_shop_record(shop_record).await.unwrap();

    let wrong_key_str: String = wrong_key.into();
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::PUT)
            .route_key("PUT /api/v1/shops/{shopId}/products")
            .path_parameter("shopId", shop_id.to_string())
            .header("x-api-key", wrong_key_str)
            .body_serde(&vec![serde_json::json!({
                "shopsProductId": "test-product",
                "state": "AVAILABLE"
            })])
            .build(),
        context: Default::default(),
    };

    let response =
        product_api_partner::handle(lambda_event, &get_shop_service, &command_product_service)
            .await;
    assert!(response.is_err());
    assert_eq!(401, response.unwrap_err().status);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_404_when_shop_does_not_exist_for_put() {
    let ddb_client = get_dynamodb_client().await;
    let shop_repository = ShopDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let product_repository = ProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let get_shop_service = GetShopServiceImpl::new(&shop_repository);
    let seller_service = MockSellerService::default();
    let mut fx_rate_service = MockFxRateService::new();
    fx_rate_service
        .expect_get_current()
        .returning(|| Box::pin(async { Ok(FxRatesRecord::from(FixedFxRate())) }));
    let command_product_service = CommandProductServiceImpl::new(
        &product_repository,
        &fx_rate_service,
        &get_shop_service,
        &seller_service,
    )
    .await
    .expect("shouldn't fail creating CommandProductServiceImpl");

    let api_key = PartnerShopApiKey::new();
    let non_existent_shop_id = common::shop_id::ShopId::new();

    let api_key_str: String = api_key.into();
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::PUT)
            .route_key("PUT /api/v1/shops/{shopId}/products")
            .path_parameter("shopId", non_existent_shop_id.to_string())
            .header("x-api-key", api_key_str)
            .body_serde(&vec![serde_json::json!({
                "shopsProductId": "test-product",
                "state": "AVAILABLE"
            })])
            .build(),
        context: Default::default(),
    };

    let response =
        product_api_partner::handle(lambda_event, &get_shop_service, &command_product_service)
            .await;
    assert!(response.is_err());
    assert_eq!(404, response.unwrap_err().status);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_403_when_shop_is_not_a_partner_for_put() {
    let ddb_client = get_dynamodb_client().await;
    let shop_repository = ShopDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let product_repository = ProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let get_shop_service = GetShopServiceImpl::new(&shop_repository);
    let seller_service = MockSellerService::default();
    let mut fx_rate_service = MockFxRateService::new();
    fx_rate_service
        .expect_get_current()
        .returning(|| Box::pin(async { Ok(FxRatesRecord::from(FixedFxRate())) }));
    let command_product_service = CommandProductServiceImpl::new(
        &product_repository,
        &fx_rate_service,
        &get_shop_service,
        &seller_service,
    )
    .await
    .expect("shouldn't fail creating CommandProductServiceImpl");

    let api_key = PartnerShopApiKey::new();
    let mut shop_record: ShopRecord = Faker.fake();
    shop_record.partner_api_key_short = None;
    shop_record.partner_api_key_long_hash = None;
    let shop_id = shop_record.shop_id;
    shop_repository.put_shop_record(shop_record).await.unwrap();

    let api_key_str: String = api_key.into();
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::PUT)
            .route_key("PUT /api/v1/shops/{shopId}/products")
            .path_parameter("shopId", shop_id.to_string())
            .header("x-api-key", api_key_str)
            .body_serde(&vec![serde_json::json!({
                "shopsProductId": "test-product",
                "state": "AVAILABLE"
            })])
            .build(),
        context: Default::default(),
    };

    let response =
        product_api_partner::handle(lambda_event, &get_shop_service, &command_product_service)
            .await;
    assert!(response.is_err());
    assert_eq!(403, response.unwrap_err().status);
}
