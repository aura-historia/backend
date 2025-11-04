use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::{
    api::{
        api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder,
        error::ApiError,
        error_code::{BAD_PATH_PARAMETER_VALUE, INVALID_UUID},
    },
    user_id::api::extract_user_id_cognito_jwt,
};
use lambda_runtime::LambdaEvent;
use search_filter::core::user_search_filter_id::UserSearchFilterId;
use search_filter::service::user_search_filter_service::UserSearchFilterService;

#[tracing::instrument(
    skip(event, service),
    fields(
        requestId = %event.context.request_id,
        path = &event.payload.raw_path,
        query = &event.payload.raw_query_string,
    )
)]
pub async fn handler(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl UserSearchFilterService,
) -> Result<ApiGatewayV2httpResponse, lambda_runtime::Error> {
    match handle(event, service).await {
        Ok(response) => Ok(response),
        Err(err) => Ok(ApiGatewayV2httpResponse::from(err)),
    }
}

// DELETE /api/v1/me/search-filters/{searchFilterId}
pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl UserSearchFilterService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id = extract_user_id_cognito_jwt(&event.payload.request_context)?;
    let search_filter_id = event
        .payload
        .path_parameters
        .get("userSearchFilterId")
        .filter(|str| !str.is_empty())
        .map(String::as_str)
        .map(UserSearchFilterId::try_from)
        .ok_or_else(|| {
            ApiError::bad_request(BAD_PATH_PARAMETER_VALUE).with_path_field("userSearchFilterId")
        })?
        .map_err(|err| {
            ApiError::bad_request(INVALID_UUID)
                .with_path_field("userSearchFilterId")
                .with_message(err.to_string())
        })?;

    service
        .delete_user_search_filter(&user_id, &search_filter_id)
        .await?;

    Ok(ApiGatewayV2HttpResponseBuilder::new(204).build())
}

#[cfg(test)]
mod tests {
    use crate::handler;
    use common::user_id::UserId;
    use fake::{Fake, Faker};
    use lambda_runtime::LambdaEvent;
    use search_filter::core::user_search_filter_id::UserSearchFilterId;
    use search_filter::service::user_search_filter_service::{
        MockUserSearchFilterService, UserSearchFilterError,
    };
    use test_api::{ApiGatewayV2httpRequestProxy, extract_apigw_response_json_body};

    #[tokio::test]
    async fn should_204_when_success() {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::DELETE)
                .path_parameter("userSearchFilterId", UserSearchFilterId::new())
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let mut service = MockUserSearchFilterService::default();
        service
            .expect_delete_user_search_filter()
            .return_once(|_, _| Box::pin(async { Ok(()) }));

        let response = handler(lambda_event, &service).await.unwrap();

        assert_eq!(204, response.status_code);
    }

    #[tokio::test]
    async fn should_400_when_path_param_search_filter_id_missing() {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::DELETE)
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let mut service = MockUserSearchFilterService::default();
        service.expect_delete_user_search_filter().never();

        let response = handler(lambda_event, &service).await.unwrap();
        let json = extract_apigw_response_json_body!(response);

        assert_eq!(400, response.status_code);
        assert_eq!(400, json["status"]);
        assert_eq!("BAD_PATH_PARAMETER_VALUE", json["error"]);
    }

    #[tokio::test]
    async fn should_400_when_search_filter_id_invalid() {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::DELETE)
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

        let response = handler(lambda_event, &service).await.unwrap();
        let json = extract_apigw_response_json_body!(response);

        assert_eq!(400, response.status_code);
        assert_eq!(400, json["status"]);
        assert_eq!("INVALID_UUID", json["error"]);
    }

    #[tokio::test]
    async fn should_404_when_search_filter_not_exists() {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::DELETE)
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

        let response = handler(lambda_event, &service).await.unwrap();
        let json = extract_apigw_response_json_body!(response);

        assert_eq!(404, response.status_code);
        assert_eq!(404, json["status"]);
        assert_eq!("SEARCH_FILTER_NOT_FOUND", json["error"]);
    }
}
