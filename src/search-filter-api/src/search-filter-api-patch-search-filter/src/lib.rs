pub mod patch;

use crate::patch::PatchUserSearchFilterData;
use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::{
    api::{
        api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder,
        error::{ApiError, log_api_error},
        error_code::{BAD_BODY_VALUE, BAD_PATH_PARAMETER_VALUE, INVALID_UUID},
    },
    user_id::api::extract_user_id_request_context,
};
use lambda_runtime::LambdaEvent;
use search_filter::core::user_search_filter_id::UserSearchFilterId;
use search_filter::data::user_search_filter_data::UserSearchFilterData;
use search_filter::service::{
    user_search_filter_service::UserSearchFilterService,
    user_search_filter_update::UserSearchFilterUpdate,
};

#[tracing::instrument(
    skip(event, service),
    fields(
        requestId = %event.context.request_id,
        method = event.payload.request_context.http.method.as_str(),
        path = &event.payload.raw_path.as_deref().unwrap_or("NULL"),
        query = &event.payload.raw_query_string.as_deref().unwrap_or("NULL"),
        body = &event.payload.body.as_deref().unwrap_or("NULL"),
        ip = &event.payload.request_context.http.source_ip.as_deref().unwrap_or("NULL"),
        userAgent = &event.payload.request_context.http.user_agent.as_deref().unwrap_or("NULL"),
        userId = tracing::field::Empty,
    )
)]
pub async fn handler(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl UserSearchFilterService,
) -> Result<ApiGatewayV2httpResponse, lambda_runtime::Error> {
    match handle(event, service).await {
        Ok(response) => Ok(response),
        Err(err) => {
            log_api_error(&err);
            Ok(ApiGatewayV2httpResponse::from(err))
        }
    }
}

// PATCH /api/v1/me/search-filters/{searchFilterId}
pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl UserSearchFilterService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id = extract_user_id_request_context(&event.payload.request_context)?;
    tracing::Span::current().record("userId", user_id.to_string());
    let search_filter_id = event
        .payload
        .path_parameters
        .get("userSearchFilterId")
        .filter(|str| !str.is_empty())
        .map(String::as_str)
        .map(UserSearchFilterId::try_from)
        .ok_or_else(|| {
            let err_msg = "Parameter 'userSearchFilterId' cannot be empty.";
            ApiError::bad_request(BAD_PATH_PARAMETER_VALUE, err_msg.into())
                .with_path_field("userSearchFilterId")
                .with_detail(err_msg)
        })?
        .map_err(|err| {
            let err_msg = err.to_string();
            ApiError::bad_request(INVALID_UUID, Box::new(err))
                .with_path_field("userSearchFilterId")
                .with_detail(err_msg)
        })?;
    let body = event.payload.body;

    let patched: UserSearchFilterData = match body {
        Some(body) if !body.is_empty() => {
            let patch: PatchUserSearchFilterData = serde_json::from_str(&body).map_err(|err| {
                let err_msg = err.to_string();
                ApiError::bad_request(BAD_BODY_VALUE, Box::new(err)).with_detail(err_msg)
            })?;
            let update: UserSearchFilterUpdate = patch.into();
            if update.is_empty() {
                service
                    .find_user_search_filter(&user_id, &search_filter_id)
                    .await?
                    .into()
            } else {
                service
                    .update_user_search_filter(&user_id, &search_filter_id, update)
                    .await?
                    .into()
            }
        }
        _ => service
            .find_user_search_filter(&user_id, &search_filter_id)
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
    use search_filter::core::user_search_filter_id::UserSearchFilterId;
    use search_filter::service::user_search_filter_service::{
        MockUserSearchFilterService, UserSearchFilterError,
    };
    use test_api::{ApiGatewayV2httpRequestProxy, extract_apigw_response_json_body};

    #[tokio::test]
    async fn should_200_when_success() {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .path_parameter("userSearchFilterId", UserSearchFilterId::new())
                .body_serde(&PatchUserSearchFilterData {
                    name: Some("foo".into()),
                    search: None,
                })
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let mut service = MockUserSearchFilterService::default();
        service
            .expect_update_user_search_filter()
            .return_once(|_, _, _| Box::pin(async { Ok(Faker.fake()) }));

        let response = handler(lambda_event, &service).await.unwrap();

        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_200_no_op_when_body_missing() {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .path_parameter("userSearchFilterId", UserSearchFilterId::new())
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let mut service = MockUserSearchFilterService::default();
        service.expect_update_user_search_filter().never();
        service
            .expect_find_user_search_filter()
            .return_once(|_, _| Box::pin(async { Ok(Faker.fake()) }));

        let response = handler(lambda_event, &service).await.unwrap();

        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_200_no_op_when_body_empty_object() {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .path_parameter("userSearchFilterId", UserSearchFilterId::new())
                .body_serde(&PatchUserSearchFilterData::default())
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let mut service = MockUserSearchFilterService::default();
        service.expect_update_user_search_filter().never();
        service
            .expect_find_user_search_filter()
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

        let mut service = MockUserSearchFilterService::default();
        service.expect_update_user_search_filter().never();

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
                .path_parameter("userSearchFilterId", "not-a-valid-uuid")
                .body_serde(&Faker.fake::<PatchUserSearchFilterData>())
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let mut service = MockUserSearchFilterService::default();
        service
            .expect_update_user_search_filter()
            .return_once(|_, _, _| {
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
                .http_method(http::Method::PATCH)
                .path_parameter("userSearchFilterId", UserSearchFilterId::new())
                .body_serde(&Faker.fake::<PatchUserSearchFilterData>())
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let mut service = MockUserSearchFilterService::default();
        service.expect_find_user_search_filter().returning(|_, _| {
            Box::pin(async {
                Err(UserSearchFilterError::UserSearchFilterNotFound(
                    Faker.fake(),
                    Faker.fake(),
                ))
            })
        });
        service
            .expect_update_user_search_filter()
            .returning(|_, _, _| {
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
