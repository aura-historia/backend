use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use partner_shop_application::{
    data::get_partner_shop_application_data::GetPartnerShopApplicationData,
    dynamodb::repository::PartnerShopApplicationDynamoDbRepositoryImpl,
    service::partner_shop_application_service::{
        PartnerShopApplicationService, PartnerShopApplicationServiceImpl,
    },
};
use partner_shop_application_api::handler;
use test_api::*;
use user::dynamodb::repository::UserDynamoDbRepositoryImpl;
use user::service::user_service::UserServiceImpl;

fn system_ctx() -> common::actor::RequestContext {
    common::actor::RequestContext {
        actor: common::actor::domain::Actor::System,
    }
}

#[localstack_test(services = [DynamoDB()])]
async fn should_200_respond_empty_list_when_no_applications_exist() {
    let repository =
        PartnerShopApplicationDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let mut sfn_adapter = partner_shop_application::service::sfn_adapter::MockSfnAdapter::default();
    sfn_adapter
        .expect_start_execution()
        .returning(|_, _| Box::pin(async { Ok("foo".into()) }));
    let service = PartnerShopApplicationServiceImpl::new(
        &repository,
        &sfn_adapter,
        "arn:aws:states:us-east-1:123456789:stateMachine:test",
    );
    let user_repository = UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .route_key("GET /api/v1/me/partner-applications")
            .jwt_claim("sub", common::user_id::UserId::new())
            .build(),
        context: Default::default(),
    };
    let response = handler(lambda_event, &service, &user_service)
        .await
        .unwrap();
    let actual: Vec<GetPartnerShopApplicationData> =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();

    assert!(actual.is_empty());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_200_respond_applications_when_they_exist() {
    let repository =
        PartnerShopApplicationDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let mut sfn_adapter = partner_shop_application::service::sfn_adapter::MockSfnAdapter::default();
    sfn_adapter
        .expect_start_execution()
        .returning(|_, _| Box::pin(async { Ok("foo".into()) }));
    let service = PartnerShopApplicationServiceImpl::new(
        &repository,
        &sfn_adapter,
        "arn:aws:states:us-east-1:123456789:stateMachine:test",
    );
    let user_repository = UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);

    let application = service
        .create_partner_shop_application(&system_ctx(), Faker.fake())
        .await
        .unwrap();

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .route_key("GET /api/v1/me/partner-applications")
            .jwt_claim("sub", application.applicant_user_id)
            .build(),
        context: Default::default(),
    };
    let response = handler(lambda_event, &service, &user_service)
        .await
        .unwrap();
    let actual: Vec<GetPartnerShopApplicationData> =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();

    assert_eq!(1, actual.len());
    assert_eq!(application.id, actual[0].id);
}
