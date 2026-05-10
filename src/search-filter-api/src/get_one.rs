use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::{
    api::{api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder, error::ApiError},
    user_id::api::extract_user_id_request_context,
    user_search_filter_id::api::extract_user_search_filter_id_path,
};
use lambda_runtime::LambdaEvent;
use search_filter::data::user_search_filter_data::UserSearchFilterData;
use search_filter::service::user_search_filter_service::UserSearchFilterService;

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl UserSearchFilterService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id = extract_user_id_request_context(&event.payload.request_context)?;
    tracing::Span::current().record("userId", user_id.to_string());
    let search_filter_id = extract_user_search_filter_id_path(&event.payload.path_parameters)?;

    let user_search_filter_data: UserSearchFilterData = service
        .find_user_search_filter(&user_id, &search_filter_id)
        .await?
        .into();
    let content_language = user_search_filter_data.search.language;

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .content_language(content_language)
        .last_modified(user_search_filter_data.updated)
        .cache_control("no-store", None, None)
        .body_serde(user_search_filter_data)?
        .build())
}

#[cfg(test)]
mod tests {
    use crate::handle;
    use common::user_id::UserId;
    use common::user_search_filter_id::UserSearchFilterId;
    use fake::{Fake, Faker};
    use http::header::CACHE_CONTROL;
    use lambda_runtime::LambdaEvent;
    use product::service::get_service::MockGetProductService;
    use product::service::query_service::MockQueryProductService;
    use product_personalization::service::MockProductPersonalizationService;
    use search_filter::service::user_search_filter_service::{
        MockUserSearchFilterService, UserSearchFilterError,
    };
    use test_api::ApiGatewayV2httpRequestProxy;

    #[tokio::test]
    async fn should_200_when_success() {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/me/search-filters/{userSearchFilterId}")
                .path_parameter("userSearchFilterId", UserSearchFilterId::new())
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let mut service = MockUserSearchFilterService::default();
        service
            .expect_find_user_search_filter()
            .return_once(|_, _| Box::pin(async { Ok(Faker.fake()) }));

        let get_product_service = MockGetProductService::default();
        let query_product_service = MockQueryProductService::default();
        let personalization_service = MockProductPersonalizationService::default();
        let response = handle(
            lambda_event,
            &service,
            &get_product_service,
            &query_product_service,
            None,
            None,
            &personalization_service,
        )
        .await
        .unwrap();

        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_400_when_path_param_search_filter_id_missing() {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/me/search-filters/{userSearchFilterId}")
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let mut service = MockUserSearchFilterService::default();
        service.expect_find_user_search_filter().never();

        let get_product_service = MockGetProductService::default();
        let query_product_service = MockQueryProductService::default();
        let personalization_service = MockProductPersonalizationService::default();
        let expected = handle(
            lambda_event,
            &service,
            &get_product_service,
            &query_product_service,
            None,
            None,
            &personalization_service,
        )
        .await
        .unwrap_err();
        assert_eq!(400, expected.status);
    }

    #[tokio::test]
    async fn should_400_when_search_filter_id_invalid() {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/me/search-filters/{userSearchFilterId}")
                .path_parameter("userSearchFilterId", "not-a-valid-uuid")
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let mut service = MockUserSearchFilterService::default();
        service
            .expect_find_user_search_filter()
            .return_once(|_, _| {
                Box::pin(async {
                    Err(UserSearchFilterError::UserSearchFilterNotFound(
                        Faker.fake(),
                        Faker.fake(),
                    ))
                })
            });

        let get_product_service = MockGetProductService::default();
        let query_product_service = MockQueryProductService::default();
        let personalization_service = MockProductPersonalizationService::default();
        let expected = handle(
            lambda_event,
            &service,
            &get_product_service,
            &query_product_service,
            None,
            None,
            &personalization_service,
        )
        .await
        .unwrap_err();
        assert_eq!(400, expected.status);
    }

    #[tokio::test]
    async fn should_404_when_search_filter_not_exists() {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/me/search-filters/{userSearchFilterId}")
                .path_parameter("userSearchFilterId", UserSearchFilterId::new())
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let mut service = MockUserSearchFilterService::default();
        service
            .expect_find_user_search_filter()
            .return_once(|_, _| {
                Box::pin(async {
                    Err(UserSearchFilterError::UserSearchFilterNotFound(
                        Faker.fake(),
                        Faker.fake(),
                    ))
                })
            });

        let get_product_service = MockGetProductService::default();
        let query_product_service = MockQueryProductService::default();
        let personalization_service = MockProductPersonalizationService::default();
        let expected = handle(
            lambda_event,
            &service,
            &get_product_service,
            &query_product_service,
            None,
            None,
            &personalization_service,
        )
        .await
        .unwrap_err();
        assert_eq!(404, expected.status);
    }

    #[tokio::test]
    async fn should_set_cache_control_to_no_store_for_get_one_search_filter() {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/me/search-filters/{userSearchFilterId}")
                .path_parameter("userSearchFilterId", UserSearchFilterId::new())
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let mut service = MockUserSearchFilterService::default();
        service
            .expect_find_user_search_filter()
            .return_once(|_, _| Box::pin(async { Ok(Faker.fake()) }));

        let get_product_service = MockGetProductService::default();
        let query_product_service = MockQueryProductService::default();
        let personalization_service = MockProductPersonalizationService::default();
        let response = handle(
            lambda_event,
            &service,
            &get_product_service,
            &query_product_service,
            None,
            None,
            &personalization_service,
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
