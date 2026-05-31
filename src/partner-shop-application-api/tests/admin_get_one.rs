use common::user_id::UserId;
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
use user::core::role::UserRole;
use user::dynamodb::repository::UserDynamoDbRepositoryImpl;
use user::service::command::CreateUserCommand;
use user::service::user_service::{UserService, UserServiceImpl};

fn system_ctx() -> common::actor::RequestContext {
    common::actor::RequestContext {
        actor: common::actor::domain::Actor::System,
    }
}

async fn create_admin_user(user_service: &impl UserService) -> UserId {
    let user_id = UserId::new();
    let cmd = CreateUserCommand {
        id: user_id,
        email: format!("admin-{}@test.com", user_id).try_into().unwrap(),
    };
    user_service.create_user(&system_ctx(), cmd).await.unwrap();

    let update_cmd = user::service::command::UpdateUserCommand {
        role: Some(UserRole::Admin),
        ..Default::default()
    };
    user_service
        .update_user(&system_ctx(), &user_id, update_cmd)
        .await
        .unwrap();
    user_id
}

#[localstack_test(services = [DynamoDB()])]
async fn should_200_respond_application_by_id_for_admin() {
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

    let admin_user_id = create_admin_user(&user_service).await;

    let application = service
        .create_partner_shop_application(&system_ctx(), Faker.fake())
        .await
        .unwrap();

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .route_key("GET /api/v1/partner-applications/{partnerApplicationId}")
            .jwt_claim("sub", admin_user_id)
            .path_parameter("partnerApplicationId", application.id)
            .build(),
        context: Default::default(),
    };
    let response = handler(lambda_event, &service, &user_service)
        .await
        .unwrap();
    assert_eq!(200, response.status_code);

    let actual: GetPartnerShopApplicationData =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert_eq!(application.id, actual.id);
}
