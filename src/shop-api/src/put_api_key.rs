use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::shop_id::api::extract_shop_id_path;
use common::user_id::api::extract_user_id_request_context;
use lambda_runtime::LambdaEvent;
use serde::Serialize;
use shop::core::partner_shop_api_key::PartnerShopApiKey;
use shop::service::command_service::CommandShopService;
use shop::service::get_service::GetShopService;
use user::service::user_service::UserService;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ApiKeyResponse {
    api_key: PartnerShopApiKey,
}

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    command_shop_service: &(impl CommandShopService + Sync),
    get_shop_service: &(impl GetShopService + Sync),
    user_service: &(impl UserService + Sync),
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id = extract_user_id_request_context(&event.payload.request_context)?;
    tracing::Span::current().record("userId", user_id.to_string());

    let shop_id = extract_shop_id_path(&event.payload.path_parameters)?;
    let is_admin = user_service.check_admin(&user_id).await.is_ok();

    let effective_user_id = if is_admin {
        let partner_shop = get_shop_service.find_partner_shop(&shop_id).await?;
        partner_shop.partner_user_id
    } else {
        user_id
    };

    let api_key = command_shop_service
        .create_api_key(&effective_user_id, &shop_id)
        .await?;

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .body_serde(ApiKeyResponse { api_key })?
        .build())
}

#[cfg(test)]
mod tests {
    use crate::handle;
    use common::shop_id::ShopId;
    use common::user_id::UserId;
    use fake::{Fake, Faker};
    use lambda_runtime::LambdaEvent;
    use shop::core::partner_shop::PartnerShop;
    use shop::core::partner_shop_api_key::PartnerShopApiKey;
    use shop::service::command_service::{CommandShopError, MockCommandShopService};
    use shop::service::get_service::{MockGetShopService, VerifyPartnerShopError};
    use shop::service::query_service::MockQueryShopService;
    use test_api::ApiGatewayV2httpRequestProxy;
    use user::core::user::User;
    use user::service::user_service::{MockUserService, UserServiceError};

    #[tokio::test]
    async fn should_return_200_when_partner_user_creates_api_key() {
        let user_id = UserId::new();
        let shop_id = ShopId::new();
        let api_key = PartnerShopApiKey::new();
        let expected_key = api_key.clone();

        let mut command_service = MockCommandShopService::default();
        command_service
            .expect_create_api_key()
            .return_once(move |_, _| Box::pin(async move { Ok(api_key) }));

        let get_shop_service = MockGetShopService::default();

        let mut user_service = MockUserService::default();
        user_service
            .expect_check_admin()
            .return_once(move |_| Box::pin(async { Err(UserServiceError::AdminRoleRequired) }));

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PUT)
                .route_key("PUT /api/v1/shops/{shopId}/api-key")
                .path_parameter("shopId", shop_id.to_string())
                .jwt_claim("sub", user_id)
                .build(),
            context: Default::default(),
        };

        let response = handle(
            lambda_event,
            &command_service,
            &get_shop_service,
            &MockQueryShopService::default(),
            &user_service,
        )
        .await
        .unwrap();
        assert_eq!(200, response.status_code);

        let body: serde_json::Value = match response.body {
            Some(aws_lambda_events::encodings::Body::Text(body_str)) => {
                serde_json::from_str(&body_str).unwrap()
            }
            _ => panic!("Expected response body to be Text"),
        };
        let key_str: String = expected_key.into();
        assert_eq!(body["apiKey"], key_str);
    }

    #[tokio::test]
    async fn should_return_200_when_admin_creates_api_key() {
        let admin_user_id = UserId::new();
        let partner_user_id = UserId::new();
        let shop_id = ShopId::new();

        let mut partner_shop: PartnerShop = Faker.fake();
        partner_shop.shop_id = shop_id;
        partner_shop.partner_user_id = partner_user_id;

        let api_key = PartnerShopApiKey::new();

        let mut get_shop_service = MockGetShopService::default();
        get_shop_service
            .expect_find_partner_shop()
            .return_once(move |_| Box::pin(async move { Ok(partner_shop) }));

        let mut command_service = MockCommandShopService::default();
        command_service
            .expect_create_api_key()
            .return_once(move |_, _| Box::pin(async move { Ok(api_key) }));

        let mut user_service = MockUserService::default();
        user_service
            .expect_check_admin()
            .return_once(move |_| Box::pin(async { Ok(()) }));

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PUT)
                .route_key("PUT /api/v1/shops/{shopId}/api-key")
                .path_parameter("shopId", shop_id.to_string())
                .jwt_claim("sub", admin_user_id)
                .build(),
            context: Default::default(),
        };

        let response = handle(
            lambda_event,
            &command_service,
            &get_shop_service,
            &MockQueryShopService::default(),
            &user_service,
        )
        .await
        .unwrap();
        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_return_401_when_jwt_claim_missing() {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PUT)
                .route_key("PUT /api/v1/shops/{shopId}/api-key")
                .path_parameter("shopId", ShopId::new().to_string())
                .build(),
            context: Default::default(),
        };

        let response = handle(
            lambda_event,
            &MockCommandShopService::default(),
            &MockGetShopService::default(),
            &MockQueryShopService::default(),
            &MockUserService::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(401, response.status);
    }

    #[tokio::test]
    async fn should_return_403_when_user_is_not_partner_of_shop() {
        let user_id = UserId::new();
        let shop_id = ShopId::new();

        let mut command_service = MockCommandShopService::default();
        command_service
            .expect_create_api_key()
            .return_once(move |_, _| {
                Box::pin(async move {
                    Err(CommandShopError::NotThePartnerUser(user_id, shop_id))
                })
            });

        let mut user_service = MockUserService::default();
        user_service
            .expect_check_admin()
            .return_once(move |_| Box::pin(async { Err(UserServiceError::AdminRoleRequired) }));

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PUT)
                .route_key("PUT /api/v1/shops/{shopId}/api-key")
                .path_parameter("shopId", shop_id.to_string())
                .jwt_claim("sub", user_id)
                .build(),
            context: Default::default(),
        };

        let response = handle(
            lambda_event,
            &command_service,
            &MockGetShopService::default(),
            &MockQueryShopService::default(),
            &user_service,
        )
        .await
        .unwrap_err();
        assert_eq!(403, response.status);
    }

    #[tokio::test]
    async fn should_return_404_when_shop_not_found() {
        let user_id = UserId::new();
        let shop_id = ShopId::new();

        let mut command_service = MockCommandShopService::default();
        command_service
            .expect_create_api_key()
            .return_once(move |_, _| {
                Box::pin(async move { Err(CommandShopError::ShopNotFound(shop_id)) })
            });

        let mut user_service = MockUserService::default();
        user_service
            .expect_check_admin()
            .return_once(move |_| Box::pin(async { Err(UserServiceError::AdminRoleRequired) }));

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PUT)
                .route_key("PUT /api/v1/shops/{shopId}/api-key")
                .path_parameter("shopId", shop_id.to_string())
                .jwt_claim("sub", user_id)
                .build(),
            context: Default::default(),
        };

        let response = handle(
            lambda_event,
            &command_service,
            &MockGetShopService::default(),
            &MockQueryShopService::default(),
            &user_service,
        )
        .await
        .unwrap_err();
        assert_eq!(404, response.status);
    }
}
