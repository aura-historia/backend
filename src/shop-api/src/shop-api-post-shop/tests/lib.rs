use lambda_runtime::LambdaEvent;
use shop::{
    data::{get_shop_data::GetShopData, post_shop_data::PostShopData},
    dynamodb::repository::{ShopDynamoDbRepository, ShopDynamoDbRepositoryImpl},
    service::command_service::CommandShopServiceImpl,
};
use shop_api_post_shop::handler;
use test_api::*;
use url::Url;

#[localstack_test(services = [DynamoDB()])]
async fn should_create_shop_when_payload_valid() {
    let repository = ShopDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let service = CommandShopServiceImpl::new(&repository);

    let post_shop_data = PostShopData {
        name: "Hanses shippy shop".into(),
        urls: [
            Url::parse("https://hans.com").unwrap(),
            Url::parse("https://hansi-shoppy.de").unwrap(),
        ]
        .into(),
        image: None,
    };

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .body_serde(&post_shop_data)
            .build(),
        context: Default::default(),
    };
    let response = handler(lambda_event, &service).await.unwrap();
    assert_eq!(201, response.status_code);

    let actual_response_shop_data: GetShopData =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert_eq!(post_shop_data.name, actual_response_shop_data.name);
    assert_eq!(post_shop_data.urls, actual_response_shop_data.urls);
    assert_eq!(post_shop_data.image, actual_response_shop_data.image);

    let persisted_shop = repository
        .get_shop_record_by_id(&actual_response_shop_data.shop_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(post_shop_data.name, persisted_shop.name);
    assert_eq!(post_shop_data.urls, persisted_shop.urls);
    assert_eq!(post_shop_data.image, persisted_shop.image);
}
