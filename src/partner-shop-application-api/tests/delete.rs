use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use partner_shop_application::{
    dynamodb::repository::{
        PartnerShopApplicationDynamoDbRepository, PartnerShopApplicationDynamoDbRepositoryImpl,
    },
    service::partner_shop_application_service::{
        PartnerShopApplicationService, PartnerShopApplicationServiceImpl,
    },
};
use partner_shop_application_api::handler;
use test_api::*;
use user::dynamodb::repository::UserDynamoDbRepositoryImpl;
use user::service::user_service::UserServiceImpl;

#[localstack_test(services = [DynamoDB()])]
async fn should_204_when_deleting_existing_application() {
    let repository =
        PartnerShopApplicationDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let mut sfn_adapter = partner_shop_application::service::sfn_adapter::MockSfnAdapter::default();
    sfn_adapter
        .expect_start_execution()
        .return_once(|_, _| Box::pin(async { Ok("foo".into()) }));
    let service = PartnerShopApplicationServiceImpl::new(
        &repository,
        &sfn_adapter,
        "arn:aws:states:us-east-1:123456789:stateMachine:test",
    );
    let user_repository = UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);

    let application = service
        .create_partner_shop_application(Faker.fake())
        .await
        .unwrap();

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::DELETE)
            .route_key("DELETE /api/v1/me/partner-applications/{partnerApplicationId}")
            .jwt_claim("sub", application.applicant_user_id)
            .path_parameter("partnerApplicationId", application.id)
            .build(),
        context: Default::default(),
    };
    let response = handler(lambda_event, &service, &user_service)
        .await
        .unwrap();
    assert_eq!(204, response.status_code);

    let deleted = repository
        .get_partner_shop_application_record(&application.applicant_user_id, &application.id)
        .await
        .unwrap();
    assert!(deleted.is_none());
}
