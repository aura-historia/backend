use common::shop_name::ShopName;
use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use partner_shop_application::{
    data::{
        get_partner_shop_application_data::GetPartnerShopApplicationData,
        partner_shop_application_state_data::PartnerShopApplicationStateData,
        patch_partner_shop_application_data::PatchPartnerShopApplicationData,
    },
    dynamodb::repository::PartnerShopApplicationDynamoDbRepositoryImpl,
    service::partner_shop_application_service::{
        PartnerShopApplicationService, PartnerShopApplicationServiceImpl,
    },
};
use partner_shop_application_api::handler;
use shop::dynamodb::repository::ShopDynamoDbRepositoryImpl;
use shop::service::get_service::GetShopServiceImpl;
use test_api::*;
use user::dynamodb::repository::UserDynamoDbRepositoryImpl;
use user::service::user_service::UserServiceImpl;

fn system_ctx() -> common::actor::RequestContext {
    common::actor::RequestContext {
        actor: common::actor::domain::Actor::System,
    }
}

#[localstack_test(services = [DynamoDB()])]
async fn should_200_when_updating_application() {
    let repository =
        PartnerShopApplicationDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let mut sfn_adapter = partner_shop_application::service::sfn_adapter::MockSfnAdapter::default();
    sfn_adapter
        .expect_start_execution()
        .returning(|_, _| Box::pin(async { Ok("foo".into()) }));
    let shop_repository = ShopDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let shop_service = GetShopServiceImpl::new(&shop_repository);
    let service = PartnerShopApplicationServiceImpl::new(
        &repository,
        &shop_service,
        &sfn_adapter,
        "arn:aws:states:us-east-1:123456789:stateMachine:test",
    );
    let user_repository = UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);

    let application = service
        .create_partner_shop_application(&system_ctx(), Faker.fake())
        .await
        .unwrap();

    let new_shop_name: ShopName = Faker.fake();
    let patch_data = PatchPartnerShopApplicationData {
        shop_name: Some(new_shop_name),
        shop_type: None,
        shop_domains: None,
        shop_url: None,
        shop_image: None,
        shop_structured_address: None,
        shop_phone: None,
        shop_email: None,
    };
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::PATCH)
            .route_key("PATCH /api/v1/me/partner-applications/{partnerApplicationId}")
            .jwt_claim("sub", application.applicant_user_id)
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
    assert_eq!(
        PartnerShopApplicationStateData::Submitted,
        actual.business_state
    );
}
