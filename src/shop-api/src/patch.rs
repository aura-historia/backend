use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::api::error_code::BAD_BODY_VALUE;
use common::shop_id::api::extract_shop_id_path;
use common::user_id::api::extract_user_id_request_context;
use lambda_runtime::LambdaEvent;
use shop::data::get_shop_data::GetShopData;
use shop::data::patch_shop_data::PatchShopData;
use shop::service::command::UpdateShopCommand;
use shop::service::command_service::CommandShopService;
use shop::service::get_service::GetShopService;
use user::service::user_service::UserService;

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
    if !is_admin {
        let partner_shop = get_shop_service.find_partner_shop(&shop_id).await?;
        if partner_shop.partner_user_id != user_id {
            return Err(
                shop::service::command_service::CommandShopError::NotThePartnerUser(
                    user_id, shop_id,
                )
                .into(),
            );
        }
    }

    let body = event
        .payload
        .body
        .filter(|str| !str.is_empty())
        .ok_or_else(|| {
            let err_msg = "Body cannot be empty";
            ApiError::bad_request(BAD_BODY_VALUE, err_msg.into()).with_detail(err_msg)
        })?;

    let patch_data: PatchShopData = serde_json::from_str(&body).map_err(|err| {
        let err_msg = err.to_string();
        ApiError::bad_request(BAD_BODY_VALUE, Box::new(err)).with_detail(err_msg)
    })?;

    let update_command = UpdateShopCommand {
        shop_type: patch_data.shop_type.map(Into::into),
        domains: patch_data.domains,
        url: patch_data.url,
        image: patch_data.image,
        structured_address: patch_data.structured_address.map(Into::into),
        phone: patch_data.phone,
        email: patch_data.email,
    };

    let updated_shop = command_shop_service
        .update(&shop_id, update_command)
        .await?;

    let shop_data: GetShopData = GetShopData::from(updated_shop);

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .last_modified(shop_data.updated)
        .body_serde(shop_data)?
        .build())
}

#[cfg(test)]
mod tests {
    use super::handle;
    use common::shop_id::ShopId;
    use common::user_id::UserId;
    use fake::{Fake, Faker};
    use lambda_runtime::LambdaEvent;
    use shop::core::partner_shop::PartnerShop;
    use shop::core::shop::Shop;
    use shop::data::patch_shop_data::PatchShopData;
    use shop::service::command_service::MockCommandShopService;
    use shop::service::get_service::MockGetShopService;
    use test_api::ApiGatewayV2httpRequestProxy;
    use user::service::user_service::{MockUserService, UserServiceError};

    #[tokio::test]
    async fn should_return_200_when_partner_user_updates_shop() {
        let user_id = UserId::new();
        let shop_id = ShopId::new();

        let mut partner_shop: PartnerShop = Faker.fake();
        partner_shop.shop_id = shop_id;
        partner_shop.partner_user_id = user_id;

        let mut get_shop_service = MockGetShopService::default();
        get_shop_service
            .expect_find_partner_shop()
            .return_once(move |_| Box::pin(async move { Ok(partner_shop) }));

        let mut command_service = MockCommandShopService::default();
        command_service.expect_update().return_once(move |_, _| {
            let shop: Shop = Faker.fake();
            Box::pin(async move { Ok(shop) })
        });

        let mut user_service = MockUserService::default();
        user_service
            .expect_check_admin()
            .return_once(move |_| Box::pin(async { Err(UserServiceError::AdminRoleRequired) }));

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .route_key("PATCH /api/v1/shops/{shopId}")
                .path_parameter("shopId", shop_id.to_string())
                .jwt_claim("sub", user_id)
                .body_serde(&Faker.fake::<PatchShopData>())
                .build(),
            context: Default::default(),
        };

        let response = handle(
            lambda_event,
            &command_service,
            &get_shop_service,
            &user_service,
        )
        .await
        .unwrap();
        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_return_200_when_admin_updates_shop() {
        let admin_user_id = UserId::new();
        let shop_id = ShopId::new();

        let mut command_service = MockCommandShopService::default();
        command_service.expect_update().return_once(move |_, _| {
            let shop: Shop = Faker.fake();
            Box::pin(async move { Ok(shop) })
        });

        let get_shop_service = MockGetShopService::default();

        let mut user_service = MockUserService::default();
        user_service
            .expect_check_admin()
            .return_once(move |_| Box::pin(async { Ok(()) }));

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .route_key("PATCH /api/v1/shops/{shopId}")
                .path_parameter("shopId", shop_id.to_string())
                .jwt_claim("sub", admin_user_id)
                .body_serde(&Faker.fake::<PatchShopData>())
                .build(),
            context: Default::default(),
        };

        let response = handle(
            lambda_event,
            &command_service,
            &get_shop_service,
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
                .http_method(http::Method::PATCH)
                .route_key("PATCH /api/v1/shops/{shopId}")
                .path_parameter("shopId", ShopId::new().to_string())
                .body_serde(&Faker.fake::<PatchShopData>())
                .build(),
            context: Default::default(),
        };

        let response = handle(
            lambda_event,
            &MockCommandShopService::default(),
            &MockGetShopService::default(),
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

        let mut partner_shop: PartnerShop = Faker.fake();
        partner_shop.shop_id = shop_id;
        partner_shop.partner_user_id = UserId::new(); // different user

        let mut get_shop_service = MockGetShopService::default();
        get_shop_service
            .expect_find_partner_shop()
            .return_once(move |_| Box::pin(async move { Ok(partner_shop) }));

        let mut user_service = MockUserService::default();
        user_service
            .expect_check_admin()
            .return_once(move |_| Box::pin(async { Err(UserServiceError::AdminRoleRequired) }));

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .route_key("PATCH /api/v1/shops/{shopId}")
                .path_parameter("shopId", shop_id.to_string())
                .jwt_claim("sub", user_id)
                .body_serde(&Faker.fake::<PatchShopData>())
                .build(),
            context: Default::default(),
        };

        let response = handle(
            lambda_event,
            &MockCommandShopService::default(),
            &get_shop_service,
            &user_service,
        )
        .await
        .unwrap_err();
        assert_eq!(403, response.status);
    }

    #[tokio::test]
    async fn should_return_400_when_body_is_empty() {
        let user_id = UserId::new();
        let shop_id = ShopId::new();

        let mut user_service = MockUserService::default();
        user_service
            .expect_check_admin()
            .return_once(move |_| Box::pin(async { Ok(()) }));

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .route_key("PATCH /api/v1/shops/{shopId}")
                .path_parameter("shopId", shop_id.to_string())
                .jwt_claim("sub", user_id)
                .build(),
            context: Default::default(),
        };

        let response = handle(
            lambda_event,
            &MockCommandShopService::default(),
            &MockGetShopService::default(),
            &user_service,
        )
        .await
        .unwrap_err();
        assert_eq!(400, response.status);
    }
}
