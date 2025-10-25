pub mod patch;

use crate::patch::PatchUserSearchFilterData;
use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::{
    api::{
        api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder,
        error::ApiError,
        error_code::{BAD_BODY_VALUE, BAD_PATH_PARAMETER_VALUE, INVALID_UUID},
    },
    user_id::api::extract_user_id_cognito_jwt,
};
use lambda_runtime::LambdaEvent;
use search_filter_core::search_filter_id::SearchFilterId;
use search_filter_data::user_search_filter_data::UserSearchFilterData;
use search_filter_service::{
    search_filter_update::SearchFilterUpdate, service::SearchFilterService,
};

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
    service: &impl SearchFilterService,
) -> Result<ApiGatewayV2httpResponse, lambda_runtime::Error> {
    match handle(event, service).await {
        Ok(response) => Ok(response),
        Err(err) => Ok(ApiGatewayV2httpResponse::from(err)),
    }
}

// PATCH /api/v1/search-filters/{searchFilterId}
pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl SearchFilterService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id = extract_user_id_cognito_jwt(&event.payload.request_context)?;
    let search_filter_id = event
        .payload
        .path_parameters
        .get("searchFilterId")
        .filter(|str| !str.is_empty())
        .map(String::as_str)
        .map(SearchFilterId::try_from)
        .ok_or_else(|| {
            ApiError::bad_request(BAD_PATH_PARAMETER_VALUE).with_path_field("searchFilterId")
        })?
        .map_err(|err| {
            ApiError::bad_request(INVALID_UUID)
                .with_path_field("searchFilterId")
                .with_message(err.to_string())
        })?;
    let body = event.payload.body;

    let patched: UserSearchFilterData = match body {
        Some(body) if !body.is_empty() => {
            let patch: PatchUserSearchFilterData = serde_json::from_str(&body).map_err(|err| {
                ApiError::bad_request(BAD_BODY_VALUE).with_message(err.to_string())
            })?;
            let update: SearchFilterUpdate = patch.into();
            if update.is_empty() {
                service
                    .find_search_filter(&user_id, &search_filter_id)
                    .await?
                    .into()
            } else {
                service
                    .update_search_filter(&user_id, &search_filter_id, update)
                    .await?
                    .into()
            }
        }
        _ => service
            .find_search_filter(&user_id, &search_filter_id)
            .await?
            .into(),
    };

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .last_modified(patched.updated)
        .body_serde(patched)?
        .build())
}

#[cfg(test)]
mod tests {
    use crate::{handler, patch::PatchUserSearchFilterData};
    use common::user_id::UserId;
    use fake::{Fake, Faker};
    use lambda_runtime::LambdaEvent;
    use search_filter_core::search_filter_id::SearchFilterId;
    use search_filter_service::service::{MockSearchFilterService, SearchFilterError};
    use test_api::{ApiGatewayV2httpRequestProxy, extract_apigw_response_json_body};

    #[tokio::test]
    async fn should_200_when_success() {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .path_parameter("searchFilterId", SearchFilterId::new())
                .body_serde(&Faker.fake::<PatchUserSearchFilterData>())
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let mut service = MockSearchFilterService::default();
        service
            .expect_update_search_filter()
            .return_once(|_, _, _| Box::pin(async { Ok(Faker.fake()) }));

        let response = handler(lambda_event, &service).await.unwrap();

        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_200_no_op_when_body_missing() {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .path_parameter("searchFilterId", SearchFilterId::new())
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let mut service = MockSearchFilterService::default();
        service.expect_update_search_filter().never();
        service
            .expect_find_search_filter()
            .return_once(|_, _| Box::pin(async { Ok(Faker.fake()) }));

        let response = handler(lambda_event, &service).await.unwrap();

        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_200_no_op_when_body_empty_object() {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .path_parameter("searchFilterId", SearchFilterId::new())
                .body_serde(&PatchUserSearchFilterData::default())
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let mut service = MockSearchFilterService::default();
        service.expect_update_search_filter().never();
        service
            .expect_find_search_filter()
            .return_once(|_, _| Box::pin(async { Ok(Faker.fake()) }));

        let response = handler(lambda_event, &service).await.unwrap();

        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_400_when_path_param_search_filter_id_missing() {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .body_serde(&Faker.fake::<PatchUserSearchFilterData>())
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let mut service = MockSearchFilterService::default();
        service.expect_update_search_filter().never();

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
                .http_method(http::Method::PATCH)
                .path_parameter("searchFilterId", "not-a-valid-uuid")
                .body_serde(&Faker.fake::<PatchUserSearchFilterData>())
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let mut service = MockSearchFilterService::default();
        service
            .expect_update_search_filter()
            .return_once(|_, _, _| {
                Box::pin(async {
                    Err(SearchFilterError::SearchFilterNotFound(
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
                .http_method(http::Method::PATCH)
                .path_parameter("searchFilterId", SearchFilterId::new())
                .body_serde(&Faker.fake::<PatchUserSearchFilterData>())
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let mut service = MockSearchFilterService::default();
        service
            .expect_update_search_filter()
            .return_once(|_, _, _| {
                Box::pin(async {
                    Err(SearchFilterError::SearchFilterNotFound(
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
