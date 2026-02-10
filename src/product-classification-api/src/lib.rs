use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::error::{ApiError, log_api_error};
use common::api::error_code::{INTERNAL_SERVER_ERROR, NOT_FOUND};
use lambda_runtime::LambdaEvent;
use product_classification::category::service::{CategoryService, CategoryServiceError};

pub mod category;

pub(crate) fn category_service_error_to_api_error(err: CategoryServiceError) -> ApiError {
    match err {
        CategoryServiceError::CategoryNotExists(_) => ApiError::not_found(NOT_FOUND, Box::new(err)),
        CategoryServiceError::OpenSearchError(e) => e.into(),
        CategoryServiceError::DynamoDbSdkPutItemError(e) => e.into(),
        CategoryServiceError::DynamoDbSdkGetItemError(e) => e.into(),
        CategoryServiceError::DynamoDbSdkQueryError(e) => e.into(),
        CategoryServiceError::MappingError(e) => {
            ApiError::internal_server_error(INTERNAL_SERVER_ERROR, Box::new(e))
        }
    }
}

#[tracing::instrument(
    skip(event, category_service),
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
    category_service: &impl CategoryService,
) -> Result<ApiGatewayV2httpResponse, lambda_runtime::Error> {
    match handle(event, category_service).await {
        Ok(response) => Ok(response),
        Err(err) => {
            log_api_error(&err);
            Ok(ApiGatewayV2httpResponse::from(err))
        }
    }
}

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    category_service: &impl CategoryService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    match event.payload.route_key.as_deref() {
        Some("GET /api/v1/categories/{categoryId}") => {
            category::get::handle(event, category_service).await
        }
        Some("GET /api/v1/categories") => category::get_all::handle(event, category_service).await,
        Some("POST /api/v1/categories/search") => {
            category::search::handle(event, category_service).await
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
