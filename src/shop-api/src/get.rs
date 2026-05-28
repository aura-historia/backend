use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::api::error_code::INTERNAL_SERVER_ERROR;
use common::shop_id::api::{extract_shop_id_path, extract_shop_slug_id_path};
use lambda_runtime::LambdaEvent;
use shop::data::get_shop_data::GetShopData;
use shop::service::get_service::GetShopService;
use user::service::authenticator_service::AuthenticatorService;

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl GetShopService,
    authenticator_service: &(impl AuthenticatorService + Sync),
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let authenticated = authenticator_service
        .authenticate(&event.payload.headers)
        .await?;
    if let Some(principal) = authenticated.as_ref() {
        tracing::Span::current().record("userId", principal.user_id().to_string());
    }

    let shop = match event.payload.route_key.as_deref() {
        Some("GET /api/v1/shops/{shopId}") => {
            let shop_id = extract_shop_id_path(&event.payload.path_parameters)?;
            service.find_shop(&shop_id).await?
        }
        Some("GET /api/v1/by-slug/shops/{shopSlugId}") => {
            let shop_slug_id = extract_shop_slug_id_path(&event.payload.path_parameters)?;
            service.find_shop_by_slug(&shop_slug_id).await?
        }
        Some(unknown) => {
            return Err(ApiError::internal_server_error(
                INTERNAL_SERVER_ERROR,
                format!("Unknown route-key '{unknown}' in AWS-Payload").into(),
            ));
        }
        None => {
            return Err(ApiError::internal_server_error(
                INTERNAL_SERVER_ERROR,
                "Missing route-key in AWS-Payload".into(),
            ));
        }
    };

    let shop_data: GetShopData = GetShopData::from(shop);

    let (cache_directive, max_age, s_max_age) = if authenticated.is_some() {
        ("no-store", None, None)
    } else {
        ("public", Some(600), Some(3600))
    };

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .last_modified(shop_data.updated)
        .cache_control(cache_directive, max_age, s_max_age)
        .body_serde(shop_data)?
        .build())
}

#[cfg(test)]
mod tests {
    use crate::handle;
    use common::shop_id::ShopId;
    use fake::{Fake, Faker};
    use http::header::{CACHE_CONTROL, LAST_MODIFIED};
    use lambda_runtime::LambdaEvent;
    use shop::core::shop::Shop;
    use shop::service::command_service::MockCommandShopService;
    use shop::service::get_service::{GetShopError, MockGetShopService};
    use shop::service::query_service::MockQueryShopService;
    use test_api::ApiGatewayV2httpRequestProxy;
    use time::macros::datetime;
    use user::service::{
        authenticator_service::MockAuthenticatorService, user_service::MockUserService,
    };

    #[tokio::test]
    async fn should_include_updated_timestamp_as_header_last_modified() {
        let timestamp = datetime!(2020-01-01 0:00 UTC);
        let mut service = MockGetShopService::default();
        service.expect_find_shop().return_once(move |_| {
            let mut shop: Shop = Faker.fake();
            shop.updated = timestamp;
            Box::pin(async move { Ok(shop) })
        });
        let shop_id = ShopId::new();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/shops/{shopId}")
                .path_parameter("shopId", shop_id)
                .build(),
            context: Default::default(),
        };
        let mut authenticator_service = MockAuthenticatorService::default();
        authenticator_service
            .expect_authenticate()
            .return_once(|_| Box::pin(async { Ok(None) }));
        let response = handle(
            lambda_event,
            &service,
            &MockQueryShopService::default(),
            &MockCommandShopService::default(),
            &MockUserService::default(),
            &authenticator_service,
        )
        .await
        .unwrap();
        assert_eq!(200, response.status_code);
        assert_eq!(
            "Wed, 01 Jan 2020 00:00:00 GMT",
            response.headers.get(LAST_MODIFIED).unwrap()
        );
    }

    #[tokio::test]
    async fn should_400_when_path_param_shop_id_is_missing() {
        let mut service = MockGetShopService::default();
        service.expect_find_shop().never();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/shops/{shopId}")
                .build(),
            context: Default::default(),
        };
        let mut authenticator_service = MockAuthenticatorService::default();
        authenticator_service
            .expect_authenticate()
            .return_once(|_| Box::pin(async { Ok(None) }));

        let response = handle(
            lambda_event,
            &service,
            &MockQueryShopService::default(),
            &MockCommandShopService::default(),
            &MockUserService::default(),
            &authenticator_service,
        )
        .await
        .unwrap_err();
        assert_eq!(400, response.status);
    }

    #[tokio::test]
    async fn should_404_when_shop_does_not_exist_for_id() {
        let shop_id = ShopId::new();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/shops/{shopId}")
                .path_parameter("shopId", shop_id)
                .build(),
            context: Default::default(),
        };

        let mut service = MockGetShopService::default();
        service.expect_find_shop().return_once(move |shop_id| {
            let shop_id = *shop_id;
            Box::pin(async move { Err(GetShopError::ShopNotFound(shop_id)) })
        });
        let mut authenticator_service = MockAuthenticatorService::default();
        authenticator_service
            .expect_authenticate()
            .return_once(|_| Box::pin(async { Ok(None) }));

        let response = handle(
            lambda_event,
            &service,
            &MockQueryShopService::default(),
            &MockCommandShopService::default(),
            &MockUserService::default(),
            &authenticator_service,
        )
        .await
        .unwrap_err();
        assert_eq!(404, response.status);
    }

    #[tokio::test]
    async fn should_set_cache_control_to_public_with_long_max_ages_when_unauthenticated() {
        let mut service = MockGetShopService::default();
        service.expect_find_shop().return_once(move |_| {
            let shop: Shop = Faker.fake();
            Box::pin(async move { Ok(shop) })
        });
        let shop_id = ShopId::new();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/shops/{shopId}")
                .path_parameter("shopId", shop_id)
                .build(),
            context: Default::default(),
        };
        let mut authenticator_service = MockAuthenticatorService::default();
        authenticator_service
            .expect_authenticate()
            .return_once(|_| Box::pin(async { Ok(None) }));

        let response = handle(
            lambda_event,
            &service,
            &MockQueryShopService::default(),
            &MockCommandShopService::default(),
            &MockUserService::default(),
            &authenticator_service,
        )
        .await
        .unwrap();

        assert_eq!(200, response.status_code);
        assert_eq!(
            "public, max-age=600, s-maxage=3600",
            response
                .headers
                .get(CACHE_CONTROL)
                .unwrap()
                .to_str()
                .unwrap()
        );
    }

    #[tokio::test]
    async fn should_set_cache_control_to_no_store_when_authenticated() {
        use common::user_id::UserId;
        use user::service::authenticator_service::AuthenticatedPrincipal;

        let mut service = MockGetShopService::default();
        service.expect_find_shop().return_once(move |_| {
            let shop: Shop = Faker.fake();
            Box::pin(async move { Ok(shop) })
        });
        let shop_id = ShopId::new();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/shops/{shopId}")
                .path_parameter("shopId", shop_id)
                .build(),
            context: Default::default(),
        };
        let user_id = UserId::new();
        let mut authenticator_service = MockAuthenticatorService::default();
        authenticator_service
            .expect_authenticate()
            .return_once(move |_| {
                Box::pin(async move { Ok(Some(AuthenticatedPrincipal::UserId(user_id))) })
            });

        let response = handle(
            lambda_event,
            &service,
            &MockQueryShopService::default(),
            &MockCommandShopService::default(),
            &MockUserService::default(),
            &authenticator_service,
        )
        .await
        .unwrap();

        assert_eq!(200, response.status_code);
        assert_eq!(
            "no-store",
            response
                .headers
                .get(CACHE_CONTROL)
                .unwrap()
                .to_str()
                .unwrap()
        );
    }
}
