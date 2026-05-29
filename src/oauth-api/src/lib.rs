use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use base64::Engine as _;
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::{ApiError, log_api_error};
use common::api::error_code::{
    BAD_BODY_VALUE, BAD_PATH_PARAMETER_VALUE, BAD_QUERY_PARAMETER_VALUE, INTERNAL_SERVER_ERROR,
    INVALID_UUID, UNAUTHORIZED,
};
use common::oauth_client_id::OAuthClientId;
use lambda_runtime::LambdaEvent;
use oauth::core::authorization_code::{
    CodeChallengeMethod, OAuthAuthorizationCode, OAuthCodeChallenge, OAuthCodeVerifier,
};
use oauth::core::client::OAuthRedirectUri;
use oauth::data::{
    OAuthClientMetadataPatchData, OAuthClientMetadataRequestData, OAuthClientMetadataResponseData,
};
use oauth::service::oauth_service::{
    AuthorizeRequest, OAuthGrantType, OAuthResponseType, OAuthService, OAuthState,
    TokenIntrospectionRequest, TokenRequest, TokenRevocationRequest,
};
use std::collections::HashSet;
use user::core::access_token::{RawAccessToken, RawOAuthClientSecret, Scope};
use user::data::access_token_data::ScopeData;
use user::service::user_service::UserService;

mod response;

#[tracing::instrument(
    skip(event, service, user_service),
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
    user_service: &(impl UserService + Sync),
) -> Result<ApiGatewayV2httpResponse, lambda_runtime::Error> {
    match handle(event, service, user_service).await {
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
    user_service: &(impl UserService + Sync),
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    match event.payload.route_key.as_deref() {
        Some("POST /api/v1/oauth/clients") => create_client(event, service, user_service).await,
        Some("GET /api/v1/oauth/clients") => get_clients(event, service, user_service).await,
        Some("GET /api/v1/oauth/clients/{clientId}") => {
            get_client(event, service, user_service).await
        }
        Some("PATCH /api/v1/oauth/clients/{clientId}") => {
            update_client(event, service, user_service).await
        }
        Some("DELETE /api/v1/oauth/clients/{clientId}") => {
            delete_client(event, service, user_service).await
        }
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

async fn create_client(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl OAuthService,
    user_service: &impl UserService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id =
        common::user_id::api::extract_user_id_request_context(&event.payload.request_context)?;
    user_service.check_admin(&user_id).await?;
    let data: OAuthClientMetadataRequestData =
        serde_json::from_str(&non_empty_body(event.payload.body)?).map_err(bad_json)?;
    let response: OAuthClientMetadataResponseData =
        service.create_client(&user_id, data.into()).await?.into();
    Ok(ApiGatewayV2HttpResponseBuilder::json(201)
        .location(
            &format!("oauth/clients/{}", response.client_id),
            &event.payload.request_context,
        )
        .cache_control("no-store", None, None)
        .body_serde(response)?
        .build())
}

async fn get_clients(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl OAuthService,
    user_service: &impl UserService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id =
        common::user_id::api::extract_user_id_request_context(&event.payload.request_context)?;
    user_service.check_admin(&user_id).await?;
    let response: Vec<OAuthClientMetadataResponseData> = service
        .get_clients()
        .await?
        .into_iter()
        .map(Into::into)
        .collect();
    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .cache_control("no-store", None, None)
        .body_serde(response)?
        .build())
}

async fn get_client(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl OAuthService,
    user_service: &impl UserService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id =
        common::user_id::api::extract_user_id_request_context(&event.payload.request_context)?;
    user_service.check_admin(&user_id).await?;
    let client_id = extract_client_id_path(&event.payload.path_parameters)?;
    let response: OAuthClientMetadataResponseData = service.get_client(&client_id).await?.into();
    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .cache_control("no-store", None, None)
        .body_serde(response)?
        .build())
}

async fn update_client(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl OAuthService,
    user_service: &impl UserService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id =
        common::user_id::api::extract_user_id_request_context(&event.payload.request_context)?;
    user_service.check_admin(&user_id).await?;
    let client_id = extract_client_id_path(&event.payload.path_parameters)?;
    let data: OAuthClientMetadataPatchData =
        serde_json::from_str(&non_empty_body(event.payload.body)?).map_err(bad_json)?;
    let response: OAuthClientMetadataResponseData =
        service.update_client(&client_id, data.into()).await?.into();
    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .cache_control("no-store", None, None)
        .body_serde(response)?
        .build())
}

async fn delete_client(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl OAuthService,
    user_service: &impl UserService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id =
        common::user_id::api::extract_user_id_request_context(&event.payload.request_context)?;
    user_service.check_admin(&user_id).await?;
    let client_id = extract_client_id_path(&event.payload.path_parameters)?;
    service.delete_client(&client_id).await?;
    Ok(ApiGatewayV2HttpResponseBuilder::new(204).build())
}

async fn authorize(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl OAuthService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id =
        common::user_id::api::extract_user_id_request_context(&event.payload.request_context)?;
    let params = &event.payload.query_string_parameters;
    let request = AuthorizeRequest {
        response_type: parse_response_type(required_query(params, "response_type")?)?,
        client_id: OAuthClientId::try_from(required_query(params, "client_id")?).map_err(
            |err| {
                let msg = err.to_string();
                ApiError::bad_request(INVALID_UUID, Box::new(err))
                    .with_detail(msg)
                    .with_query_field("client_id")
            },
        )?,
        redirect_uri: OAuthRedirectUri::from(required_query(params, "redirect_uri")?),
        scope: parse_scope(params.first("scope"))?,
        state: params.first("state").map(OAuthState::from),
        code_challenge: OAuthCodeChallenge::from(required_query(params, "code_challenge")?),
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
    let response = service.authorize(&user_id, request).await?;
    Ok(response::redirect(&response.redirect_to))
}

async fn token(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl OAuthService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let form = parse_form(event.payload.body, event.payload.is_base64_encoded)?;
    let request = TokenRequest {
        grant_type: parse_grant_type(required_form(&form, "grant_type")?)?,
        code: OAuthAuthorizationCode::try_from(required_form(&form, "code")?.to_owned()).map_err(
            |err| ApiError::bad_request(BAD_BODY_VALUE, Box::new(err)).with_body_field("code"),
        )?,
        redirect_uri: OAuthRedirectUri::from(required_form(&form, "redirect_uri")?),
        client_id: OAuthClientId::try_from(required_form(&form, "client_id")?).map_err(|err| {
            let msg = err.to_string();
            ApiError::bad_request(INVALID_UUID, Box::new(err))
                .with_detail(msg)
                .with_body_field("client_id")
        })?,
        client_secret: RawOAuthClientSecret::try_from(
            required_form(&form, "client_secret")?.to_owned(),
        )
        .map_err(|err| ApiError::unauthorized(UNAUTHORIZED).with_detail(err.to_string()))?,
        code_verifier: OAuthCodeVerifier::from(required_form(&form, "code_verifier")?),
    };
    let response = service.token(request).await?;
    response::json_no_store(200, oauth::data::TokenResponseData::from(response))
}

async fn revoke(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl OAuthService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let form = parse_form(event.payload.body, event.payload.is_base64_encoded)?;
    let request = TokenRevocationRequest {
        token: RawAccessToken::try_from(required_form(&form, "token")?.to_owned()).map_err(
            |err| ApiError::bad_request(BAD_BODY_VALUE, Box::new(err)).with_body_field("token"),
        )?,
        client_id: OAuthClientId::try_from(required_form(&form, "client_id")?).map_err(|err| {
            let msg = err.to_string();
            ApiError::bad_request(INVALID_UUID, Box::new(err))
                .with_detail(msg)
                .with_body_field("client_id")
        })?,
        client_secret: RawOAuthClientSecret::try_from(
            required_form(&form, "client_secret")?.to_owned(),
        )
        .map_err(|err| ApiError::unauthorized(UNAUTHORIZED).with_detail(err.to_string()))?,
    };
    service.revoke(request).await?;
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
    let form = parse_form(event.payload.body, event.payload.is_base64_encoded)?;
    let request = TokenIntrospectionRequest {
        token: RawAccessToken::try_from(required_form(&form, "token")?.to_owned()).map_err(
            |err| ApiError::bad_request(BAD_BODY_VALUE, Box::new(err)).with_body_field("token"),
        )?,
        client_id: OAuthClientId::try_from(required_form(&form, "client_id")?).map_err(|err| {
            let msg = err.to_string();
            ApiError::bad_request(INVALID_UUID, Box::new(err))
                .with_detail(msg)
                .with_body_field("client_id")
        })?,
        client_secret: RawOAuthClientSecret::try_from(
            required_form(&form, "client_secret")?.to_owned(),
        )
        .map_err(|err| ApiError::unauthorized(UNAUTHORIZED).with_detail(err.to_string()))?,
    };
    let response = service.introspect(request).await?;
    response::json_no_store(200, oauth::data::IntrospectionResponseData::from(response))
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

fn extract_client_id_path(
    path_parameters: &std::collections::HashMap<String, String>,
) -> Result<OAuthClientId, ApiError> {
    path_parameters
        .get("clientId")
        .map(OAuthClientId::try_from)
        .transpose()
        .map_err(|err| {
            let msg = err.to_string();
            ApiError::bad_request(INVALID_UUID, Box::new(err))
                .with_detail(msg)
                .with_path_field("clientId")
        })?
        .ok_or_else(|| {
            ApiError::bad_request(
                BAD_PATH_PARAMETER_VALUE,
                "Missing path parameter clientId".into(),
            )
            .with_detail("Missing path parameter clientId")
            .with_path_field("clientId")
        })
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

fn parse_form(
    body: Option<String>,
    is_base64_encoded: bool,
) -> Result<std::collections::HashMap<String, String>, ApiError> {
    let body_value = body
        .filter(|body| !body.is_empty())
        .ok_or_else(|| ApiError::bad_request(BAD_BODY_VALUE, "Body cannot be empty".into()))?;
    let bytes = if is_base64_encoded {
        base64::engine::general_purpose::STANDARD
            .decode(body_value.as_bytes())
            .map_err(|err| ApiError::bad_request(BAD_BODY_VALUE, Box::new(err)))?
    } else {
        body_value.into_bytes()
    };
    Ok(url::form_urlencoded::parse(&bytes).into_owned().collect())
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

fn parse_response_type(value: &str) -> Result<OAuthResponseType, ApiError> {
    match value {
        "code" => Ok(OAuthResponseType::Code),
        value => Err(ApiError::bad_request(
            BAD_QUERY_PARAMETER_VALUE,
            format!("Unsupported response_type '{value}'").into(),
        )
        .with_query_field("response_type")),
    }
}

fn parse_grant_type(value: &str) -> Result<OAuthGrantType, ApiError> {
    match value {
        "authorization_code" => Ok(OAuthGrantType::AuthorizationCode),
        value => Err(ApiError::bad_request(
            BAD_BODY_VALUE,
            format!("Unsupported grant_type '{value}'").into(),
        )
        .with_body_field("grant_type")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lambda_runtime::LambdaEvent;
    use oauth::core::client::{OAuthClient, OAuthClientName};
    use oauth::service::oauth_service::{
        AuthorizeResponse, IntrospectionResponse, MockOAuthService, OAuthTokenType, TokenResponse,
    };
    use test_api::ApiGatewayV2httpRequestProxy;
    use user::service::user_service::{MockUserService, UserServiceError};

    fn client_id() -> OAuthClientId {
        OAuthClientId::try_from("018f6e7a-8b9c-7d0e-8f12-3456789abcde").unwrap()
    }

    fn oauth_client(user_id: common::user_id::UserId) -> OAuthClient {
        let secret = RawOAuthClientSecret::new();
        let now = time::OffsetDateTime::now_utc();
        OAuthClient {
            client_id: client_id(),
            hashed_client_secret: secret.into(),
            name: OAuthClientName::from("Client"),
            redirect_uris: HashSet::from([OAuthRedirectUri::from(
                "https://client.example/callback",
            )]),
            scopes: HashSet::from([Scope::ProductsWrite]),
            created_by: user_id,
            created: now,
            updated: now,
        }
    }

    fn create_client_event(
        user_id: common::user_id::UserId,
    ) -> LambdaEvent<ApiGatewayV2httpRequest> {
        LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .route_key("POST /api/v1/oauth/clients")
                .jwt_claim("sub", user_id)
                .body_serde(&OAuthClientMetadataRequestData {
                    client_name: "Client".to_owned(),
                    redirect_uris: HashSet::from(["https://client.example/callback".to_owned()]),
                    scope: HashSet::from([ScopeData::ProductsWrite]),
                })
                .build(),
            context: Default::default(),
        }
    }

    fn admin_ok(user_id: common::user_id::UserId) -> MockUserService {
        let mut user_service = MockUserService::default();
        user_service
            .expect_check_admin()
            .return_once(move |actual_user_id| {
                assert_eq!(&user_id, actual_user_id);
                Box::pin(async { Ok(()) })
            });
        user_service
    }

    #[tokio::test]
    async fn should_create_client_metadata() {
        let user_id = common::user_id::UserId::new();
        let mut service = MockOAuthService::default();
        let user_service = admin_ok(user_id);
        service
            .expect_create_client()
            .return_once(move |actual_user_id, request| {
                assert_eq!(&user_id, actual_user_id);
                assert_eq!(OAuthClientName::from("Client"), request.name);
                Box::pin(async move { Ok((RawOAuthClientSecret::new(), oauth_client(user_id))) })
            });
        let event = create_client_event(user_id);

        let response = create_client(event, &service, &user_service).await.unwrap();
        assert_eq!(201, response.status_code);
        assert!(response.headers.contains_key(http::header::LOCATION));
        let body = match response.body.unwrap() {
            aws_lambda_events::encodings::Body::Text(body) => body,
            body => panic!("unexpected response body: {body:?}"),
        };
        assert!(body.contains("client_secret"));
    }

    #[tokio::test]
    async fn should_get_all_client_metadata() {
        let user_id = common::user_id::UserId::new();
        let mut service = MockOAuthService::default();
        let user_service = admin_ok(user_id);
        service
            .expect_get_clients()
            .return_once(move || Box::pin(async move { Ok(vec![oauth_client(user_id)]) }));
        let event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/oauth/clients")
                .jwt_claim("sub", user_id)
                .build(),
            context: Default::default(),
        };

        let response = get_clients(event, &service, &user_service).await.unwrap();

        assert_eq!(200, response.status_code);
        let body = match response.body.unwrap() {
            aws_lambda_events::encodings::Body::Text(body) => body,
            body => panic!("unexpected response body: {body:?}"),
        };
        assert!(body.contains("client_secret"));
    }

    #[tokio::test]
    async fn should_get_client_metadata() {
        let user_id = common::user_id::UserId::new();
        let mut service = MockOAuthService::default();
        let user_service = admin_ok(user_id);
        service
            .expect_get_client()
            .return_once(move |actual_client_id| {
                assert_eq!(&client_id(), actual_client_id);
                Box::pin(async move { Ok(oauth_client(user_id)) })
            });
        let event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/oauth/clients/{clientId}")
                .jwt_claim("sub", user_id)
                .path_parameter("clientId", client_id().to_string())
                .build(),
            context: Default::default(),
        };

        let response = get_client(event, &service, &user_service).await.unwrap();
        assert_eq!(200, response.status_code);
        let body = match response.body.unwrap() {
            aws_lambda_events::encodings::Body::Text(body) => body,
            body => panic!("unexpected response body: {body:?}"),
        };
        assert!(body.contains("client_secret"));
    }

    #[tokio::test]
    async fn should_update_client_metadata() {
        let user_id = common::user_id::UserId::new();
        let mut service = MockOAuthService::default();
        let user_service = admin_ok(user_id);
        service
            .expect_update_client()
            .return_once(move |actual_client_id, command| {
                assert_eq!(&client_id(), actual_client_id);
                assert_eq!(Some(OAuthClientName::from("Updated")), command.name);
                Box::pin(async move { Ok(oauth_client(user_id)) })
            });
        let event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .route_key("PATCH /api/v1/oauth/clients/{clientId}")
                .jwt_claim("sub", user_id)
                .path_parameter("clientId", client_id().to_string())
                .body_serde(&OAuthClientMetadataPatchData {
                    client_name: Some("Updated".to_owned()),
                    redirect_uris: None,
                    scope: None,
                })
                .build(),
            context: Default::default(),
        };

        let response = update_client(event, &service, &user_service).await.unwrap();

        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_delete_client_metadata() {
        let user_id = common::user_id::UserId::new();
        let mut service = MockOAuthService::default();
        let user_service = admin_ok(user_id);
        service
            .expect_delete_client()
            .return_once(|actual_client_id| {
                assert_eq!(&client_id(), actual_client_id);
                Box::pin(async { Ok(()) })
            });
        let event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::DELETE)
                .route_key("DELETE /api/v1/oauth/clients/{clientId}")
                .jwt_claim("sub", user_id)
                .path_parameter("clientId", client_id().to_string())
                .build(),
            context: Default::default(),
        };

        let response = delete_client(event, &service, &user_service).await.unwrap();

        assert_eq!(204, response.status_code);
    }

    #[tokio::test]
    async fn should_reject_client_metadata_for_non_admin() {
        let user_id = common::user_id::UserId::new();
        let service = MockOAuthService::default();
        let mut user_service = MockUserService::default();
        user_service
            .expect_check_admin()
            .return_once(|_| Box::pin(async { Err(UserServiceError::AdminRoleRequired) }));

        let err = create_client(create_client_event(user_id), &service, &user_service)
            .await
            .unwrap_err();

        assert_eq!(403, err.status);
    }

    #[tokio::test]
    async fn should_reject_invalid_client_metadata_body() {
        let user_id = common::user_id::UserId::new();
        let service = MockOAuthService::default();
        let user_service = admin_ok(user_id);
        let mut payload = ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .route_key("POST /api/v1/oauth/clients")
            .jwt_claim("sub", user_id)
            .build();
        payload.body = Some("{".to_owned());
        let event = LambdaEvent {
            payload,
            context: Default::default(),
        };

        let err = create_client(event, &service, &user_service)
            .await
            .unwrap_err();

        assert_eq!(400, err.status);
    }

    #[tokio::test]
    async fn should_map_client_metadata_service_errors() {
        let user_id = common::user_id::UserId::new();
        let mut service = MockOAuthService::default();
        let user_service = admin_ok(user_id);
        service.expect_get_clients().return_once(|| {
            Box::pin(async {
                Err(oauth::service::oauth_service::OAuthServiceError::ClientForbidden)
            })
        });
        let event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/oauth/clients")
                .jwt_claim("sub", user_id)
                .build(),
            context: Default::default(),
        };

        let err = get_clients(event, &service, &user_service)
            .await
            .unwrap_err();

        assert_eq!(403, err.status);
    }

    #[tokio::test]
    async fn should_reject_invalid_client_id_path() {
        let user_id = common::user_id::UserId::new();
        let service = MockOAuthService::default();
        let user_service = admin_ok(user_id);
        let event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/oauth/clients/{clientId}")
                .jwt_claim("sub", user_id)
                .path_parameter("clientId", "not-a-uuid")
                .build(),
            context: Default::default(),
        };

        let err = get_client(event, &service, &user_service)
            .await
            .unwrap_err();

        assert_eq!(400, err.status);
    }

    #[tokio::test]
    async fn should_reject_empty_update_client_body() {
        let user_id = common::user_id::UserId::new();
        let service = MockOAuthService::default();
        let user_service = admin_ok(user_id);
        let event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .route_key("PATCH /api/v1/oauth/clients/{clientId}")
                .jwt_claim("sub", user_id)
                .path_parameter("clientId", client_id().to_string())
                .build(),
            context: Default::default(),
        };

        let err = update_client(event, &service, &user_service)
            .await
            .unwrap_err();

        assert_eq!(400, err.status);
    }

    #[tokio::test]
    async fn should_map_delete_client_service_errors() {
        let user_id = common::user_id::UserId::new();
        let mut service = MockOAuthService::default();
        let user_service = admin_ok(user_id);
        service.expect_delete_client().return_once(|_| {
            Box::pin(async {
                Err(oauth::service::oauth_service::OAuthServiceError::ClientNotFound)
            })
        });
        let event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::DELETE)
                .route_key("DELETE /api/v1/oauth/clients/{clientId}")
                .jwt_claim("sub", user_id)
                .path_parameter("clientId", client_id().to_string())
                .build(),
            context: Default::default(),
        };

        let err = delete_client(event, &service, &user_service)
            .await
            .unwrap_err();

        assert_eq!(401, err.status);
    }

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
                .query_string_parameter("client_id", client_id().to_string())
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
        let secret = RawOAuthClientSecret::new();
        let token_value: String = RawAccessToken::new().into();
        let mut service = MockOAuthService::default();
        service.expect_token().return_once(move |_| {
            Box::pin(async move {
                Ok(TokenResponse {
                    access_token: RawAccessToken::try_from(token_value).unwrap(),
                    token_type: OAuthTokenType::Bearer,
                    expires: Some(time::OffsetDateTime::now_utc() + time::Duration::hours(1)),
                    scopes: HashSet::from([Scope::ProductsWrite]),
                })
            })
        });
        let body = format!(
            "grant_type=authorization_code&code={}&redirect_uri={}&client_id={}&client_secret={}&code_verifier=verifier",
            oauth::core::authorization_code::OAuthAuthorizationCode::new(),
            url::form_urlencoded::byte_serialize(b"https://client.example/callback")
                .collect::<String>(),
            client_id(),
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
        let secret = RawOAuthClientSecret::new();
        let mut service = MockOAuthService::default();
        service.expect_introspect().return_once(|_| {
            Box::pin(async {
                Ok(IntrospectionResponse {
                    active: false,
                    scopes: None,
                    client_id: None,
                    subject: None,
                    token_type: None,
                    expires: None,
                    issued_at: None,
                })
            })
        });
        let body = format!(
            "token={}&client_id={}&client_secret={}",
            url::form_urlencoded::byte_serialize(String::from(token_value).as_bytes())
                .collect::<String>(),
            client_id(),
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
