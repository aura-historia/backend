use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use test_api::*;
use user::data::access_token_data::{
    GetAccessTokenData, PatchAccessTokenData, PostAccessTokenData, ScopeData,
};
use user::dynamodb::repository::UserDynamoDbRepositoryImpl;
use user::service::user_service::{UserService, UserServiceImpl};
use user_api::handler;

fn system_ctx() -> common::actor::RequestContext {
    common::actor::RequestContext {
        actor: common::actor::domain::Actor::System,
    }
}

#[localstack_test(services = [DynamoDB()])]
async fn should_crud_access_tokens() {
    let repository = UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let service = UserServiceImpl::new(&repository);
    let user = service
        .create_user(&system_ctx(), Faker.fake())
        .await
        .unwrap();

    let create = PostAccessTokenData {
        name: "Integration token".to_owned(),
        scope: [ScopeData::ProductsWrite].into(),
        expires_at: None,
    };
    let response = handler(
        LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .route_key("POST /api/v1/me/access-tokens")
                .jwt_claim("sub", user.user_id)
                .body_serde(&create)
                .build(),
            context: Default::default(),
        },
        &service,
    )
    .await
    .unwrap();
    assert_eq!(201, response.status_code);
    let created: GetAccessTokenData =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    let access_token_id = created.access_token_id;

    let response = handler(
        LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/me/access-tokens")
                .jwt_claim("sub", user.user_id)
                .build(),
            context: Default::default(),
        },
        &service,
    )
    .await
    .unwrap();
    assert_eq!(200, response.status_code);
    let tokens: Vec<GetAccessTokenData> =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert_eq!(1, tokens.len());

    let response = handler(
        LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/me/access-tokens/{accessTokenId}")
                .jwt_claim("sub", user.user_id)
                .path_parameter("accessTokenId", access_token_id.to_string())
                .build(),
            context: Default::default(),
        },
        &service,
    )
    .await
    .unwrap();
    assert_eq!(200, response.status_code);
    let token: GetAccessTokenData =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert_eq!(access_token_id, token.access_token_id);

    let patch = PatchAccessTokenData {
        access_token_id,
        name: Some("Renamed token".to_owned()),
        scope: Some(HashSet::from([ScopeData::ProductsWrite])),
        expires_at: None,
    };
    let response = handler(
        LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .route_key("PATCH /api/v1/me/access-tokens")
                .jwt_claim("sub", user.user_id)
                .body_serde(&patch)
                .build(),
            context: Default::default(),
        },
        &service,
    )
    .await
    .unwrap();
    assert_eq!(200, response.status_code);
    let updated: GetAccessTokenData =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert_eq!("Renamed token", updated.name);

    let response = handler(
        LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::DELETE)
                .route_key("DELETE /api/v1/me/access-tokens/{accessTokenId}")
                .jwt_claim("sub", user.user_id)
                .path_parameter("accessTokenId", access_token_id.to_string())
                .build(),
            context: Default::default(),
        },
        &service,
    )
    .await
    .unwrap();
    assert_eq!(204, response.status_code);
}
