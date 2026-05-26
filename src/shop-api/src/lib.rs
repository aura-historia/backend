use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use cognito::access_token_verifier_service::AccessTokenVerifierService;
use common::api::error::{ApiError, log_api_error};
use common::api::error_code::INTERNAL_SERVER_ERROR;
use lambda_runtime::LambdaEvent;
use shop::service::command_service::CommandShopService;
use shop::service::get_service::GetShopService;
use shop::service::query_service::QueryShopService;
use user::service::user_service::UserService;

pub mod get;
pub mod patch;
pub mod post;
pub mod search;

#[tracing::instrument(
    skip(event, get_shop_service, query_shop_service, command_shop_service, user_service, access_token_verifier_service),
    fields(
        requestId = %event.context.request_id,
        method = event.payload.request_context.http.method.as_str(),
        path = &event.payload.raw_path.as_deref().unwrap_or("NULL"),
        query = &event.payload.raw_query_string.as_deref().unwrap_or("NULL"),
        body = &event.payload.body.as_deref().unwrap_or("NULL"),
        ip = &event.payload.request_context.http.source_ip.as_deref().unwrap_or("NULL"),
        userAgent = &event.payload.request_context.http.user_agent.as_deref().unwrap_or("NULL"),
        userId = tracing::field::Empty,
    )
)]
pub async fn handler(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    get_shop_service: &(impl GetShopService + Sync),
    query_shop_service: &(impl QueryShopService + Sync),
    command_shop_service: &(impl CommandShopService + Sync),
    user_service: &(impl UserService + Sync),
    access_token_verifier_service: &(impl AccessTokenVerifierService + Sync),
) -> Result<ApiGatewayV2httpResponse, lambda_runtime::Error> {
    match handle(
        event,
        get_shop_service,
        query_shop_service,
        command_shop_service,
        user_service,
        access_token_verifier_service,
    )
    .await
    {
        Ok(response) => Ok(response),
        Err(err) => {
            log_api_error(&err);
            Ok(ApiGatewayV2httpResponse::from(err))
        }
    }
}

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    get_shop_service: &(impl GetShopService + Sync),
    query_shop_service: &(impl QueryShopService + Sync),
    command_shop_service: &(impl CommandShopService + Sync),
    user_service: &(impl UserService + Sync),
    access_token_verifier_service: &(impl AccessTokenVerifierService + Sync),
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    match event.payload.route_key.as_deref() {
        Some("GET /api/v1/shops/{shopId}")
        | Some("GET /api/v1/by-slug/shops/{shopSlugId}")
        | Some("GET /api/v1/by-domain/shops/{shopDomain}") => {
            get::handle(event, get_shop_service, access_token_verifier_service).await
        }
        Some("POST /api/v1/shops/search") | Some("GET /api/v1/shops") => {
            search::handle(event, query_shop_service, access_token_verifier_service).await
        }
        Some("PATCH /api/v1/shops/{shopId}") => {
            patch::handle(
                event,
                command_shop_service,
                get_shop_service,
                user_service,
                access_token_verifier_service,
            )
            .await
        }
        Some("POST /api/v1/shops") => post::handle(event, command_shop_service, user_service).await,
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
