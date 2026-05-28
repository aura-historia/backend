use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::error::{ApiError, log_api_error};
use common::api::error_code::{
    BAD_BODY_VALUE, BAD_QUERY_PARAMETER_VALUE, INTERNAL_SERVER_ERROR, UNAUTHORIZED,
};
use lambda_runtime::LambdaEvent;
use oauth::core::authorization_code::{CodeChallengeMethod, OAuthAuthorizationCode};
use oauth::service::oauth_service::{
    AuthorizeRequest, OAuthService, OAuthServiceError, TokenIntrospectionRequest, TokenRequest,
    TokenRevocationRequest,
};
use std::collections::HashSet;
use user::core::access_token::{RawAccessToken, Scope};
use user::data::access_token_data::ScopeData;

mod response;

#[tracing::instrument(
    skip(event, service),
    fields(
        requestId = %event.context.request_id,
        method = event.payload.request_context.http.method.as_str(),
        path = &event.payload.raw_path.as_deref().unwrap_or("NULL"),
        query = &event.payload.raw_query_string.as_deref().unwrap_or("NULL"),
        body = &event.payload.body.as_deref().unwrap_or("NULL"),
        ip = &event.payload.request_context.http.source_ip.as_deref().unwrap_or("NULL"),
        userAgent = &event.payload.request_context.http.user_agent.as_deref().unwrap_or("NULL"),
    )
)]
pub async fn handler(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &(impl OAuthService + Sync),
) -> Result<ApiGatewayV2httpResponse, lambda_runtime::Error> {
    match handle(event, service).await {
        Ok(response) => Ok(response),
        Err(err) => {
            log_api_error(&err);
            Ok(ApiGatewayV2httpResponse::from(err))
        }
    }
}

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &(impl OAuthService + Sync),
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    match event.payload.route_key.as_deref() {
        Some("GET /api/v1/oauth/authorize") => authorize(event, service).await,
        Some("POST /api/v1/oauth/token") => token(event, service).await,
        Some("POST /api/v1/oauth/revoke") => revoke(event, service).await,
        Some("POST /api/v1/oauth/introspect") => introspect(event, service).await,
        Some(unknown) => Err(ApiError::internal_server_error(
            INTERNAL_SERVER_ERROR,
            format!("Unknown route-key '{unknown}' in AWS-Payload").into(),
        )),
        None => Err(ApiError::internal_server_error(
            INTERNAL_SERVER_ERROR,
            "Missing route-key in AWS-Payload".into(),
        )),
    }
}

async fn authorize(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl OAuthService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id =
        common::user_id::api::extract_user_id_request_context(&event.payload.request_context)?;
    let params = &event.payload.query_string_parameters;
    let request = AuthorizeRequest {
        response_type: required_query(params, "response_type")?.to_owned(),
        client_id: required_query(params, "client_id")?.to_owned().into(),
        redirect_uri: required_query(params, "redirect_uri")?.to_owned(),
        scope: parse_scope(params.first("scope"))?,
        state: params.first("state").map(ToOwned::to_owned),
        code_challenge: required_query(params, "code_challenge")?.to_owned(),
        code_challenge_method: match required_query(params, "code_challenge_method")? {
            "S256" => CodeChallengeMethod::S256,
            value => {
                return Err(ApiError::bad_request(
                    BAD_QUERY_PARAMETER_VALUE,
                    format!("Unsupported code_challenge_method '{value}'").into(),
                )
                .with_query_field("code_challenge_method"));
            }
        },
    };
    let response = service
        .authorize(&user_id, request)
        .await
        .map_err(oauth_error)?;
    Ok(response::redirect(&response.redirect_to))
}

async fn token(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl OAuthService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let form = parse_form(event.payload.body)?;
    let request = TokenRequest {
        grant_type: required_form(&form, "grant_type")?.to_owned(),
        code: OAuthAuthorizationCode::try_from(required_form(&form, "code")?.to_owned()).map_err(
            |err| ApiError::bad_request(BAD_BODY_VALUE, Box::new(err)).with_body_field("code"),
        )?,
        redirect_uri: required_form(&form, "redirect_uri")?.to_owned(),
        client_id: required_form(&form, "client_id")?.to_owned().into(),
        client_secret: RawAccessToken::try_from(required_form(&form, "client_secret")?.to_owned())
            .map_err(|err| ApiError::unauthorized(UNAUTHORIZED).with_detail(err.to_string()))?,
        code_verifier: required_form(&form, "code_verifier")?.to_owned(),
    };
    let response = service.token(request).await.map_err(oauth_error)?;
    response::json_no_store(200, response)
}

async fn revoke(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl OAuthService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let form = parse_form(event.payload.body)?;
    let request = TokenRevocationRequest {
        token: RawAccessToken::try_from(required_form(&form, "token")?.to_owned()).map_err(
            |err| ApiError::bad_request(BAD_BODY_VALUE, Box::new(err)).with_body_field("token"),
        )?,
        client_id: required_form(&form, "client_id")?.to_owned().into(),
        client_secret: RawAccessToken::try_from(required_form(&form, "client_secret")?.to_owned())
            .map_err(|err| ApiError::unauthorized(UNAUTHORIZED).with_detail(err.to_string()))?,
    };
    service.revoke(request).await.map_err(oauth_error)?;
    Ok(
        common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder::new(
            200,
        )
        .cache_control("no-store", None, None)
        .build(),
    )
}

async fn introspect(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl OAuthService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let form = parse_form(event.payload.body)?;
    let request = TokenIntrospectionRequest {
        token: RawAccessToken::try_from(required_form(&form, "token")?.to_owned()).map_err(
            |err| ApiError::bad_request(BAD_BODY_VALUE, Box::new(err)).with_body_field("token"),
        )?,
        client_id: required_form(&form, "client_id")?.to_owned().into(),
        client_secret: RawAccessToken::try_from(required_form(&form, "client_secret")?.to_owned())
            .map_err(|err| ApiError::unauthorized(UNAUTHORIZED).with_detail(err.to_string()))?,
    };
    let response = service.introspect(request).await.map_err(oauth_error)?;
    response::json_no_store(200, response)
}

fn required_query<'a>(
    params: &'a aws_lambda_events::query_map::QueryMap,
    key: &'static str,
) -> Result<&'a str, ApiError> {
    params.first(key).ok_or_else(|| {
        ApiError::bad_request(
            BAD_QUERY_PARAMETER_VALUE,
            format!("Missing query parameter {key}").into(),
        )
        .with_query_field(key)
    })
}

fn parse_form(body: Option<String>) -> Result<std::collections::HashMap<String, String>, ApiError> {
    let body = body
        .filter(|body| !body.is_empty())
        .ok_or_else(|| ApiError::bad_request(BAD_BODY_VALUE, "Body cannot be empty".into()))?;
    Ok(url::form_urlencoded::parse(body.as_bytes())
        .into_owned()
        .collect())
}

fn required_form<'a>(
    form: &'a std::collections::HashMap<String, String>,
    key: &'static str,
) -> Result<&'a str, ApiError> {
    form.get(key).map(String::as_str).ok_or_else(|| {
        ApiError::bad_request(BAD_BODY_VALUE, format!("Missing form field {key}").into())
            .with_body_field(key)
    })
}

fn parse_scope(value: Option<&str>) -> Result<HashSet<Scope>, ApiError> {
    value
        .unwrap_or("")
        .split_whitespace()
        .map(|scope| {
            ScopeData::try_from(scope).map(Into::into).map_err(|err| {
                ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE, err.into())
                    .with_query_field("scope")
            })
        })
        .collect()
}

fn oauth_error(err: OAuthServiceError) -> ApiError {
    match err {
        OAuthServiceError::InvalidClientSecret | OAuthServiceError::ClientNotFound => {
            ApiError::unauthorized(UNAUTHORIZED).with_detail(err.to_string())
        }
        OAuthServiceError::UnsupportedResponseType(_)
        | OAuthServiceError::InvalidRedirectUri
        | OAuthServiceError::InvalidScope => {
            ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE, Box::new(err))
        }
        OAuthServiceError::UnsupportedGrantType(_)
        | OAuthServiceError::AuthorizationCodeNotFound
        | OAuthServiceError::AuthorizationCodeExpired
        | OAuthServiceError::AuthorizationCodeClientMismatch
        | OAuthServiceError::AuthorizationCodeRedirectUriMismatch
        | OAuthServiceError::InvalidCodeVerifier => {
            ApiError::bad_request(BAD_BODY_VALUE, Box::new(err))
        }
        OAuthServiceError::SdkGetItemError(sdk_error) => sdk_error.into(),
        OAuthServiceError::SdkPutItemError(sdk_error) => sdk_error.into(),
        OAuthServiceError::SdkDeleteItemError(sdk_error) => sdk_error.into(),
        OAuthServiceError::UserServiceError(user_err) => user_err.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lambda_runtime::LambdaEvent;
    use oauth::data::{IntrospectionResponseData, TokenResponseData};
    use oauth::service::oauth_service::{AuthorizeResponse, MockOAuthService};
    use test_api::ApiGatewayV2httpRequestProxy;
    use user::data::access_token_data::AccessTokenTypeData;

    #[tokio::test]
    async fn should_authorize() {
        let user_id = common::user_id::UserId::new();
        let mut service = MockOAuthService::default();
        service.expect_authorize().return_once(|_, _| {
            Box::pin(async {
                Ok(AuthorizeResponse {
                    redirect_to: "https://client.example/callback?code=abc".to_owned(),
                })
            })
        });
        let event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/oauth/authorize")
                .jwt_claim("sub", user_id)
                .query_string_parameter("response_type", "code")
                .query_string_parameter("client_id", "client_1")
                .query_string_parameter("redirect_uri", "https://client.example/callback")
                .query_string_parameter("scope", "products:write")
                .query_string_parameter("code_challenge", "challenge")
                .query_string_parameter("code_challenge_method", "S256")
                .build(),
            context: Default::default(),
        };

        let response = authorize(event, &service).await.unwrap();
        assert_eq!(302, response.status_code);
        assert_eq!(
            "https://client.example/callback?code=abc",
            response.headers.get(http::header::LOCATION).unwrap()
        );
    }

    #[tokio::test]
    async fn should_exchange_token() {
        let secret = RawAccessToken::new();
        let token_value: String = RawAccessToken::new().into();
        let mut service = MockOAuthService::default();
        service.expect_token().return_once(move |_| {
            Box::pin(async move {
                Ok(TokenResponseData {
                    access_token: token_value,
                    token_type: AccessTokenTypeData::Bearer,
                    expires_in: Some(3600),
                    scope: "products:write".to_owned(),
                })
            })
        });
        let body = format!(
            "grant_type=authorization_code&code={}&redirect_uri={}&client_id=client_1&client_secret={}&code_verifier=verifier",
            oauth::core::authorization_code::OAuthAuthorizationCode::new(),
            url::form_urlencoded::byte_serialize(b"https://client.example/callback")
                .collect::<String>(),
            url::form_urlencoded::byte_serialize(String::from(secret).as_bytes())
                .collect::<String>(),
        );
        let mut payload = ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .route_key("POST /api/v1/oauth/token")
            .build();
        payload.body = Some(body);
        let event = LambdaEvent {
            payload,
            context: Default::default(),
        };

        let response = token(event, &service).await.unwrap();
        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_introspect_inactive_token() {
        let token_value = RawAccessToken::new();
        let secret = RawAccessToken::new();
        let mut service = MockOAuthService::default();
        service.expect_introspect().return_once(|_| {
            Box::pin(async {
                Ok(IntrospectionResponseData {
                    active: false,
                    scope: None,
                    client_id: None,
                    sub: None,
                    token_type: None,
                    exp: None,
                    iat: None,
                })
            })
        });
        let body = format!(
            "token={}&client_id=client_1&client_secret={}",
            url::form_urlencoded::byte_serialize(String::from(token_value).as_bytes())
                .collect::<String>(),
            url::form_urlencoded::byte_serialize(String::from(secret).as_bytes())
                .collect::<String>(),
        );
        let mut payload = ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .route_key("POST /api/v1/oauth/introspect")
            .build();
        payload.body = Some(body);
        let event = LambdaEvent {
            payload,
            context: Default::default(),
        };

        let response = introspect(event, &service).await.unwrap();
        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_reject_empty_token_body() {
        let service = MockOAuthService::default();
        let event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .route_key("POST /api/v1/oauth/token")
                .build(),
            context: Default::default(),
        };

        let err = token(event, &service).await.unwrap_err();
        assert_eq!(400, err.status);
    }
}
