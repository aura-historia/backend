pub mod post;

use crate::post::PostUserSearchFilterData;
use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::{
    api::{
        api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder,
        error::{ApiError, log_api_error},
        error_code::BAD_BODY_VALUE,
    },
    user_id::api::extract_user_id_request_context,
};
use lambda_runtime::LambdaEvent;
use search_filter::data::user_search_filter_data::UserSearchFilterData;
use search_filter::service::user_search_filter_service::UserSearchFilterService;

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

// POST /api/v1/me/search-filters
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
        .save_user_search_filter(
            &user_id,
            user_search_filter_data.name,
            user_search_filter_data.search.into(),
        )
        .await?
        .into();

    let location = match event.payload.request_context.domain_name {
        None => None,
        Some(domain_name) => match event.payload.request_context.stage {
            Some(stage_name) => Some(format!(
                "https://{domain_name}/{stage_name}/api/v1/me/search-filters/{}",
                user_search_filter_data.user_search_filter_id
            )),
            None => None,
        },
    };
    let content_language = user_search_filter_data.search.language;

    Ok(ApiGatewayV2HttpResponseBuilder::json(201)
        .try_location(location.as_deref())
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
    use http::header::LOCATION;
    use lambda_runtime::LambdaEvent;
    use search_filter::core::user_search_filter::UserSearchFilter;
    use search_filter::service::user_search_filter_service::MockUserSearchFilterService;
    use test_api::{ApiGatewayV2httpRequestProxy, extract_apigw_response_json_body};

    #[tokio::test]
    async fn should_201_when_success() {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .body_serde(&Faker.fake::<PostUserSearchFilterData>())
                .jwt_claim("sub", UserId::new())
                .domain_name("my.domain.com")
                .stage("prod")
                .build(),
            context: Default::default(),
        };

        let expected = Faker.fake::<UserSearchFilter>();
        let expected_search_filter_id = expected.user_search_filter_id;
        let mut service = MockUserSearchFilterService::default();
        service
            .expect_save_user_search_filter()
            .return_once(move |_, _, _| Box::pin(async move { Ok(expected) }));

        let response = handler(lambda_event, &service).await.unwrap();

        assert_eq!(201, response.status_code);
        assert_eq!(
            format!(
                "https://my.domain.com/prod/api/v1/me/search-filters/{expected_search_filter_id}"
            ),
            response.headers.get(LOCATION).unwrap().to_str().unwrap()
        )
    }

    #[tokio::test]
    async fn should_400_when_body_search_filter_missing() {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let mut service = MockUserSearchFilterService::default();
        service.expect_save_user_search_filter().never();

        let response = handler(lambda_event, &service).await.unwrap();
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
                .jwt_claim("sub", UserId::new())
                .body_serde(&"invalid-search-filter-json")
                .build(),
            context: Default::default(),
        };

        let mut service = MockUserSearchFilterService::default();
        service.expect_save_user_search_filter().never();

        let response = handler(lambda_event, &service).await.unwrap();
        let json = extract_apigw_response_json_body!(response);

        assert_eq!(400, response.status_code);
        assert_eq!(400, json["status"]);
        assert_eq!("BAD_BODY_VALUE", json["error"]);
    }
}
