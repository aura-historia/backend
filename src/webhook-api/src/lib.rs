use crate::woocommerce::handler::handle_woocommerce;
use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::{
    error::{ApiError, log_api_error},
    error_code::INTERNAL_SERVER_ERROR,
};
use lambda_runtime::LambdaEvent;
use product_lambda_ingest_partner_products::AsyncProductCommandService;
use shop::service::get_service::GetShopService;
pub mod woocommerce;

#[tracing::instrument(
    skip(event, get_shop_service, async_product_command_service),
    fields(
        requestId = %event.context.request_id,
        method = event.payload.request_context.http.method.as_str(),
        path = &event.payload.raw_path.as_deref().unwrap_or("NULL"),
        ip = &event.payload.request_context.http.source_ip.as_deref().unwrap_or("NULL"),
        userAgent = &event.payload.request_context.http.user_agent.as_deref().unwrap_or("NULL"),
    )
)]
pub async fn handler(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    get_shop_service: &(impl GetShopService + Sync),
    async_product_command_service: &(impl AsyncProductCommandService + Sync),
) -> Result<ApiGatewayV2httpResponse, lambda_runtime::Error> {
    match handle(event, get_shop_service, async_product_command_service).await {
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
    async_product_command_service: &(impl AsyncProductCommandService + Sync),
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    match event.payload.route_key.as_deref() {
        Some("POST /api/v1/webhooks/woocommerce/{shopId}") => {
            handle_woocommerce(event, get_shop_service, async_product_command_service).await
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
