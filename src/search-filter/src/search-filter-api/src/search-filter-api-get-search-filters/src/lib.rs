use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::{
    api::{
        api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder,
        collection::{OffsetLimitPaginatedData, PaginationData},
        error::ApiError,
    },
    sort::api::extract_sort_query,
    user_id::api::extract_user_id_cognito_jwt,
};
use lambda_runtime::LambdaEvent;
use search_filter_data::{
    sort_search_filter_data::SortSearchFilterFieldData,
    user_search_filter_data::UserSearchFilterData,
};
use search_filter_service::service::SearchFilterService;

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

// GET /api/v1/search-filters
pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl SearchFilterService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id = extract_user_id_cognito_jwt(&event.payload.request_context)?;
    let order =
        extract_sort_query::<SortSearchFilterFieldData>(&event.payload.query_string_parameters)?
            .map(|sort| sort.order);

    let user_search_filters_data: Vec<UserSearchFilterData> = service
        .find_search_filters(&user_id, &order)
        .await?
        .into_iter()
        .map(UserSearchFilterData::from)
        .collect();
    let count = user_search_filters_data.len();
    let collection = OffsetLimitPaginatedData {
        items: user_search_filters_data,
        pagination: PaginationData {
            from: 0,
            size: count as u64,
            total: Some(count as u64),
            next: None,
        },
    };

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .body_serde(collection)?
        .cors()
        .build())
}

#[cfg(test)]
mod tests {
    use crate::handler;
    use common::user_id::UserId;
    use lambda_runtime::LambdaEvent;
    use search_filter_core::user_search_filter::UserSearchFilter;
    use search_filter_service::service::MockSearchFilterService;
    use test_api::ApiGatewayV2httpRequestProxy;

    #[tokio::test]
    async fn should_200_when_success() {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let mut service = MockSearchFilterService::default();
        service
            .expect_find_search_filters()
            .return_once(|_, _| Box::pin(async { Ok(fake::vec![UserSearchFilter; 42]) }));

        let response = handler(lambda_event, &service).await.unwrap();

        assert_eq!(200, response.status_code);
    }
}
