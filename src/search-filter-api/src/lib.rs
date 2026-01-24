use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::{
    error::{ApiError, log_api_error},
    error_code::INTERNAL_SERVER_ERROR,
};
use lambda_runtime::LambdaEvent;
use search_filter::service::user_search_filter_service::UserSearchFilterService;

mod delete;
mod get_all;
mod get_one;
mod patch;
pub mod patch_types;
mod post;
pub mod post_types;

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
        userId = tracing::field::Empty,
    )
)]
pub async fn handler(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl UserSearchFilterService,
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
    service: &impl UserSearchFilterService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    match event.payload.route_key.as_deref() {
        Some("GET /api/v1/me/search-filters") => get_all::handle(event, service).await,
        Some("POST /api/v1/me/search-filters") => post::handle(event, service).await,
        Some("GET /api/v1/me/search-filters/{userSearchFilterId}") => {
            get_one::handle(event, service).await
        }
        Some("PATCH /api/v1/me/search-filters/{userSearchFilterId}") => {
            patch::handle(event, service).await
        }
        Some("DELETE /api/v1/me/search-filters/{userSearchFilterId}") => {
            delete::handle(event, service).await
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
