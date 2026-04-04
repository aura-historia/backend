use crate::patch_types::PatchUserSearchFilterData;
use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::{
    api::{
        api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder, error::ApiError,
        error_code::BAD_BODY_VALUE,
    },
    user_id::api::extract_user_id_request_context,
};
use lambda_runtime::LambdaEvent;
use search_filter::core::{
    user_search_filter_id::api::extract_user_search_filter_id_path,
    user_search_filter_update::UserSearchFilterUpdate,
};
use search_filter::data::user_search_filter_data::UserSearchFilterData;
use search_filter::service::user_search_filter_service::UserSearchFilterService;

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl UserSearchFilterService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id = extract_user_id_request_context(&event.payload.request_context)?;
    tracing::Span::current().record("userId", user_id.to_string());
    let search_filter_id = extract_user_search_filter_id_path(&event.payload.path_parameters)?;
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
    use product::service::get_service::MockGetProductService;
    use product_personalization::service::MockProductPersonalizationService;
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
                .route_key("PATCH /api/v1/me/search-filters/{userSearchFilterId}")
                .path_parameter("userSearchFilterId", UserSearchFilterId::new())
                .body_serde(&PatchUserSearchFilterData {
                    name: Some("foo".into()),
                    notifications: None,
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

        let get_product_service = MockGetProductService::default();
        let personalization_service = MockProductPersonalizationService::default();
        let response = handler(
            lambda_event,
            &service,
            &get_product_service,
            &personalization_service,
        )
        .await
        .unwrap();

        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_200_no_op_when_body_missing() {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .route_key("PATCH /api/v1/me/search-filters/{userSearchFilterId}")
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

        let get_product_service = MockGetProductService::default();
        let personalization_service = MockProductPersonalizationService::default();
        let response = handler(
            lambda_event,
            &service,
            &get_product_service,
            &personalization_service,
        )
        .await
        .unwrap();

        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_200_no_op_when_body_empty_object() {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .route_key("PATCH /api/v1/me/search-filters/{userSearchFilterId}")
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

        let get_product_service = MockGetProductService::default();
        let personalization_service = MockProductPersonalizationService::default();
        let response = handler(
            lambda_event,
            &service,
            &get_product_service,
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
                .http_method(http::Method::PATCH)
                .route_key("PATCH /api/v1/me/search-filters/{userSearchFilterId}")
                .body_serde(&Faker.fake::<PatchUserSearchFilterData>())
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let mut service = MockUserSearchFilterService::default();
        service.expect_update_user_search_filter().never();

        let get_product_service = MockGetProductService::default();
        let personalization_service = MockProductPersonalizationService::default();
        let response = handler(
            lambda_event,
            &service,
            &get_product_service,
            &personalization_service,
        )
        .await
        .unwrap();
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
                .route_key("PATCH /api/v1/me/search-filters/{userSearchFilterId}")
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

        let get_product_service = MockGetProductService::default();
        let personalization_service = MockProductPersonalizationService::default();
        let response = handler(
            lambda_event,
            &service,
            &get_product_service,
            &personalization_service,
        )
        .await
        .unwrap();
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
                .route_key("PATCH /api/v1/me/search-filters/{userSearchFilterId}")
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

        let get_product_service = MockGetProductService::default();
        let personalization_service = MockProductPersonalizationService::default();
        let response = handler(
            lambda_event,
            &service,
            &get_product_service,
            &personalization_service,
        )
        .await
        .unwrap();
        let json = extract_apigw_response_json_body!(response);

        assert_eq!(404, response.status_code);
        assert_eq!(404, json["status"]);
        assert_eq!("SEARCH_FILTER_NOT_FOUND", json["error"]);
    }
}
