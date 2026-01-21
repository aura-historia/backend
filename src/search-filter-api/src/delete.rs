use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::{
    api::{api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder, error::ApiError},
    user_id::api::extract_user_id_request_context,
};
use lambda_runtime::LambdaEvent;
use search_filter::core::user_search_filter_id::api::extract_user_search_filter_id_path;
use search_filter::service::user_search_filter_service::UserSearchFilterService;

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl UserSearchFilterService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id = extract_user_id_request_context(&event.payload.request_context)?;
    tracing::Span::current().record("userId", user_id.to_string());
    let search_filter_id = extract_user_search_filter_id_path(&event.payload.path_parameters)?;

    service
        .delete_user_search_filter(&user_id, &search_filter_id)
        .await?;

    Ok(ApiGatewayV2HttpResponseBuilder::new(204).build())
}

#[cfg(test)]
mod tests {
    use crate::handle;
    use common::user_id::UserId;
    use fake::{Fake, Faker};
    use lambda_runtime::LambdaEvent;
    use search_filter::core::user_search_filter_id::UserSearchFilterId;
    use search_filter::service::user_search_filter_service::{
        MockUserSearchFilterService, UserSearchFilterError,
    };
    use test_api::ApiGatewayV2httpRequestProxy;

    #[tokio::test]
    async fn should_204_when_success() {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::DELETE)
                .route_key("DELETE /api/v1/me/search-filters/{userSearchFilterId}")
                .path_parameter("userSearchFilterId", UserSearchFilterId::new())
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let mut service = MockUserSearchFilterService::default();
        service
            .expect_delete_user_search_filter()
            .return_once(|_, _| Box::pin(async { Ok(()) }));

        let response = handle(lambda_event, &service).await.unwrap();

        assert_eq!(204, response.status_code);
    }

    #[tokio::test]
    async fn should_400_when_path_param_search_filter_id_missing() {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::DELETE)
                .route_key("DELETE /api/v1/me/search-filters/{userSearchFilterId}")
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let mut service = MockUserSearchFilterService::default();
        service.expect_delete_user_search_filter().never();

        let expected = handle(lambda_event, &service).await.unwrap_err();
        assert_eq!(400, expected.status);
    }

    #[tokio::test]
    async fn should_400_when_search_filter_id_invalid() {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::DELETE)
                .route_key("DELETE /api/v1/me/search-filters/{userSearchFilterId}")
                .path_parameter("userSearchFilterId", "not-a-valid-uuid")
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let mut service = MockUserSearchFilterService::default();
        service
            .expect_delete_user_search_filter()
            .return_once(|_, _| {
                Box::pin(async {
                    Err(UserSearchFilterError::UserSearchFilterNotFound(
                        Faker.fake(),
                        Faker.fake(),
                    ))
                })
            });

        let expected = handle(lambda_event, &service).await.unwrap_err();
        assert_eq!(400, expected.status);
    }

    #[tokio::test]
    async fn should_404_when_search_filter_not_exists() {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::DELETE)
                .route_key("DELETE /api/v1/me/search-filters/{userSearchFilterId}")
                .path_parameter("userSearchFilterId", UserSearchFilterId::new())
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let mut service = MockUserSearchFilterService::default();
        service
            .expect_delete_user_search_filter()
            .return_once(|_, _| {
                Box::pin(async {
                    Err(UserSearchFilterError::UserSearchFilterNotFound(
                        Faker.fake(),
                        Faker.fake(),
                    ))
                })
            });

        let expected = handle(lambda_event, &service).await.unwrap_err();
        assert_eq!(404, expected.status);
    }
}
