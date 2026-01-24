use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::{
    api::{api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder, error::ApiError},
    pagination::page::api::PaginatedData,
    sort::api::extract_sort_query,
    user_id::api::extract_user_id_request_context,
};
use lambda_runtime::LambdaEvent;
use search_filter::data::{
    sort_user_search_filter_data::SortUserSearchFilterFieldData,
    user_search_filter_data::UserSearchFilterData,
};
use search_filter::service::user_search_filter_service::UserSearchFilterService;

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl UserSearchFilterService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id = extract_user_id_request_context(&event.payload.request_context)?;
    tracing::Span::current().record("userId", user_id.to_string());
    let order = extract_sort_query::<SortUserSearchFilterFieldData>(
        &event.payload.query_string_parameters,
    )?
    .map(|sort| sort.order);

    let user_search_filters_data: Vec<UserSearchFilterData> = service
        .find_user_search_filters(&user_id, &order)
        .await?
        .into_iter()
        .map(UserSearchFilterData::from)
        .collect();
    let count = user_search_filters_data.len();
    let collection = PaginatedData {
        items: user_search_filters_data,
        from: 0,
        size: count as u64,
        total: Some(count as u64),
    };

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .body_serde(collection)?
        .build())
}

#[cfg(test)]
mod tests {
    use crate::handle;
    use common::user_id::UserId;
    use lambda_runtime::LambdaEvent;
    use search_filter::core::user_search_filter::UserSearchFilter;
    use search_filter::service::user_search_filter_service::MockUserSearchFilterService;
    use test_api::ApiGatewayV2httpRequestProxy;

    #[tokio::test]
    async fn should_200_when_success() {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/me/search-filters")
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let mut service = MockUserSearchFilterService::default();
        service
            .expect_find_user_search_filters()
            .return_once(|_, _| Box::pin(async { Ok(fake::vec![UserSearchFilter; 42]) }));

        let response = handle(lambda_event, &service).await.unwrap();

        assert_eq!(200, response.status_code);
    }
}
