use lambda_runtime::LambdaEvent;
use oauth::core::third_party_exchange_code::{ThirdPartyExchangeCode, ThirdPartyExchangeCodeGrant};
use oauth::data::TokenResponseData;
use oauth::dynamodb::repository::{OAuthDynamoDbRepositoryImpl, OAuthRepository};
use oauth::dynamodb::third_party_exchange_code_record::ThirdPartyExchangeCodeRecord;
use oauth::service::oauth_service::OAuthServiceImpl;
use test_api::*;
use time::{Duration, OffsetDateTime};
use user::core::access_token::{RawAccessToken, Scope};
use user::dynamodb::repository::UserDynamoDbRepositoryImpl;
use user::service::user_service::UserServiceImpl;

fn now() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(OffsetDateTime::now_utc().unix_timestamp()).unwrap()
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_access_token_when_third_party_code_is_valid_for_handler() {
    let dynamodb_client = get_dynamodb_client().await;
    let oauth_repository = OAuthDynamoDbRepositoryImpl::new(dynamodb_client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(dynamodb_client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let oauth_service = OAuthServiceImpl::new(&oauth_repository, &user_service);
    let now = now();
    let raw_access_token = RawAccessToken::new();
    let third_party_code = ThirdPartyExchangeCode::new();
    let grant = ThirdPartyExchangeCodeGrant {
        code: third_party_code,
        access_token: raw_access_token.clone(),
        access_token_expires: Some(now + Duration::hours(1)),
        scopes: std::collections::HashSet::from([Scope::ProductsWrite]),
        expires: now + Duration::seconds(60),
        created: now,
    };
    oauth_repository
        .put_third_party_exchange_code_record(ThirdPartyExchangeCodeRecord::from(grant))
        .await
        .unwrap();

    let response = oauth_api::handler(
        LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/oauth/tokens/by-third-party-code/{thirdPartyCode}")
                .path_parameter("thirdPartyCode", third_party_code.to_string())
                .build(),
            context: Default::default(),
        },
        &oauth_service,
        &user_service,
    )
    .await
    .unwrap();

    assert_eq!(200, response.status_code);
    let token: TokenResponseData =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert_eq!(String::from(raw_access_token), token.access_token);
    assert_eq!("products:write", token.scope);
    assert!(token.third_party_exchange_code.is_none());
    assert!(
        oauth_repository
            .get_third_party_exchange_code_record(&third_party_code)
            .await
            .unwrap()
            .is_none()
    );
}
