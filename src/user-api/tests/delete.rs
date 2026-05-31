use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use test_api::*;
use user::{
    dynamodb::repository::{UserDynamoDbRepository, UserDynamoDbRepositoryImpl},
    service::{
        cognito_admin_service::MockCognitoAdminService,
        user_service::{UserService, UserServiceImpl},
    },
};
use user_api::handler;

fn system_ctx() -> common::actor::RequestContext {
    common::actor::RequestContext {
        actor: common::actor::domain::Actor::System,
    }
}

#[localstack_test(services = [DynamoDB()])]
async fn should_204_when_deleting_existing_user() {
    let repository = UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let mut cognito = MockCognitoAdminService::default();
    cognito
        .expect_admin_delete_user()
        .return_once(|_| Box::pin(async { Ok(()) }));
    let service = UserServiceImpl::with_cognito(&repository, &cognito);

    let user = service
        .create_user(&system_ctx(), Faker.fake())
        .await
        .unwrap();

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::DELETE)
            .route_key("DELETE /api/v1/me")
            .jwt_claim("sub", user.user_id)
            .build(),
        context: Default::default(),
    };
    let response = handler(lambda_event, &service).await.unwrap();
    assert_eq!(204, response.status_code);

    let deleted = repository.get_user_record(&user.user_id).await.unwrap();
    assert!(deleted.is_none());
}
