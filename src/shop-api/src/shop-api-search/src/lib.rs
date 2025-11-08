use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::{
    api::{
        api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder,
        error::{ApiError, log_api_error},
        error_code::BAD_BODY_VALUE,
    },
    pagination::cursor::{
        Cursor,
        api::{JsonCursoredData, extract_json_cursor_query},
    },
    sort::api::extract_sort_query,
};
use lambda_runtime::LambdaEvent;
use shop::core::sort_shop_field::SortShopField;
use shop::data::{
    get_shop_data::GetShopData, shop_search_data::ShopSearchData,
    sort_shop_field_data::SortShopFieldData,
};
use shop::opensearch::shop_search::ShopSearch;
use shop::service::query_service::QueryShopService;

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
    )
)]
pub async fn handler(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl QueryShopService,
) -> Result<ApiGatewayV2httpResponse, lambda_runtime::Error> {
    match handle(event, service).await {
        Ok(response) => Ok(response),
        Err(err) => {
            log_api_error(&err);
            Ok(ApiGatewayV2httpResponse::from(err))
        }
    }
}

// POST /api/v1/shops/search
pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl QueryShopService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let sort = extract_sort_query::<SortShopFieldData>(&event.payload.query_string_parameters)?
        .map(|sort_data| sort_data.map(SortShopField::from));
    let cursor =
        extract_json_cursor_query(&event.payload.query_string_parameters)?.unwrap_or(Cursor {
            size: 21,
            search_after: None,
        });
    let body = event
        .payload
        .body
        .filter(|str| !str.is_empty())
        .ok_or_else(|| {
            let err_msg = "Body cannot be empty. If you want to search without any restrictions, supply the body '{}'.";
            ApiError::bad_request(BAD_BODY_VALUE, err_msg.into()).with_message(err_msg)
        })?;
    let search_data: ShopSearchData = serde_json::from_str(&body).map_err(|err| {
        let err_msg = err.to_string();
        ApiError::bad_request(BAD_BODY_VALUE, Box::new(err)).with_message(err_msg)
    })?;

    let search = ShopSearch {
        shop_name_query: search_data.shop_name_query,
        created: search_data.created,
        updated: search_data.updated,
    };
    let search_result = service
        .search_shops(&search, &sort, &Some(cursor))
        .await?
        .map_item(GetShopData::from);
    let search_result_data: JsonCursoredData<GetShopData> = JsonCursoredData::from(search_result);

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .body_serde(search_result_data)?
        .build())
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
mod tests {
    use crate::handler;
    use common::pagination::cursor::{Cursor, CursoredResult};
    use fake::Fake;
    use fake::Faker;
    use lambda_runtime::LambdaEvent;
    use shop::core::shop::Shop;
    use shop::data::shop_search_data::ShopSearchData;
    use shop::service::query_service::MockQueryShopService;
    use test_api::ApiGatewayV2httpRequestProxy;

    #[tokio::test]
    #[rstest::rstest]
    #[case(Some("name"), Some("asc"))]
    #[case(Some("created"), Some("desc"))]
    #[case(None, None)]
    #[case(None, None)]
    async fn should_handle_request(#[case] sort: Option<&str>, #[case] order: Option<&str>) {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .try_query_string_parameter("sort", sort)
                .try_query_string_parameter("order", order)
                .body_serde(&Faker.fake::<ShopSearchData>())
                .build(),
            context: Default::default(),
        };

        let mut service = MockQueryShopService::default();
        service
            .expect_search_shops()
            .return_once(move |_, _, cursor| {
                let count = cursor.clone().map(|cursor| cursor.size).unwrap_or(20) as usize;
                let search_result = CursoredResult {
                    items: fake::vec![Shop; count],
                    total: Some(789),
                    cursor: Cursor {
                        size: count as u64,
                        search_after: None,
                    },
                };
                Box::pin(async move { Ok(search_result) })
            });
        let response = handler(lambda_event, &service).await.unwrap();

        assert_eq!(200, response.status_code);
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
        service.expect_search_shops().return_once(|_, _, cursor| {
            let count = cursor.clone().map(|cursor| cursor.size).unwrap_or(20) as usize;
            let search_result = CursoredResult {
                items: fake::vec![Shop; count],
                total: Some(789),
                cursor: Cursor {
                    size: count as u64,
                    search_after: None,
                },
            };
            Box::pin(async move { Ok(search_result) })
        });
        let response = handler(lambda_event, &service).await.unwrap();

        assert_eq!(200, response.status_code);
    }
}
