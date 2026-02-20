use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::error::{ApiError, log_api_error};
use common::api::error_code::INTERNAL_SERVER_ERROR;
use lambda_runtime::LambdaEvent;
use product_classification::category::service::CategoryService;

pub mod category;

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
        Some("GET /api/v1/categories") => {
            let is_simple_search = event
                .payload
                .query_string_parameters
                .iter()
                .next()
                .is_some();
            if is_simple_search {
                category::search::handle(event, category_service).await
            } else {
                category::get_all::handle(event, category_service).await
            }
        }
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

#[cfg(test)]
mod tests {
    use super::handle;
    use lambda_runtime::LambdaEvent;
    use product_classification::category::service::MockCategoryService;
    use test_api::ApiGatewayV2httpRequestProxy;

    #[tokio::test]
    async fn should_route_to_simple_search_when_any_query_params_for_categories_get() {
        let mut service = MockCategoryService::default();
        service.expect_view_categories().never();
        service
            .expect_search_categories()
            .once()
            .return_once(|_, _| Box::pin(async { Ok(vec![]) }));

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/categories")
                .query_string_parameter("language", "de")
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &service).await.unwrap();

        assert_eq!(200, response.status_code);
    }
}
