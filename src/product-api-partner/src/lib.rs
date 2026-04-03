use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::{
    error::{ApiError, log_api_error},
    error_code::INTERNAL_SERVER_ERROR,
};
use lambda_runtime::LambdaEvent;
use product::service::command_service::CommandProductService;
use shop::service::get_service::GetShopService;
use shop::service::seller_service::SellerService;

pub mod patch_products;
pub mod post_products;
pub mod put_products;

#[tracing::instrument(
    skip(event, get_shop_service, command_product_service, seller_service),
    fields(
        requestId = %event.context.request_id,
        method = event.payload.request_context.http.method.as_str(),
        path = &event.payload.raw_path.as_deref().unwrap_or("NULL"),
        body = &event.payload.body.as_deref().unwrap_or("NULL"),
        ip = &event.payload.request_context.http.source_ip.as_deref().unwrap_or("NULL"),
        userAgent = &event.payload.request_context.http.user_agent.as_deref().unwrap_or("NULL"),
    )
)]
pub async fn handler(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    get_shop_service: &(impl GetShopService + Sync),
    command_product_service: &(impl CommandProductService + Sync),
    seller_service: &(impl SellerService + Sync),
) -> Result<ApiGatewayV2httpResponse, lambda_runtime::Error> {
    match handle(
        event,
        get_shop_service,
        command_product_service,
        seller_service,
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
    command_product_service: &(impl CommandProductService + Sync),
    seller_service: &(impl SellerService + Sync),
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    match event.payload.route_key.as_deref() {
        Some("POST /api/v1/shops/{shopId}/products") => {
            post_products::handle(
                event,
                get_shop_service,
                command_product_service,
                seller_service,
            )
            .await
        }
        Some("PATCH /api/v1/shops/{shopId}/products") => {
            patch_products::handle(event, get_shop_service, command_product_service).await
        }
        Some("PUT /api/v1/shops/{shopId}/products") => {
            put_products::handle(
                event,
                get_shop_service,
                command_product_service,
                seller_service,
            )
            .await
        }
        Some(unknown) => Err(ApiError::internal_server_error(
            INTERNAL_SERVER_ERROR,
            format!("Unknown route-key '{}' in AWS-Payload", unknown).into(),
        )),
        None => Err(ApiError::internal_server_error(
            INTERNAL_SERVER_ERROR,
            "Missing route-key in AWS-Payload".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_lambda_events::apigw::ApiGatewayV2httpRequest;
    use lambda_runtime::LambdaEvent;
    use product::service::command_service::MockCommandProductService;
    use shop::service::get_service::MockGetShopService;
    use shop::service::seller_service::MockSellerService;

    fn make_event(route_key: Option<&str>) -> LambdaEvent<ApiGatewayV2httpRequest> {
        let mut request = ApiGatewayV2httpRequest::default();
        request.route_key = route_key.map(String::from);
        LambdaEvent::new(request, lambda_runtime::Context::default())
    }

    #[tokio::test]
    async fn should_return_500_when_unknown_route_key() {
        let event = make_event(Some("GET /unknown"));
        let shop_service = MockGetShopService::default();
        let command_service = MockCommandProductService::default();
        let seller_service = MockSellerService::default();

        let result = handle(event, &shop_service, &command_service, &seller_service).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.status, 500);
    }

    #[tokio::test]
    async fn should_return_500_when_missing_route_key() {
        let event = make_event(None);
        let shop_service = MockGetShopService::default();
        let command_service = MockCommandProductService::default();
        let seller_service = MockSellerService::default();

        let result = handle(event, &shop_service, &command_service, &seller_service).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.status, 500);
    }
}
