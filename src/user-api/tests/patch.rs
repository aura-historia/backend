use common::{currency::data::CurrencyData, language::data::LanguageData};
use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use test_api::*;
use user::{
    data::{get_user_data::GetUserAccountData, patch_user_data::PatchUserAccountData},
    dynamodb::repository::UserDynamoDbRepositoryImpl,
    service::user_service::{UserService, UserServiceImpl},
};
use user_api::handler;

#[localstack_test(services = [DynamoDB()])]
async fn should_200_respond_user_when_exists() {
    let repository = UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let service = UserServiceImpl::new(&repository);

    let user = service.create_user(Faker.fake()).await.unwrap();

    let patch_user_account_data = PatchUserAccountData {
        first_name: Some("Hansi".into()),
        last_name: Some("Hans".into()),
        language: Some(LanguageData::Fr),
        currency: Some(CurrencyData::Nzd),
        prohibited_content_consent: None,
        marketing_consent: None,
    };
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::PATCH)
            .route_key("PATCH /api/v1/me/account")
            .jwt_claim("sub", user.user_id)
            .body_serde(&patch_user_account_data)
            .build(),
        context: Default::default(),
    };
    let response = handler(lambda_event, &service).await.unwrap();
    let actual: GetUserAccountData =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();

    assert_eq!(
        patch_user_account_data.first_name.unwrap(),
        actual.first_name.unwrap()
    );
    assert_eq!(
        patch_user_account_data.last_name.unwrap(),
        actual.last_name.unwrap()
    );
    assert_eq!(
        patch_user_account_data.language.unwrap(),
        actual.language.unwrap()
    );
    assert_eq!(
        patch_user_account_data.currency.unwrap(),
        actual.currency.unwrap()
    );
}

#[localstack_test(services = [DynamoDB()])]
async fn should_update_prohibited_content_consent_when_provided() {
    let repository = UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let service = UserServiceImpl::new(&repository);

    let user = service.create_user(Faker.fake()).await.unwrap();
    assert!(!user.prohibited_content_consent);

    let patch_user_account_data = PatchUserAccountData {
        first_name: None,
        last_name: None,
        language: None,
        currency: None,
        prohibited_content_consent: Some(true),
        marketing_consent: None,
    };
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::PATCH)
            .route_key("PATCH /api/v1/me/account")
            .jwt_claim("sub", user.user_id)
            .body_serde(&patch_user_account_data)
            .build(),
        context: Default::default(),
    };
    let response = handler(lambda_event, &service).await.unwrap();
    let actual: GetUserAccountData =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();

    assert!(actual.prohibited_content_consent);
}
