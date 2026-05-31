use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use test_api::*;
use user::{
    data::get_user_data::GetUserAccountData,
    dynamodb::repository::UserDynamoDbRepositoryImpl,
    service::user_service::{UserService, UserServiceImpl},
};
use user_api::handler;

fn system_ctx() -> common::actor::RequestContext {
    common::actor::RequestContext {
        actor: common::actor::domain::Actor::System,
    }
}

#[localstack_test(services = [DynamoDB()])]
async fn should_200_respond_user_when_exists() {
    let repository = UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let service = UserServiceImpl::new(&repository);

    let user = service
        .create_user(&system_ctx(), Faker.fake())
        .await
        .unwrap();
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .route_key("GET /api/v1/me/account")
            .jwt_claim("sub", user.user_id)
            .build(),
        context: Default::default(),
    };
    let response = handler(lambda_event, &service).await.unwrap();
    let actual: GetUserAccountData =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();

    assert_eq!(GetUserAccountData::from(user), actual);
}
