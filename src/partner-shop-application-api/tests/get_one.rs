use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use partner_shop_application::{
    core::command::{CreatePartnerShopApplicationCommand, CreatePartnerShopApplicationPayload},
    data::get_partner_shop_application_data::{
        GetPartnerShopApplicationData, GetPartnerShopApplicationPayloadData,
    },
    dynamodb::repository::PartnerShopApplicationDynamoDbRepositoryImpl,
    service::partner_shop_application_service::{
        PartnerShopApplicationService, PartnerShopApplicationServiceImpl,
    },
};
use partner_shop_application_api::handler;
use shop::dynamodb::{
    repository::{ShopDynamoDbRepository, ShopDynamoDbRepositoryImpl},
    shop_record::ShopRecord,
};
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
async fn should_200_respond_application_when_exists() {
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

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .route_key("GET /api/v1/me/partner-applications/{partnerApplicationId}")
            .jwt_claim("sub", application.applicant_user_id)
            .path_parameter("partnerApplicationId", application.id)
            .build(),
        context: Default::default(),
    };
    let response = handler(lambda_event, &service, &user_service)
        .await
        .unwrap();
    let actual: GetPartnerShopApplicationData =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();

    assert_eq!(application.id, actual.id);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_200_respond_hydrated_shop_when_existing_application_exists() {
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

    let shop: shop::core::shop::Shop = Faker.fake();
    shop_repository
        .put_shop_record(ShopRecord::from(shop.clone()))
        .await
        .unwrap();
    let application = service
        .create_partner_shop_application(
            &system_ctx(),
            CreatePartnerShopApplicationCommand {
                applicant_user_id: common::user_id::UserId::new(),
                payload: CreatePartnerShopApplicationPayload::Existing(shop.shop_id),
            },
        )
        .await
        .unwrap();

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .route_key("GET /api/v1/me/partner-applications/{partnerApplicationId}")
            .jwt_claim("sub", application.applicant_user_id)
            .path_parameter("partnerApplicationId", application.id)
            .build(),
        context: Default::default(),
    };
    let response = handler(lambda_event, &service, &user_service)
        .await
        .unwrap();
    let actual: GetPartnerShopApplicationData =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();

    match actual.payload {
        GetPartnerShopApplicationPayloadData::Existing { shop: actual_shop } => {
            assert_eq!(shop.shop_id, actual_shop.shop_id);
            assert_eq!(shop.name, actual_shop.name);
        }
        payload => panic!("Expected existing payload, got {payload:?}"),
    }
}
