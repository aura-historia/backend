use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use cognito::access_token_verifier_service::AccessTokenVerifierService;
use common::{
    api::{
        api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder, error::ApiError,
        error_code::BAD_BODY_VALUE,
    },
    pagination::cursor::{
        CursoredResult,
        api::{JsonCursoredData, extract_json_cursor_query},
    },
    personalized::{Personalized, api::PersonalizedData},
    sort::api::extract_sort_query,
};
use item::{
    core::sort_item_field::SortItemField,
    data::{item_search_data::ItemSearchData, user_state_data::ItemUserStateData},
};
use item::{core::user_state::ItemUserState, service::query_service::QueryItemService};
use item::{
    data::{get_data::GetItemData, sort_item_field_data::SortItemFieldData},
    service::personalization_service::ItemPersonalizationService,
};
use lambda_runtime::LambdaEvent;

#[tracing::instrument(
    skip(event, query_item_service, access_token_verifier_service, item_personalization_service),
    fields(
        requestId = %event.context.request_id,
        path = &event.payload.raw_path,
        query = &event.payload.raw_query_string,
    )
)]
pub async fn handler(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    query_item_service: &impl QueryItemService,
    access_token_verifier_service: &(impl AccessTokenVerifierService + Sync),
    item_personalization_service: &impl ItemPersonalizationService,
) -> Result<ApiGatewayV2httpResponse, lambda_runtime::Error> {
    match handle(
        event,
        query_item_service,
        access_token_verifier_service,
        item_personalization_service,
    )
    .await
    {
        Ok(response) => Ok(response),
        Err(err) => Ok(ApiGatewayV2httpResponse::from(err)),
    }
}

// POST /api/v1/items/search
pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl QueryItemService,
    access_token_verifier_service: &(impl AccessTokenVerifierService + Sync),
    item_personalization_service: &impl ItemPersonalizationService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id_opt = access_token_verifier_service
        .verify_extract_user_id(&event.payload.headers)
        .await?;
    let sort = extract_sort_query::<SortItemFieldData>(&event.payload.query_string_parameters)?
        .map(|sort_data| sort_data.map(SortItemField::from));
    let cursor =
        extract_json_cursor_query(&event.payload.query_string_parameters)?.unwrap_or_default();
    let body = event
        .payload
        .body
        .filter(|str| !str.is_empty())
        .ok_or_else(|| {
            let err_msg = "Body cannot be empty";
            ApiError::bad_request(BAD_BODY_VALUE, err_msg.into()).with_message(err_msg)
        })?;
    let item_search_data: ItemSearchData = serde_json::from_str(&body).map_err(|err| {
        let err_msg = err.to_string();
        ApiError::bad_request(BAD_BODY_VALUE, Box::new(err)).with_message(err_msg)
    })?;

    let item_search = item_search_data.into();
    let search_result = service
        .search_items(&item_search, &sort, &Some(cursor))
        .await?;
    let cursored_result = match user_id_opt {
        Some(user_id) => {
            let personalized_items = item_personalization_service
                .personalize_all_watchlist(&user_id, search_result.items)
                .await?
                .into_iter()
                .map(|personalized_item| Personalized {
                    item: personalized_item.item,
                    user_state: personalized_item
                        .user_state
                        .map(|watchlist| ItemUserState { watchlist }),
                })
                .collect();
            CursoredResult {
                items: personalized_items,
                cursor: search_result.cursor,
                total: search_result.total,
            }
        }
        None => search_result.map_item(|item| Personalized {
            item,
            user_state: None,
        }),
    };

    let json_cursored_data: JsonCursoredData<PersonalizedData<GetItemData, ItemUserStateData>> =
        JsonCursoredData::from(cursored_result);

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .body_serde(json_cursored_data)?
        .build())
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
mod tests {
    use crate::handler;
    use cognito::access_token_verifier_service::MockAccessTokenVerifierService;
    use common::pagination::cursor::Cursor;
    use common::pagination::cursor::CursoredResult;
    use fake::Fake;
    use fake::Faker;
    use item::core::item::LocalizedItemView;
    use item::data::item_search_data::ItemSearchData;
    use item::service::personalization_service::MockItemPersonalizationService;
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
    async fn should_handle_request_when_anon(
        #[case] sort: Option<&str>,
        #[case] order: Option<&str>,
    ) {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .try_query_string_parameter("sort", sort)
                .try_query_string_parameter("order", order)
                .body_serde(&Faker.fake::<ItemSearchData>())
                .build(),
            context: Default::default(),
        };

        let item_personalization_service = MockItemPersonalizationService::default();
        let mut access_token_verifier_service = MockAccessTokenVerifierService::default();
        access_token_verifier_service
            .expect_verify_extract_user_id()
            .return_once(|_| Box::pin(async { Ok(None) }));
        let mut query_item_service = MockQueryItemService::default();
        query_item_service
            .expect_search_items()
            .return_once(|_, _, cursor| {
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
        let response = handler(
            lambda_event,
            &query_item_service,
            &access_token_verifier_service,
            &item_personalization_service,
        )
        .await
        .unwrap();

        assert_eq!(200, response.status_code);
    }
}
