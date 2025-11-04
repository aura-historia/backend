use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::{
    api::{
        api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder, error::ApiError,
        error_code::BAD_BODY_VALUE,
    },
    pagination::cursor::api::{JsonCursoredData, extract_json_cursor_query},
    sort::api::extract_sort_query,
};
use item::data::{get_data::GetItemData, sort_item_field_data::SortItemFieldData};
use item::service::query_service::QueryItemService;
use item::{core::sort_item_field::SortItemField, data::item_search_data::ItemSearchData};
use lambda_runtime::LambdaEvent;

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
    let cursor =
        extract_json_cursor_query(&event.payload.query_string_parameters)?.unwrap_or_default();
    let body = event
        .payload
        .body
        .filter(|str| !str.is_empty())
        .ok_or_else(|| {
            ApiError::bad_request(BAD_BODY_VALUE).with_message("Body cannot be empty")
        })?;
    let item_search_data: ItemSearchData = serde_json::from_str(&body)
        .map_err(|err| ApiError::bad_request(BAD_BODY_VALUE).with_message(err.to_string()))?;

    let item_search = item_search_data.into();
    let search_result = service
        .search_items(&item_search, &sort, &Some(cursor))
        .await?
        .map_item(GetItemData::from);
    let collection = JsonCursoredData::from(search_result);

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .body_serde(collection)?
        .build())
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
mod tests {
    use crate::handler;
    use common::pagination::cursor::Cursor;
    use common::pagination::cursor::CursoredResult;
    use fake::Fake;
    use fake::Faker;
    use item::core::item::LocalizedItemView;
    use item::data::item_search_data::ItemSearchData;
    use item::service::query_service::MockQueryItemService;
    use lambda_runtime::LambdaEvent;
    use serde_json::json;
    use test_api::ApiGatewayV2httpRequestProxy;

    #[tokio::test]
    #[rstest::rstest]
    #[case(Some("price"), Some("asc"))]
    #[case(Some("created"), Some("desc"))]
    #[case(None, None)]
    #[case(Some("updated"), Some("desc"))]
    #[case(None, None)]
    async fn should_handle_request(#[case] sort: Option<&str>, #[case] order: Option<&str>) {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .try_query_string_parameter("sort", sort)
                .try_query_string_parameter("order", order)
                .body_serde(&Faker.fake::<ItemSearchData>())
                .build(),
            context: Default::default(),
        };

        let mut service = MockQueryItemService::default();
        service.expect_search_items().return_once(|_, _, cursor| {
            let count = cursor.as_ref().map(|cursor| cursor.size).unwrap_or(20) as usize;
            let search_result = CursoredResult {
                items: fake::vec![LocalizedItemView;count],
                cursor: Cursor {
                    size: count as u64,
                    search_after: Some(json!(["Booooop", 123465])),
                },
                total: Some(789),
            };
            Box::pin(async move { Ok(search_result) })
        });
        let response = handler(lambda_event, &service).await.unwrap();

        assert_eq!(200, response.status_code);
    }
}
