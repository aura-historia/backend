use common::user_id::UserId;
use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use partner_shop_application::{
    data::{
        admin_patch_partner_shop_application_data::AdminPatchPartnerShopApplicationData,
        get_partner_shop_application_data::GetPartnerShopApplicationData,
        partner_shop_application_state_data::PartnerShopApplicationStateData,
    },
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

async fn create_admin_user(user_service: &impl UserService) -> UserId {
    let user_id = UserId::new();
    let cmd = CreateUserCommand {
        id: user_id,
        email: format!("admin-{}@test.com", user_id).try_into().unwrap(),
    };
    user_service.create_user(cmd).await.unwrap();

    let update_cmd = user::service::command::UpdateUserCommand {
        role: Some(UserRole::Admin),
        ..Default::default()
    };
    user_service
        .update_user(&user_id, update_cmd)
        .await
        .unwrap();
    user_id
}

#[localstack_test(services = [DynamoDB()])]
async fn should_200_when_admin_updates_application_state() {
    let repository =
        PartnerShopApplicationDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let service = PartnerShopApplicationServiceImpl::new(&repository);
    let user_repository = UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);

    let admin_user_id = create_admin_user(&user_service).await;

    let application = service
        .create_partner_shop_application(Faker.fake())
        .await
        .unwrap();

    let patch_data = AdminPatchPartnerShopApplicationData {
        state: Some(PartnerShopApplicationStateData::Approved),
        shop_name: None,
        shop_type: None,
        shop_domains: None,
        shop_image: None,
    };
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::PATCH)
            .route_key("PATCH /api/v1/partner-applications/{partnerApplicationId}")
            .jwt_claim("sub", admin_user_id)
            .path_parameter("partnerApplicationId", application.id)
            .body_serde(&patch_data)
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
    assert_eq!(PartnerShopApplicationStateData::Approved, actual.state);
}
