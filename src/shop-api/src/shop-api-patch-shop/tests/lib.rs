use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use shop::{
    data::{get_shop_data::GetShopData, patch_shop_data::PatchShopData},
    dynamodb::repository::ShopDynamoDbRepositoryImpl,
    service::command_service::{CommandShopService, CommandShopServiceImpl},
};
use shop_api_patch_shop::handler;
use test_api::*;
use url::Url;

#[localstack_test(services = [DynamoDB()])]
async fn should_update_shop_when_payload_valid_for_path_param_shop_id() {
    let repository = ShopDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let service = CommandShopServiceImpl::new(&repository);

    let existing_shop = service.create(Faker.fake()).await.unwrap();

    let patch_shop_data = PatchShopData {
        name: Some("hans goes shopping nig".into()),
        shop_type: None,
        domains: None,
        image: Some(Url::parse("https://hans-shopping-nig.co.uk").unwrap()),
    };

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::PATCH)
            .path_parameter("shopIdentifier", existing_shop.shop_id)
            .body_serde(&patch_shop_data)
            .build(),
        context: Default::default(),
    };
    let response = handler(lambda_event, &service).await.unwrap();
    assert_eq!(200, response.status_code);

    let actual: GetShopData =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();

    assert_eq!(patch_shop_data.name.unwrap(), actual.name);
    assert_eq!(existing_shop.domains, actual.domains);
    assert_eq!(patch_shop_data.image.unwrap(), actual.image.unwrap());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_update_shop_when_payload_valid_for_path_param_shop_domain() {
    let repository = ShopDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let service = CommandShopServiceImpl::new(&repository);

    let existing_shop = service.create(Faker.fake()).await.unwrap();

    let patch_shop_data = PatchShopData {
        name: Some("hans goes shopping nig".into()),
        shop_type: None,
        domains: None,
        image: Some(Url::parse("https://hans-shopping-nig.co.uk").unwrap()),
    };

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::PATCH)
            .path_parameter(
                "shopIdentifier",
                existing_shop.domains.iter().next().unwrap().clone(),
            )
            .body_serde(&patch_shop_data)
            .build(),
        context: Default::default(),
    };
    let response = handler(lambda_event, &service).await.unwrap();
    assert_eq!(200, response.status_code);

    let actual: GetShopData =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();

    assert_eq!(patch_shop_data.name.unwrap(), actual.name);
    assert_eq!(existing_shop.domains, actual.domains);
    assert_eq!(patch_shop_data.image.unwrap(), actual.image.unwrap());
}
