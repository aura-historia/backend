use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::api::error_code::{INTERNAL_SERVER_ERROR, NOT_FOUND};
use common::product_id::ProductKey;
use common::shop_id::api::extract_shop_id_path;
use common::shops_product_id::api::extract_shops_product_id_path;
use lambda_runtime::LambdaEvent;
use product::service::command_service::{CommandProductService, DeleteProductCommandError};
use shop::service::get_service::GetShopService;
use user::service::{authenticator_service::AuthenticatorService, user_service::UserService};

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    get_shop_service: &(impl GetShopService + Sync),
    user_service: &(impl UserService + Sync),
    authenticator_service: &(impl AuthenticatorService + Sync),
    command_product_service: &(impl CommandProductService + Sync),
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let shop_id = extract_shop_id_path(&event.payload.path_parameters)?;
    let shops_product_id = extract_shops_product_id_path(&event.payload.path_parameters)?;
    crate::authorize_partner_or_admin_product_request(
        &event.payload.headers,
        &shop_id,
        get_shop_service,
        user_service,
        authenticator_service,
    )
    .await?;

    command_product_service
        .delete(&ProductKey::new(shop_id, shops_product_id))
        .await
        .map_err(|err| match err {
            DeleteProductCommandError::NotFound => ApiError::not_found(NOT_FOUND, Box::new(err)),
            DeleteProductCommandError::Load(msg) | DeleteProductCommandError::Persist(msg) => {
                ApiError::internal_server_error(INTERNAL_SERVER_ERROR, msg.into())
            }
        })?;

    Ok(ApiGatewayV2HttpResponseBuilder::json(204).build())
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::shop_id::ShopId;
    use common::shops_product_id::ShopsProductId;
    use common::user_id::UserId;
    use fake::{Fake, Faker};
    use product::service::command_service::MockCommandProductService;
    use shop::core::partner_shop::PartnerShop;
    use shop::service::get_service::MockGetShopService;
    use user::core::role::UserRole;
    use user::service::authenticator_service::{AuthenticatedPrincipal, MockAuthenticatorService};
    use user::service::user_service::MockUserService;

    fn make_event(
        shop_id: &ShopId,
        shops_product_id: &ShopsProductId,
    ) -> LambdaEvent<ApiGatewayV2httpRequest> {
        let mut request = ApiGatewayV2httpRequest::default();
        request.route_key =
            Some("DELETE /api/v1/shops/{shopId}/products/{shopsProductId}".to_string());
        request
            .path_parameters
            .insert("shopId".to_string(), shop_id.to_string());
        request
            .path_parameters
            .insert("shopsProductId".to_string(), shops_product_id.to_string());
        LambdaEvent::new(request, lambda_runtime::Context::default())
    }

    fn user_auth(user_id: UserId) -> MockAuthenticatorService {
        let mut authenticator_service = MockAuthenticatorService::default();
        authenticator_service
            .expect_authenticate()
            .return_once(move |_| {
                Box::pin(async move { Ok(Some(AuthenticatedPrincipal::UserId(user_id))) })
            });
        authenticator_service
    }

    fn user_service_with_user(
        user_id: UserId,
        shop_id: ShopId,
        role: UserRole,
        is_partner: bool,
    ) -> MockUserService {
        let mut user_service = MockUserService::default();
        user_service.expect_find_user().return_once(move |_| {
            let mut user: user::core::user::User = Faker.fake();
            user.user_id = user_id;
            user.role = role;
            user.partner_shops.clear();
            if is_partner {
                user.partner_shops.insert(shop_id);
            }
            Box::pin(async move { Ok(user) })
        });
        user_service
    }

    fn shop_service_with_partner_shop(shop_id: ShopId) -> MockGetShopService {
        let mut shop_service = MockGetShopService::default();
        shop_service
            .expect_find_partner_shop()
            .return_once(move |_| {
                let mut partner_shop: PartnerShop = Faker.fake();
                partner_shop.shop_id = shop_id;
                Box::pin(async move { Ok(partner_shop) })
            });
        shop_service
    }

    #[tokio::test]
    async fn should_delete_product_when_user_is_admin() {
        let shop_id = ShopId::new();
        let shops_product_id = ShopsProductId::from("delete-me".to_string());
        let user_id = UserId::new();
        let event = make_event(&shop_id, &shops_product_id);
        let shop_service = shop_service_with_partner_shop(shop_id);
        let user_service = user_service_with_user(user_id, shop_id, UserRole::Admin, false);
        let authenticator_service = user_auth(user_id);
        let mut command_product_service = MockCommandProductService::default();
        command_product_service
            .expect_delete()
            .return_once(move |key| {
                assert_eq!(*key, ProductKey::new(shop_id, shops_product_id));
                Box::pin(async { Ok(()) })
            });

        let result = handle(
            event,
            &shop_service,
            &user_service,
            &authenticator_service,
            &command_product_service,
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap().status_code, 204);
    }

    #[tokio::test]
    async fn should_delete_product_when_user_is_partner_for_shop() {
        let shop_id = ShopId::new();
        let shops_product_id = ShopsProductId::from("delete-me".to_string());
        let user_id = UserId::new();
        let event = make_event(&shop_id, &shops_product_id);
        let shop_service = shop_service_with_partner_shop(shop_id);
        let user_service = user_service_with_user(user_id, shop_id, UserRole::User, true);
        let authenticator_service = user_auth(user_id);
        let mut command_product_service = MockCommandProductService::default();
        command_product_service
            .expect_delete()
            .return_once(move |key| {
                assert_eq!(*key, ProductKey::new(shop_id, shops_product_id));
                Box::pin(async { Ok(()) })
            });

        let result = handle(
            event,
            &shop_service,
            &user_service,
            &authenticator_service,
            &command_product_service,
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap().status_code, 204);
    }

    #[tokio::test]
    async fn should_forbid_delete_when_user_is_not_admin_or_partner_for_shop() {
        let shop_id = ShopId::new();
        let shops_product_id = ShopsProductId::from("delete-me".to_string());
        let user_id = UserId::new();
        let event = make_event(&shop_id, &shops_product_id);
        let shop_service = MockGetShopService::default();
        let user_service = user_service_with_user(user_id, shop_id, UserRole::User, false);
        let authenticator_service = user_auth(user_id);
        let command_product_service = MockCommandProductService::default();

        let result = handle(
            event,
            &shop_service,
            &user_service,
            &authenticator_service,
            &command_product_service,
        )
        .await;

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().status, 403);
    }

    #[tokio::test]
    async fn should_return_404_when_product_command_service_returns_not_found() {
        let shop_id = ShopId::new();
        let shops_product_id = ShopsProductId::from("missing".to_string());
        let user_id = UserId::new();
        let event = make_event(&shop_id, &shops_product_id);
        let shop_service = shop_service_with_partner_shop(shop_id);
        let user_service = user_service_with_user(user_id, shop_id, UserRole::Admin, false);
        let authenticator_service = user_auth(user_id);
        let mut command_product_service = MockCommandProductService::default();
        command_product_service
            .expect_delete()
            .return_once(|_| Box::pin(async { Err(DeleteProductCommandError::NotFound) }));

        let result = handle(
            event,
            &shop_service,
            &user_service,
            &authenticator_service,
            &command_product_service,
        )
        .await;

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().status, 404);
    }
}
