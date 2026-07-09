use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::{
    error::{ApiError, log_api_error},
    error_code::INTERNAL_SERVER_ERROR,
};
use common::shop_id::ShopId;
use lambda_runtime::LambdaEvent;
use product::service::command_service::CommandProductService;
use product_lambda_ingest_partner_products::AsyncProductCommandService;
use shop::core::partner_shop::PartnerShop;
use shop::service::get_service::GetShopService;
use user::{
    core::{access_token::Scope, role::UserRole},
    service::{
        authenticator_service::{AuthenticatedPrincipal, AuthenticatorService},
        user_service::UserService,
    },
};

pub mod delete_product;
pub mod patch_products;
pub mod post_products;
pub mod put_products;

pub async fn authorize_partner_or_admin_product_request(
    headers: &http::HeaderMap,
    shop_id: &ShopId,
    get_shop_service: &(impl GetShopService + Sync),
    user_service: &(impl UserService + Sync),
    authenticator_service: &(impl AuthenticatorService + Sync),
) -> Result<PartnerShop, ApiError> {
    let authenticated = authenticator_service
        .authenticate(headers)
        .await?
        .ok_or_else(|| {
            ApiError::unauthorized(common::api::error_code::UNAUTHORIZED)
                .with_header_field("Authorization")
        })?;

    let user_id = match authenticated {
        AuthenticatedPrincipal::UserId(user_id) => user_id,
        AuthenticatedPrincipal::AccessToken(access_token) => {
            if !access_token.has_scope(Scope::ProductsWrite) {
                return Err(ApiError::forbidden(common::api::error_code::FORBIDDEN));
            }
            access_token.user_id
        }
    };

    let user = user_service.find_user(&user_id).await?;
    if user.role != UserRole::Admin && !user.partner_shops.contains(shop_id) {
        return Err(ApiError::forbidden(common::api::error_code::FORBIDDEN));
    }
    Ok(get_shop_service.find_partner_shop(shop_id).await?)
}

#[tracing::instrument(
    skip(event, get_shop_service, user_service, authenticator_service, async_product_command_service, command_product_service),
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
    user_service: &(impl UserService + Sync),
    authenticator_service: &(impl AuthenticatorService + Sync),
    async_product_command_service: &(impl AsyncProductCommandService + Sync),
    command_product_service: &(impl CommandProductService + Sync),
) -> Result<ApiGatewayV2httpResponse, lambda_runtime::Error> {
    match handle(
        event,
        get_shop_service,
        user_service,
        authenticator_service,
        async_product_command_service,
        command_product_service,
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
    user_service: &(impl UserService + Sync),
    authenticator_service: &(impl AuthenticatorService + Sync),
    async_product_command_service: &(impl AsyncProductCommandService + Sync),
    command_product_service: &(impl CommandProductService + Sync),
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    match event.payload.route_key.as_deref() {
        Some("POST /api/v1/shops/{shopId}/products") => {
            post_products::handle(
                event,
                get_shop_service,
                user_service,
                authenticator_service,
                async_product_command_service,
            )
            .await
        }
        Some("PATCH /api/v1/shops/{shopId}/products") => {
            patch_products::handle(
                event,
                get_shop_service,
                user_service,
                authenticator_service,
                async_product_command_service,
            )
            .await
        }
        Some("DELETE /api/v1/shops/{shopId}/products/{shopsProductId}") => {
            delete_product::handle(
                event,
                get_shop_service,
                user_service,
                authenticator_service,
                command_product_service,
            )
            .await
        }
        Some("PUT /api/v1/shops/{shopId}/products") => {
            put_products::handle(
                event,
                get_shop_service,
                user_service,
                authenticator_service,
                async_product_command_service,
            )
            .await
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
    use super::*;
    use aws_lambda_events::apigw::ApiGatewayV2httpRequest;
    use lambda_runtime::LambdaEvent;
    use product::service::command_service::MockCommandProductService;
    use product_lambda_ingest_partner_products::service::MockAsyncProductCommandService;
    use shop::service::get_service::MockGetShopService;
    use user::service::{
        authenticator_service::MockAuthenticatorService, user_service::MockUserService,
    };

    fn make_event(route_key: Option<&str>) -> LambdaEvent<ApiGatewayV2httpRequest> {
        let mut request = ApiGatewayV2httpRequest::default();
        request.route_key = route_key.map(String::from);
        LambdaEvent::new(request, lambda_runtime::Context::default())
    }

    #[tokio::test]
    async fn should_return_500_when_unknown_route_key() {
        let event = make_event(Some("GET /unknown"));
        let shop_service = MockGetShopService::default();
        let command_service = MockAsyncProductCommandService::default();
        let command_product_service = MockCommandProductService::default();
        let user_service = MockUserService::default();
        let authenticator_service = MockAuthenticatorService::default();

        let result = handle(
            event,
            &shop_service,
            &user_service,
            &authenticator_service,
            &command_service,
            &command_product_service,
        )
        .await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.status, 500);
    }

    #[tokio::test]
    async fn should_return_500_when_missing_route_key() {
        let event = make_event(None);
        let shop_service = MockGetShopService::default();
        let command_service = MockAsyncProductCommandService::default();
        let command_product_service = MockCommandProductService::default();
        let user_service = MockUserService::default();
        let authenticator_service = MockAuthenticatorService::default();

        let result = handle(
            event,
            &shop_service,
            &user_service,
            &authenticator_service,
            &command_service,
            &command_product_service,
        )
        .await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.status, 500);
    }
}
