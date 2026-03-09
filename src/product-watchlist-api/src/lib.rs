use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::{
    error::{ApiError, log_api_error},
    error_code::INTERNAL_SERVER_ERROR,
};
use lambda_runtime::LambdaEvent;
use product_watchlist::service::product_watchlist_service::ProductWatchListService;

pub mod watchlist_delete;
pub mod watchlist_get;
pub mod watchlist_patch;
pub mod watchlist_post;

#[tracing::instrument(
    skip(event, product_watchlist_service),
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
#[allow(clippy::too_many_arguments)]
pub async fn handler(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    product_watchlist_service: &impl ProductWatchListService,
) -> Result<ApiGatewayV2httpResponse, lambda_runtime::Error> {
    match handle(event, product_watchlist_service).await {
        Ok(response) => Ok(response),
        Err(err) => {
            log_api_error(&err);
            Ok(ApiGatewayV2httpResponse::from(err))
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    product_watchlist_service: &impl ProductWatchListService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    match event.payload.route_key.as_deref() {
        Some("DELETE /api/v1/me/watchlist/{shopId}/{shopsProductId}") => {
            watchlist_delete::handle(event, product_watchlist_service).await
        }
        Some("GET /api/v1/me/watchlist") => {
            watchlist_get::handle(event, product_watchlist_service).await
        }
        Some("PATCH /api/v1/me/watchlist/{shopId}/{shopsProductId}") => {
            watchlist_patch::handle(event, product_watchlist_service).await
        }
        Some("POST /api/v1/me/watchlist") => {
            watchlist_post::handle(event, product_watchlist_service).await
        }
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
