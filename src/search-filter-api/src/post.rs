use crate::post_types::PostUserSearchFilterData;
use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::{
    api::{
        api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder, error::ApiError,
        error_code::BAD_BODY_VALUE,
    },
    user_id::api::extract_user_id_request_context,
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
    let body = event
        .payload
        .body
        .filter(|str| !str.is_empty())
        .ok_or_else(|| {
            let err_msg = "Body cannot be empty";
            ApiError::bad_request(BAD_BODY_VALUE, err_msg.into()).with_detail(err_msg)
        })?;
    let user_search_filter_data: PostUserSearchFilterData =
        serde_json::from_str(&body).map_err(|err| {
            let err_msg = err.to_string();
            ApiError::bad_request(BAD_BODY_VALUE, Box::new(err)).with_detail(err_msg)
        })?;

    let user_search_filter_data: UserSearchFilterData = service
        .create_user_search_filter(
            &user_id,
            user_search_filter_data.name,
            user_search_filter_data.search.into(),
            user_search_filter_data
                .enhanced_search_description
                .map(Into::into),
        )
        .await?
        .into();

    let location = format!(
        "me/search-filters/{}",
        user_search_filter_data.user_search_filter_id
    );
    let content_language = user_search_filter_data.search.language;

    Ok(ApiGatewayV2HttpResponseBuilder::json(201)
        .location(&location, &event.payload.request_context)
        .content_language(content_language)
        .last_modified(user_search_filter_data.updated)
        .body_serde(user_search_filter_data)?
        .build())
}

#[cfg(test)]
mod tests {
    use crate::{handler, post::PostUserSearchFilterData};
    use common::user_id::UserId;
    use fake::{Fake, Faker};
    use lambda_runtime::LambdaEvent;
    use product::service::get_service::MockGetProductService;
    use product::service::query_service::MockQueryProductService;
    use product_personalization::service::MockProductPersonalizationService;
    use search_filter::core::user_search_filter::UserSearchFilter;
    use search_filter::service::user_search_filter_service::MockUserSearchFilterService;
    use test_api::{ApiGatewayV2httpRequestProxy, extract_apigw_response_json_body};

    #[tokio::test]
    async fn should_201_when_success() {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .route_key("POST /api/v1/me/search-filters")
                .body_serde(&Faker.fake::<PostUserSearchFilterData>())
                .jwt_claim("sub", UserId::new())
                .stage("prod")
                .build(),
            context: Default::default(),
        };

        let expected = Faker.fake::<UserSearchFilter>();
        let mut service = MockUserSearchFilterService::default();
        service
            .expect_create_user_search_filter()
            .return_once(move |_, _, _, _| Box::pin(async move { Ok(expected) }));

        let get_product_service = MockGetProductService::default();
        let query_product_service = MockQueryProductService::default();
        let personalization_service = MockProductPersonalizationService::default();
        let response = handler(
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

        assert_eq!(201, response.status_code);
    }

    #[tokio::test]
    async fn should_400_when_body_search_filter_missing() {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .route_key("POST /api/v1/me/search-filters")
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let mut service = MockUserSearchFilterService::default();
        service.expect_create_user_search_filter().never();

        let get_product_service = MockGetProductService::default();
        let query_product_service = MockQueryProductService::default();
        let personalization_service = MockProductPersonalizationService::default();
        let response = handler(
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
        let json = extract_apigw_response_json_body!(response);

        assert_eq!(400, response.status_code);
        assert_eq!(400, json["status"]);
        assert_eq!("BAD_BODY_VALUE", json["error"]);
    }

    #[tokio::test]
    async fn should_400_when_body_search_filter_invalid() {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .route_key("POST /api/v1/me/search-filters")
                .jwt_claim("sub", UserId::new())
                .body_serde(&"invalid-search-filter-json")
                .build(),
            context: Default::default(),
        };

        let mut service = MockUserSearchFilterService::default();
        service.expect_create_user_search_filter().never();

        let get_product_service = MockGetProductService::default();
        let query_product_service = MockQueryProductService::default();
        let personalization_service = MockProductPersonalizationService::default();
        let response = handler(
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
        let json = extract_apigw_response_json_body!(response);

        assert_eq!(400, response.status_code);
        assert_eq!(400, json["status"]);
        assert_eq!("BAD_BODY_VALUE", json["error"]);
    }
}
