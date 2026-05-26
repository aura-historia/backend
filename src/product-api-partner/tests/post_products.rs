use common::has_key::HasKey;
use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use product_lambda_ingest_partner_products::{
    AsyncProductCommandData, AsyncProductCommandServiceImpl,
};
use shop::core::aura_historia_api_key::{HashedRawAccessToken, RawAccessToken};
use shop::dynamodb::repository::{ShopDynamoDbRepository, ShopDynamoDbRepositoryImpl};
use shop::dynamodb::shop_record::ShopRecord;
use shop::service::get_service::GetShopServiceImpl;
use test_api::*;

const SQS: Sqs = Sqs {
    name: "product_api_partner_post_products",
};

fn make_partner_shop_record(api_key: &RawAccessToken) -> ShopRecord {
    let hashed: HashedRawAccessToken = api_key.clone().into();
    let mut record: ShopRecord = Faker.fake();
    record.partner_api_key_short = Some(hashed.short_token().to_string());
    record.partner_api_key_long_hash = Some(hashed.long_token_hash().to_string());
    record.partner_user_id = Some(Faker.fake());
    record
}

async fn receive_forwarded_command() -> AsyncProductCommandData {
    let messages = get_sqs_client()
        .await
        .receive_message()
        .queue_url(SQS.queue_url())
        .max_number_of_messages(1)
        .send()
        .await
        .unwrap()
        .messages
        .unwrap_or_default();
    let message = messages.first().expect("expected queued product command");
    serde_json::from_str(message.body.as_deref().unwrap()).unwrap()
}

#[localstack_test(services = [DynamoDB(), SQS])]
async fn should_return_202_and_forward_create_command_when_products_created_successfully() {
    let ddb_client = get_dynamodb_client().await;
    let shop_repository = ShopDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let get_shop_service = GetShopServiceImpl::new(&shop_repository);
    let async_product_command_service =
        AsyncProductCommandServiceImpl::new(get_sqs_client().await, SQS.queue_url());

    let api_key = RawAccessToken::new();
    let shop_record = make_partner_shop_record(&api_key);
    let shop_id = shop_record.shop_id;
    shop_repository.put_shop_record(shop_record).await.unwrap();

    let api_key_str: String = api_key.into();
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .route_key("POST /api/v1/shops/{shopId}/products")
            .path_parameter("shopId", shop_id.to_string())
            .header("x-aura-historia-access-token", api_key_str)
            .body_serde(&vec![serde_json::json!({
                "shopsProductId": "integration-product-1",
                "title": { "text": "Test Product", "language": "en" },
                "description": { "text": "A test product", "language": "en" },
                "state": "AVAILABLE",
                "url": "https://example.com/product/1",
                "images": ["https://example.com/img.jpg"]
            })])
            .build(),
        context: Default::default(),
    };

    let response = product_api_partner::handle(
        lambda_event,
        &get_shop_service,
        &async_product_command_service,
    )
    .await
    .unwrap();
    assert_eq!(202, response.status_code);
    let body: Vec<String> = serde_json::from_str(
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
    assert!(body.is_empty());

    let command = receive_forwarded_command().await;
    assert_eq!(command.key().shop_id, shop_id);
    assert_eq!(
        command.key().shops_product_id.to_string(),
        "integration-product-1"
    );
    assert!(matches!(command, AsyncProductCommandData::Create(_)));
}

#[localstack_test(services = [DynamoDB(), SQS])]
async fn should_return_401_when_api_key_does_not_match() {
    let ddb_client = get_dynamodb_client().await;
    let shop_repository = ShopDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let get_shop_service = GetShopServiceImpl::new(&shop_repository);
    let async_product_command_service =
        AsyncProductCommandServiceImpl::new(get_sqs_client().await, SQS.queue_url());

    let correct_key = RawAccessToken::new();
    let wrong_key = RawAccessToken::new();
    let shop_record = make_partner_shop_record(&correct_key);
    let shop_id = shop_record.shop_id;
    shop_repository.put_shop_record(shop_record).await.unwrap();

    let wrong_key_str: String = wrong_key.into();
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .route_key("POST /api/v1/shops/{shopId}/products")
            .path_parameter("shopId", shop_id.to_string())
            .header("x-aura-historia-access-token", wrong_key_str)
            .body_serde(&vec![serde_json::json!({
                "shopsProductId": "test-product",
                "title": { "text": "Test", "language": "en" },
                "description": { "text": "Test", "language": "en" },
                "state": "AVAILABLE",
                "url": "https://example.com/product/1",
                "images": []
            })])
            .build(),
        context: Default::default(),
    };

    let response = product_api_partner::handle(
        lambda_event,
        &get_shop_service,
        &async_product_command_service,
    )
    .await;
    assert!(response.is_err());
    assert_eq!(401, response.unwrap_err().status);
}

#[localstack_test(services = [DynamoDB(), SQS])]
async fn should_return_404_when_shop_does_not_exist() {
    let ddb_client = get_dynamodb_client().await;
    let shop_repository = ShopDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let get_shop_service = GetShopServiceImpl::new(&shop_repository);
    let async_product_command_service =
        AsyncProductCommandServiceImpl::new(get_sqs_client().await, SQS.queue_url());

    let api_key = RawAccessToken::new();
    let non_existent_shop_id = common::shop_id::ShopId::new();

    let api_key_str: String = api_key.into();
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .route_key("POST /api/v1/shops/{shopId}/products")
            .path_parameter("shopId", non_existent_shop_id.to_string())
            .header("x-aura-historia-access-token", api_key_str)
            .body_serde(&vec![serde_json::json!({
                "shopsProductId": "test-product",
                "title": { "text": "Test", "language": "en" },
                "description": { "text": "Test", "language": "en" },
                "state": "AVAILABLE",
                "url": "https://example.com/product/1",
                "images": []
            })])
            .build(),
        context: Default::default(),
    };

    let response = product_api_partner::handle(
        lambda_event,
        &get_shop_service,
        &async_product_command_service,
    )
    .await;
    assert!(response.is_err());
    assert_eq!(404, response.unwrap_err().status);
}

#[localstack_test(services = [DynamoDB(), SQS])]
async fn should_return_403_when_shop_is_not_a_partner() {
    let ddb_client = get_dynamodb_client().await;
    let shop_repository = ShopDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let get_shop_service = GetShopServiceImpl::new(&shop_repository);
    let async_product_command_service =
        AsyncProductCommandServiceImpl::new(get_sqs_client().await, SQS.queue_url());

    let api_key = RawAccessToken::new();
    let mut shop_record: ShopRecord = Faker.fake();
    shop_record.partner_api_key_short = None;
    shop_record.partner_api_key_long_hash = None;
    let shop_id = shop_record.shop_id;
    shop_repository.put_shop_record(shop_record).await.unwrap();

    let api_key_str: String = api_key.into();
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .route_key("POST /api/v1/shops/{shopId}/products")
            .path_parameter("shopId", shop_id.to_string())
            .header("x-aura-historia-access-token", api_key_str)
            .body_serde(&vec![serde_json::json!({
                "shopsProductId": "test-product",
                "title": { "text": "Test", "language": "en" },
                "description": { "text": "Test", "language": "en" },
                "state": "AVAILABLE",
                "url": "https://example.com/product/1",
                "images": []
            })])
            .build(),
        context: Default::default(),
    };

    let response = product_api_partner::handle(
        lambda_event,
        &get_shop_service,
        &async_product_command_service,
    )
    .await;
    assert!(response.is_err());
    assert_eq!(403, response.unwrap_err().status);
}
