use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::{
    api::{
        api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder,
        collection::{OffsetLimitPaginatedData, PaginationData},
        error::ApiError,
        error_code::BAD_BODY_VALUE,
    },
    page::{Page, api::extract_page_query_u16},
    sort::api::extract_sort_query,
};
use lambda_runtime::LambdaEvent;
use shop_core::sort_shop_field::SortShopField;
use shop_data::{
    get_shop_data::GetShopData, shop_search_data::ShopSearchData,
    sort_shop_field_data::SortShopFieldData,
};
use shop_opensearch::shop_search::ShopSearch;
use shop_service::query_service::QueryShopService;

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
    service: &impl QueryShopService,
) -> Result<ApiGatewayV2httpResponse, lambda_runtime::Error> {
    match handle(event, service).await {
        Ok(response) => Ok(response),
        Err(err) => Ok(ApiGatewayV2httpResponse::from(err)),
    }
}

// POST /api/v1/shops/search
pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl QueryShopService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let sort = extract_sort_query::<SortShopFieldData>(&event.payload.query_string_parameters)?
        .map(|sort_data| sort_data.map(SortShopField::from));
    let page = extract_page_query_u16(&event.payload.query_string_parameters)?
        .unwrap_or(Page { from: 0, size: 21 });
    let body = event
        .payload
        .body
        .filter(|str| !str.is_empty())
        .ok_or_else(|| {
            ApiError::bad_request(BAD_BODY_VALUE).with_message("Body cannot be empty. If you want to search without any restrictions, supply the body '{}'.")
        })?;
    let search_data: ShopSearchData = serde_json::from_str(&body)
        .map_err(|err| ApiError::bad_request(BAD_BODY_VALUE).with_message(err.to_string()))?;

    let search = ShopSearch {
        shop_name_query: search_data.shop_name_query,
        created: search_data.created,
        updated: search_data.updated,
    };
    let search_result = service.search_shops(&search, &sort, &Some(page)).await?;

    let items = search_result
        .hits
        .into_iter()
        .map(GetShopData::from)
        .collect::<Vec<_>>();
    let pagination = PaginationData {
        from: page.from as u64,
        size: page.size as u64,
        total: Some(search_result.total),
        next: Some(page.from as u64 + page.size as u64),
    };
    let collection = OffsetLimitPaginatedData { items, pagination };

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .body_serde(collection)?
        .cors()
        .build())
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
mod tests {
    use crate::handler;
    use common::opensearch::search_result::SearchResult;
    use fake::Fake;
    use fake::Faker;
    use lambda_runtime::LambdaEvent;
    use shop_core::shop::Shop;
    use shop_data::shop_search_data::ShopSearchData;
    use shop_service::query_service::MockQueryShopService;
    use test_api::ApiGatewayV2httpRequestProxy;
    use test_api::extract_apigw_response_json_body;

    #[tokio::test]
    #[rstest::rstest]
    #[case(Some("name"), Some("asc"), Some("5"), Some("20"))]
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
                .body_serde(&Faker.fake::<ShopSearchData>())
                .build(),
            context: Default::default(),
        };

        let mut service = MockQueryShopService::default();
        service.expect_search_shops().return_once(|_, _, page| {
            let count = page.map(|page| page.size).unwrap_or(20) as usize;
            let search_result = SearchResult {
                hits: fake::vec![Shop; count],
                total: 789,
            };
            Box::pin(async move { Ok(search_result) })
        });
        let response = handler(lambda_event, &service).await.unwrap();

        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    #[rstest::rstest]
    #[case(None, None)]
    async fn should_default_page_sizing_when_none_given(
        #[case] page_from: Option<&str>,
        #[case] page_size: Option<&str>,
    ) {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .try_query_string_parameter("from", page_from)
                .try_query_string_parameter("size", page_size)
                .body_serde(&Faker.fake::<ShopSearchData>())
                .build(),
            context: Default::default(),
        };

        let mut service = MockQueryShopService::default();
        service.expect_search_shops().return_once(|_, _, page| {
            let count = page.map(|page| page.size).unwrap() as usize;
            let search_result = SearchResult {
                hits: fake::vec![Shop; count],
                total: 789,
            };
            Box::pin(async move { Ok(search_result) })
        });
        let response = handler(lambda_event, &service).await.unwrap();

        assert_eq!(200, response.status_code);
        let json = extract_apigw_response_json_body!(response);
        assert_eq!(0, json["pagination"]["from"]);
        assert_eq!(21, json["pagination"]["size"]);
        assert_eq!(789, json["pagination"]["total"]);
    }

    #[tokio::test]
    async fn should_allow_empty_shop_search() {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .body_serde(&ShopSearchData::default())
                .build(),
            context: Default::default(),
        };

        let mut service = MockQueryShopService::default();
        service.expect_search_shops().return_once(|_, _, page| {
            let count = page.map(|page| page.size).unwrap() as usize;
            let search_result = SearchResult {
                hits: fake::vec![Shop; count],
                total: 789,
            };
            Box::pin(async move { Ok(search_result) })
        });
        let response = handler(lambda_event, &service).await.unwrap();

        assert_eq!(200, response.status_code);
    }
}
