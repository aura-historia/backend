use common::user_id::UserId;
use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use partner_api::handle;
use shop::{
    data::get_shop_data::GetShopData,
    dynamodb::repository::{ShopDynamoDbRepository, ShopDynamoDbRepositoryImpl},
    dynamodb::shop_record::ShopRecord,
    service::get_service::GetShopServiceImpl,
};
use test_api::*;
use user::service::user_service::MockUserService;

#[localstack_test(services = [DynamoDB()])]
async fn should_200_respond_shops_for_partner() {
    let repository = ShopDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let get_service = GetShopServiceImpl::new(&repository);
    let user_id = UserId::new();

    let mut shop_record: ShopRecord = Faker.fake();
    shop_record.partner_user_id = Some(user_id);
    shop_record.gsi1_pk = Some(shop::dynamodb::shop_record::mk_gsi1_pk(&user_id));
    shop_record.gsi1_sk = Some(shop::dynamodb::shop_record::mk_gsi1_sk(
        &shop_record.shop_id,
    ));
    repository
        .put_shop_record(shop_record.clone())
        .await
        .unwrap();

    let user_service = MockUserService::default();

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .route_key("GET /api/v1/partner/{partnerId}/shops")
            .path_parameter("partnerId", user_id.to_string())
            .jwt_claim("sub", user_id)
            .build(),
        context: Default::default(),
    };

    let response = handle(lambda_event, &get_service, &user_service)
        .await
        .unwrap();
    assert_eq!(200, response.status_code);

    let body: Vec<GetShopData> = match response.body {
        Some(aws_lambda_events::encodings::Body::Text(body_str)) => {
            serde_json::from_str(&body_str).unwrap()
        }
        _ => panic!("Expected response body to be Text"),
    };
    assert_eq!(1, body.len());
}
