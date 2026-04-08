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

#[localstack_test(services = [DynamoDB()])]
async fn should_200_respond_application_when_exists() {
    let repository =
        PartnerShopApplicationDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let service = PartnerShopApplicationServiceImpl::new(&repository);

    let application = service
        .create_partner_shop_application(Faker.fake())
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
    let response = handler(lambda_event, &service).await.unwrap();
    let actual: GetPartnerShopApplicationData =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();

    assert_eq!(application.id, actual.id);
}
