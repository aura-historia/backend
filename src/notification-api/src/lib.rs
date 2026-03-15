use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::{
    error::{ApiError, log_api_error},
    error_code::INTERNAL_SERVER_ERROR,
};
use lambda_runtime::LambdaEvent;
use notification::service::notification_service::NotificationService;

pub mod notification_delete_all;
pub mod notification_delete_one;
pub mod notification_get;
pub mod notification_patch_all;
pub mod notification_patch_one;

#[tracing::instrument(
    skip(event, notification_service),
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
    notification_service: &impl NotificationService,
) -> Result<ApiGatewayV2httpResponse, lambda_runtime::Error> {
    match handle(event, notification_service).await {
        Ok(response) => Ok(response),
        Err(err) => {
            log_api_error(&err);
            Ok(ApiGatewayV2httpResponse::from(err))
        }
    }
}

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    notification_service: &impl NotificationService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    match event.payload.route_key.as_deref() {
        Some("DELETE /api/v1/me/notifications") => {
            notification_delete_all::handle(event, notification_service).await
        }
        Some("DELETE /api/v1/me/notifications/{eventId}") => {
            notification_delete_one::handle(event, notification_service).await
        }
        Some("GET /api/v1/me/notifications") => {
            notification_get::handle(event, notification_service).await
        }
        Some("PATCH /api/v1/me/notifications") => {
            notification_patch_all::handle(event, notification_service).await
        }
        Some("PATCH /api/v1/me/notifications/{eventId}") => {
            notification_patch_one::handle(event, notification_service).await
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
