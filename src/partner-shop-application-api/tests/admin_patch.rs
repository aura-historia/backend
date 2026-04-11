use common::user_id::UserId;
use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use partner_shop_application::{
    data::{
        admin_patch_partner_shop_application_data::AdminPatchPartnerShopApplicationData,
        get_partner_shop_application_data::GetPartnerShopApplicationData,
        partner_shop_application_state_data::PartnerShopApplicationStateData,
    },
    dynamodb::repository::{
        PartnerShopApplicationDynamoDbRepository, PartnerShopApplicationDynamoDbRepositoryImpl,
    },
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
    let mut sfn_adapter = partner_shop_application::service::sfn_adapter::MockSfnAdapter::default();
    sfn_adapter
        .expect_start_execution()
        .returning(|_, _| Box::pin(async { Ok("execution-arn".to_string()) }));
    sfn_adapter
        .expect_send_task_success()
        .returning(|_, _| Box::pin(async { Ok(()) }));
    let service = PartnerShopApplicationServiceImpl::new(
        &repository,
        &sfn_adapter,
        "arn:aws:states:us-east-1:123456789:stateMachine:test",
    );
    let user_repository = UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);

    let admin_user_id = create_admin_user(&user_service).await;

    let application = service
        .create_partner_shop_application(Faker.fake())
        .await
        .unwrap();

    // Simulate the step function setting the application to InReview with a task token
    let record_update =
        partner_shop_application::dynamodb::partner_shop_application_record_update::PartnerShopApplicationRecordUpdate {
            state: Some(
                partner_shop_application::dynamodb::partner_shop_application_state_record::PartnerShopApplicationStateRecord::InReview,
            ),
            task_token: Some("test-task-token".to_string()),
            shop_name: None,
            shop_type: None,
            shop_domains: None,
            shop_image: None,
            updated: time::OffsetDateTime::now_utc(),
        };
    repository
        .update_partner_shop_application_record(
            &application.applicant_user_id,
            &application.id,
            record_update,
        )
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
    // The response returns the existing record (before step function processes)
    // since the step function will handle the actual state change
    assert_eq!(200, response.status_code);

    let actual: GetPartnerShopApplicationData =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert_eq!(application.id, actual.id);
}
