use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::{
    api::{
        api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder,
        collection::OffsetLimitPaginatedData, error::ApiError, error_code::BAD_BODY_VALUE,
    },
    page::{Page, api::extract_page_query_u64},
    sort::api::extract_sort_query,
};
use item_core::sort_item_field::SortItemField;
use item_data::{get_data::GetItemData, sort_item_field_data::SortItemFieldData};
use item_service::query_service::QueryItemService;
use lambda_runtime::LambdaEvent;
use search_filter_data::search_filter_data::SearchFilterData;

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
    service: &impl QueryItemService,
) -> Result<ApiGatewayV2httpResponse, lambda_runtime::Error> {
    match handle(event, service).await {
        Ok(response) => Ok(response),
        Err(err) => Ok(ApiGatewayV2httpResponse::from(err)),
    }
}

// POST /api/v1/items/search
pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl QueryItemService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let sort = extract_sort_query::<SortItemFieldData>(&event.payload.query_string_parameters)?
        .map(|sort_data| sort_data.map(SortItemField::from));
    let page = extract_page_query_u64(&event.payload.query_string_parameters)?
        .unwrap_or(Page { from: 0, size: 21 });
    let body = event
        .payload
        .body
        .filter(|str| !str.is_empty())
        .ok_or_else(|| {
            ApiError::bad_request(BAD_BODY_VALUE).with_message("Body cannot be empty")
        })?;
    let search_filter_data: SearchFilterData = serde_json::from_str(&body)
        .map_err(|err| ApiError::bad_request(BAD_BODY_VALUE).with_message(err.to_string()))?;

    let search_filter = search_filter_data.into();
    let search_result = service
        .search_items(&search_filter, &sort, &Some(page))
        .await?
        .map_item(GetItemData::from);
    let collection = OffsetLimitPaginatedData::from(search_result);

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .body_serde(collection)?
        .cors()
        .build())
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
mod tests {
    use crate::handler;
    use common::page::Page;
    use common::paginated_result::PaginatedResult;
    use fake::Fake;
    use fake::Faker;
    use item_core::item::LocalizedItemView;
    use item_service::query_service::MockQueryItemService;
    use lambda_runtime::LambdaEvent;
    use search_filter_data::search_filter_data::SearchFilterData;
    use test_api::ApiGatewayV2httpRequestProxy;

    #[tokio::test]
    #[rstest::rstest]
    #[case(Some("price"), Some("asc"), Some("5"), Some("20"))]
    #[case(Some("created"), Some("desc"), None, None)]
    #[case(None, None, Some("7"), None)]
    #[case(Some("updated"), Some("desc"), None, Some("10"))]
    #[case(None, None, None, None)]
    async fn should_handle_request(
        #[case] sort: Option<&str>,
        #[case] order: Option<&str>,
        #[case] page_from: Option<&str>,
        #[case] page_size: Option<&str>,
    ) {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .try_query_string_parameter("sort", sort)
                .try_query_string_parameter("order", order)
                .try_query_string_parameter("from", page_from)
                .try_query_string_parameter("size", page_size)
                .body_serde(&Faker.fake::<SearchFilterData>())
                .build(),
            context: Default::default(),
        };

        let mut service = MockQueryItemService::default();
        service.expect_search_items().return_once(|_, _, page| {
            let count = page.map(|page| page.size).unwrap_or(20) as usize;
            let search_result = PaginatedResult {
                items: fake::vec![LocalizedItemView; count],
                total: Some(789),
                next_after: None,
                page: Page { from: 5, size: 0 },
            };
            Box::pin(async move { Ok(search_result) })
        });
        let response = handler(lambda_event, &service).await.unwrap();

        assert_eq!(200, response.status_code);
    }
}
