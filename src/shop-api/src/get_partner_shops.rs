use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::api::error_code::FORBIDDEN;
use common::user_id::api::{extract_user_id_path, extract_user_id_request_context};
use lambda_runtime::LambdaEvent;
use shop::data::get_shop_data::GetShopData;
use shop::service::get_service::GetShopService;
use user::service::user_service::UserService;

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    get_shop_service: &(impl GetShopService + Sync),
    user_service: &(impl UserService + Sync),
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let path_user_id = extract_user_id_path(&event.payload.path_parameters)?;
    let caller_user_id = extract_user_id_request_context(&event.payload.request_context)?;

    tracing::Span::current().record("userId", caller_user_id.to_string());

    let is_admin = user_service.check_admin(&caller_user_id).await.is_ok();
    if !is_admin && caller_user_id != path_user_id {
        return Err(ApiError::forbidden(FORBIDDEN)
            .with_detail("You are not authorized to view shops for another user."));
    }

    let user = user_service.find_user(&path_user_id).await?;
    let shop_ids: Vec<_> = user.partner_shops.into_iter().collect();
    let shops = if shop_ids.is_empty() {
        vec![]
    } else {
        get_shop_service.find_shops(shop_ids).await?
    };
    let shop_data: Vec<GetShopData> = shops.into_iter().map(GetShopData::from).collect();

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .body_serde(shop_data)?
        .build())
}

#[cfg(test)]
mod tests {
    use super::handle;
    use common::user_id::UserId;
    use fake::{Fake, Faker};
    use lambda_runtime::LambdaEvent;
    use shop::core::shop::Shop;
    use shop::service::get_service::{GetShopError, MockGetShopService};
    use test_api::ApiGatewayV2httpRequestProxy;
    use user::core::user::User;
    use user::service::user_service::{MockUserService, UserServiceError};

    #[tokio::test]
    async fn should_200_with_partner_shops_for_authorized_user() {
        let user_id = UserId::new();
        let mut user: User = Faker.fake();
        let shop: Shop = Faker.fake();
        let shop_id = shop.shop_id;
        user.partner_shops = [shop_id].into();

        let mut user_service = MockUserService::default();
        user_service
            .expect_check_admin()
            .return_once(|_| Box::pin(async { Err(UserServiceError::AdminRoleRequired) }));
        let expected_user = user.clone();
        user_service
            .expect_find_user()
            .return_once(move |_| Box::pin(async move { Ok(expected_user) }));

        let expected_shop = shop.clone();
        let mut get_shop_service = MockGetShopService::default();
        get_shop_service
            .expect_find_shops()
            .return_once(move |_| Box::pin(async move { Ok(vec![expected_shop]) }));

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/me/partner-shops")
                .path_parameter("userId", user_id)
                .jwt_claim("sub", user_id)
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &get_shop_service, &user_service)
            .await
            .unwrap();

        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_403_when_caller_is_different_user_and_not_admin() {
        let path_user_id = UserId::new();
        let caller_user_id = UserId::new();

        let mut user_service = MockUserService::default();
        user_service
            .expect_check_admin()
            .return_once(|_| Box::pin(async { Err(UserServiceError::AdminRoleRequired) }));

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/me/partner-shops")
                .path_parameter("userId", path_user_id)
                .jwt_claim("sub", caller_user_id)
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &MockGetShopService::default(), &user_service)
            .await
            .unwrap_err();

        assert_eq!(403, response.status);
    }

    #[tokio::test]
    async fn should_200_with_empty_list_when_user_has_no_partner_shops() {
        let user_id = UserId::new();
        let mut user: User = Faker.fake();
        user.partner_shops = Default::default();

        let mut user_service = MockUserService::default();
        user_service
            .expect_check_admin()
            .return_once(|_| Box::pin(async { Err(UserServiceError::AdminRoleRequired) }));
        user_service
            .expect_find_user()
            .return_once(move |_| Box::pin(async move { Ok(user) }));

        let mut get_shop_service = MockGetShopService::default();
        get_shop_service.expect_find_shops().never();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/me/partner-shops")
                .path_parameter("userId", user_id)
                .jwt_claim("sub", user_id)
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &get_shop_service, &user_service)
            .await
            .unwrap();

        assert_eq!(200, response.status_code);
        let body: Vec<shop::data::get_shop_data::GetShopData> =
            serde_json::from_slice(response.body.as_deref().unwrap_or(b"[]")).unwrap();
        assert!(body.is_empty());
    }

    #[tokio::test]
    async fn should_400_when_user_id_path_param_is_missing() {
        let caller_user_id = UserId::new();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/me/partner-shops")
                .jwt_claim("sub", caller_user_id)
                .build(),
            context: Default::default(),
        };

        let response = handle(
            lambda_event,
            &MockGetShopService::default(),
            &MockUserService::default(),
        )
        .await
        .unwrap_err();

        assert_eq!(400, response.status);
    }

    #[tokio::test]
    async fn should_404_when_shop_not_found() {
        let user_id = UserId::new();
        let mut user: User = Faker.fake();
        let shop_id = common::shop_id::ShopId::new();
        user.partner_shops = [shop_id].into();

        let mut user_service = MockUserService::default();
        user_service
            .expect_check_admin()
            .return_once(|_| Box::pin(async { Err(UserServiceError::AdminRoleRequired) }));
        user_service
            .expect_find_user()
            .return_once(move |_| Box::pin(async move { Ok(user) }));

        let mut get_shop_service = MockGetShopService::default();
        get_shop_service.expect_find_shops().return_once(move |_| {
            Box::pin(async move { Err(GetShopError::ShopNotFound(shop_id)) })
        });

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/me/partner-shops")
                .path_parameter("userId", user_id)
                .jwt_claim("sub", user_id)
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &get_shop_service, &user_service)
            .await
            .unwrap_err();

        assert_eq!(404, response.status);
    }
}
