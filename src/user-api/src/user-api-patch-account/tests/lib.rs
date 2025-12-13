use common::{currency::data::CurrencyData, language::data::LanguageData};
use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use test_api::*;
use user::{
    data::{get_user_data::GetUserAccountData, patch_user_data::PatchUserAccountData},
    dynamodb::repository::UserDynamoDbRepositoryImpl,
    service::user_service::{UserService, UserServiceImpl},
};
use user_api_patch_account::handler;

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
    };
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::PATCH)
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
