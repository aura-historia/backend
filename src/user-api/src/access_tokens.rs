use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::api::error_code::{BAD_BODY_VALUE, BAD_PATH_PARAMETER_VALUE, INVALID_UUID};
use common::error::missing_field::MissingRequiredField;
use common::user_id::api::extract_user_id_request_context;
use lambda_runtime::LambdaEvent;
use std::collections::HashMap;
use user::core::access_token::AccessTokenId;
use user::data::access_token_data::{
    CreatedAccessTokenData, GetAccessTokenData, PatchAccessTokenData, PostAccessTokenData,
};
use user::service::command::{CreateAccessTokenCommand, UpdateAccessTokenCommand};
use user::service::user_service::UserService;

pub async fn post(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl UserService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id = extract_user_id_request_context(&event.payload.request_context)?;
    let body = non_empty_body(event.payload.body)?;
    let data: PostAccessTokenData = serde_json::from_str(&body).map_err(bad_json)?;
    let created: CreatedAccessTokenData = service
        .create_access_token(
            &user_id,
            CreateAccessTokenCommand {
                name: data.name.into(),
                scopes: data.scope,
                expires: data.expires_at,
            },
        )
        .await?
        .into();

    Ok(ApiGatewayV2HttpResponseBuilder::json(201)
        .cache_control("no-store", None, None)
        .body_serde(created)?
        .build())
}

pub async fn get(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl UserService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id = extract_user_id_request_context(&event.payload.request_context)?;
    let tokens: Vec<GetAccessTokenData> = service
        .get_access_tokens(&user_id)
        .await?
        .into_iter()
        .map(Into::into)
        .collect();

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .cache_control("no-store", None, None)
        .body_serde(tokens)?
        .build())
}

pub async fn get_one(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl UserService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id = extract_user_id_request_context(&event.payload.request_context)?;
    let access_token_id = extract_access_token_id_path(&event.payload.path_parameters)?;
    let token: GetAccessTokenData = service
        .find_access_token(&user_id, &access_token_id)
        .await?
        .into();

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .cache_control("no-store", None, None)
        .body_serde(token)?
        .build())
}

pub async fn patch(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl UserService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id = extract_user_id_request_context(&event.payload.request_context)?;
    let body = non_empty_body(event.payload.body)?;
    let data: PatchAccessTokenData = serde_json::from_str(&body).map_err(bad_json)?;
    let updated: GetAccessTokenData = service
        .update_access_token(
            &user_id,
            &data.access_token_id,
            UpdateAccessTokenCommand {
                name: data.name.map(Into::into),
                scopes: data.scope,
                expires: data.expires_at,
            },
        )
        .await?
        .into();

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .cache_control("no-store", None, None)
        .body_serde(updated)?
        .build())
}

pub async fn delete(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl UserService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id = extract_user_id_request_context(&event.payload.request_context)?;
    let access_token_id = event
        .payload
        .query_string_parameters
        .first("accessTokenId")
        .ok_or_else(|| {
            ApiError::bad_request(
                BAD_BODY_VALUE,
                "Missing query parameter accessTokenId".into(),
            )
            .with_detail("Missing query parameter accessTokenId")
        })?
        .to_owned();
    let access_token_id = AccessTokenId::try_from(access_token_id).map_err(|err| {
        ApiError::bad_request(BAD_BODY_VALUE, Box::new(err)).with_detail("Invalid accessTokenId")
    })?;
    service
        .delete_access_token(&user_id, &access_token_id)
        .await?;

    Ok(ApiGatewayV2HttpResponseBuilder::new(204).build())
}

fn non_empty_body(body: Option<String>) -> Result<String, ApiError> {
    body.filter(|body| !body.is_empty()).ok_or_else(|| {
        ApiError::bad_request(BAD_BODY_VALUE, "Body cannot be empty".into())
            .with_detail("Body cannot be empty")
    })
}

fn bad_json(err: serde_json::Error) -> ApiError {
    let err_msg = err.to_string();
    ApiError::bad_request(BAD_BODY_VALUE, Box::new(err)).with_detail(err_msg)
}

fn extract_access_token_id_path(
    path_params: &HashMap<String, String>,
) -> Result<AccessTokenId, ApiError> {
    path_params
        .get("accessTokenId")
        .map(AccessTokenId::try_from)
        .transpose()
        .map_err(|err| {
            let msg = err.to_string();
            ApiError::bad_request(INVALID_UUID, Box::new(err))
                .with_path_field("accessTokenId")
                .with_detail(msg)
        })?
        .ok_or(
            ApiError::bad_request(
                BAD_PATH_PARAMETER_VALUE,
                Box::new(MissingRequiredField::new("accessTokenId")),
            )
            .with_path_field("accessTokenId")
            .with_detail("Missing field 'accessTokenId'."),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::user_id::UserId;
    use fake::{Fake, Faker};
    use lambda_runtime::LambdaEvent;
    use test_api::ApiGatewayV2httpRequestProxy;
    use user::core::access_token::{AccessToken, RawAccessToken, Scope};
    use user::service::user_service::MockUserService;

    #[tokio::test]
    async fn should_create_access_token() {
        let user_id = UserId::new();
        let mut service = MockUserService::default();
        service
            .expect_create_access_token()
            .return_once(move |_, _| {
                let raw = RawAccessToken::new();
                let token: AccessToken = Faker.fake();
                Box::pin(async move { Ok((raw, token)) })
            });
        let event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .route_key("POST /api/v1/me/access-tokens")
                .jwt_claim("sub", user_id)
                .body_serde(&PostAccessTokenData {
                    name: "CI token".to_owned(),
                    scope: [Scope::ProductsWrite].into(),
                    expires_at: None,
                })
                .build(),
            context: Default::default(),
        };

        let response = post(event, &service).await.unwrap();
        assert_eq!(201, response.status_code);
    }

    #[tokio::test]
    async fn should_get_access_tokens() {
        let user_id = UserId::new();
        let mut service = MockUserService::default();
        service.expect_get_access_tokens().return_once(move |_| {
            let token: AccessToken = Faker.fake();
            Box::pin(async move { Ok(vec![token]) })
        });
        let event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/me/access-tokens")
                .jwt_claim("sub", user_id)
                .build(),
            context: Default::default(),
        };

        let response = get(event, &service).await.unwrap();
        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_get_one_access_token() {
        let user_id = UserId::new();
        let token: AccessToken = Faker.fake();
        let access_token_id = token.id;
        let mut service = MockUserService::default();
        service
            .expect_find_access_token()
            .return_once(move |_, _| Box::pin(async move { Ok(token) }));
        let event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/me/access-tokens/{accessTokenId}")
                .jwt_claim("sub", user_id)
                .path_parameter("accessTokenId", access_token_id.to_string())
                .build(),
            context: Default::default(),
        };

        let response = get_one(event, &service).await.unwrap();
        assert_eq!(200, response.status_code);
    }
}
