use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::api::error_code::INTERNAL_SERVER_ERROR;
use common::shop_id::ShopIdentifier;
use common::shop_id::api::{extract_shop_id_path, extract_shop_slug_id_path};
use lambda_runtime::LambdaEvent;
use shop::data::get_shop_data::GetShopData;
use shop::data::shop_identifier_data::extract_shop_domain_path;
use shop::service::get_service::GetShopService;

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl GetShopService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let shop = match event.payload.route_key.as_deref() {
        Some("GET /api/v1/shops/{shopId}") => {
            let shop_id = extract_shop_id_path(&event.payload.path_parameters)?;
            service.find_shop(&ShopIdentifier::ShopId(shop_id)).await?
        }
        Some("GET /api/v1/by-domain/shops/{shopDomain}") => {
            let shop_domain = extract_shop_domain_path(&event.payload.path_parameters)?;
            service
                .find_shop(&ShopIdentifier::ShopDomain(shop_domain))
                .await?
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

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .last_modified(shop_data.updated)
        .cache_control("public", Some(3600), Some(86400))
        .body_serde(shop_data)?
        .build())
}

#[cfg(test)]
mod tests {
    use crate::handle;
    use common::domain::Domain;
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

    #[tokio::test]
    async fn should_include_updated_timestamp_as_header_last_modified_for_shop_id() {
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
        let response = handle(
            lambda_event,
            &service,
            &MockQueryShopService::default(),
            &MockCommandShopService::default(),
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
    async fn should_include_updated_timestamp_as_header_last_modified_for_shop_domain() {
        let timestamp = datetime!(2020-01-01 0:00 UTC);
        let mut service = MockGetShopService::default();
        service.expect_find_shop().return_once(move |_| {
            let mut shop: Shop = Faker.fake();
            shop.updated = timestamp;
            Box::pin(async move { Ok(shop) })
        });
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/by-domain/shops/{shopDomain}")
                .path_parameter("shopDomain", "foo.bar")
                .build(),
            context: Default::default(),
        };
        let response = handle(
            lambda_event,
            &service,
            &MockQueryShopService::default(),
            &MockCommandShopService::default(),
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
    async fn should_400_when_path_param_shop_id_is_missing_for_id() {
        let mut service = MockGetShopService::default();
        service.expect_find_shop().never();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/shops/{shopId}")
                .build(),
            context: Default::default(),
        };

        let response = handle(
            lambda_event,
            &service,
            &MockQueryShopService::default(),
            &MockCommandShopService::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(400, response.status);
    }

    #[tokio::test]
    async fn should_400_when_path_param_shop_id_is_missing_for_domain() {
        let mut service = MockGetShopService::default();
        service.expect_find_shop().never();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/shops/{shopId}")
                .build(),
            context: Default::default(),
        };

        let response = handle(
            lambda_event,
            &service,
            &MockQueryShopService::default(),
            &MockCommandShopService::default(),
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
        service
            .expect_find_shop()
            .return_once(move |shop_identifier| {
                let shop_identifier = shop_identifier.clone();
                Box::pin(async move { Err(GetShopError::ShopNotFound(shop_identifier)) })
            });

        let response = handle(
            lambda_event,
            &service,
            &MockQueryShopService::default(),
            &MockCommandShopService::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(404, response.status);
    }

    #[tokio::test]
    async fn should_404_when_shop_does_not_exist_for_domain() {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/by-domain/shops/{shopDomain}")
                .path_parameter("shopDomain", Faker.fake::<Domain>())
                .build(),
            context: Default::default(),
        };

        let mut service = MockGetShopService::default();
        service
            .expect_find_shop()
            .return_once(move |shop_identifier| {
                let shop_identifier = shop_identifier.clone();
                Box::pin(async move { Err(GetShopError::ShopNotFound(shop_identifier)) })
            });

        let response = handle(
            lambda_event,
            &service,
            &MockQueryShopService::default(),
            &MockCommandShopService::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(404, response.status);
    }

    #[tokio::test]
    async fn should_set_cache_control_to_public_with_long_max_ages_for_get_shop() {
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

        let response = handle(
            lambda_event,
            &service,
            &MockQueryShopService::default(),
            &MockCommandShopService::default(),
        )
        .await
        .unwrap();

        assert_eq!(200, response.status_code);
        assert_eq!(
            "public, max-age=3600, s-maxage=86400",
            response.headers.get(CACHE_CONTROL).unwrap().to_str().unwrap()
        );
    }
}
