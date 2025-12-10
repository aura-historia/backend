use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use cognito::access_token_verifier_service::AccessTokenVerifierService;
use common::{
    api::{
        api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder,
        error::{ApiError, log_api_error},
        error_code::BAD_BODY_VALUE,
    },
    pagination::cursor::{
        CursoredResult,
        api::{JsonCursoredData, extract_json_cursor_query},
    },
    personalized::{Personalized, api::PersonalizedData},
    sort::api::extract_sort_query,
};
use lambda_runtime::LambdaEvent;
use product::{
    core::sort_product_field::SortProductField,
    data::{product_search_data::ProductSearchData, user_state_data::ProductUserStateData},
};
use product::{core::user_state::ProductUserState, service::query_service::QueryProductService};
use product::{
    data::{get_data::GetProductData, sort_product_field_data::SortProductFieldData},
    service::personalization_service::ProductPersonalizationService,
};

#[tracing::instrument(
    skip(event, query_product_service, access_token_verifier_service, product_personalization_service),
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
    query_product_service: &impl QueryProductService,
    access_token_verifier_service: &(impl AccessTokenVerifierService + Sync),
    product_personalization_service: &impl ProductPersonalizationService,
) -> Result<ApiGatewayV2httpResponse, lambda_runtime::Error> {
    match handle(
        event,
        query_product_service,
        access_token_verifier_service,
        product_personalization_service,
    )
    .await
    {
        Ok(response) => Ok(response),
        Err(err) => {
            log_api_error(&err);
            Ok(ApiGatewayV2httpResponse::from(err))
        }
    }
}

// POST /api/v1/products/search
pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl QueryProductService,
    access_token_verifier_service: &(impl AccessTokenVerifierService + Sync),
    product_personalization_service: &impl ProductPersonalizationService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id_opt = access_token_verifier_service
        .verify_extract_user_id(&event.payload.headers)
        .await?;
    if let Some(user_id) = user_id_opt {
        tracing::Span::current().record("userId", user_id.to_string());
    }

    let sort = extract_sort_query::<SortProductFieldData>(&event.payload.query_string_parameters)?
        .map(|sort_data| sort_data.map(SortProductField::from));
    let cursor =
        extract_json_cursor_query(&event.payload.query_string_parameters)?.unwrap_or_default();
    let body = event
        .payload
        .body
        .filter(|str| !str.is_empty())
        .ok_or_else(|| {
            let err_msg = "Body cannot be empty";
            ApiError::bad_request(BAD_BODY_VALUE, err_msg.into()).with_detail(err_msg)
        })?;
    let product_search_data: ProductSearchData = serde_json::from_str(&body).map_err(|err| {
        let err_msg = err.to_string();
        ApiError::bad_request(BAD_BODY_VALUE, Box::new(err)).with_detail(err_msg)
    })?;

    let product_search = product_search_data.into();
    let search_result = service
        .search_products(&product_search, &sort, &Some(cursor))
        .await?;
    let cursored_result = match user_id_opt {
        Some(user_id) => {
            let personalized_products = product_personalization_service
                .personalize_all_watchlist(&user_id, search_result.items)
                .await?
                .into_iter()
                .map(|personalized_item| Personalized {
                    item: personalized_item.item,
                    user_state: personalized_item
                        .user_state
                        .map(|watchlist| ProductUserState { watchlist }),
                })
                .collect();
            CursoredResult {
                items: personalized_products,
                cursor: search_result.cursor,
                total: search_result.total,
            }
        }
        None => search_result.map_item(|item| Personalized {
            item,
            user_state: None,
        }),
    };

    let json_cursored_data: JsonCursoredData<
        PersonalizedData<GetProductData, ProductUserStateData>,
    > = JsonCursoredData::from(cursored_result);

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .body_serde(json_cursored_data)?
        .build())
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_arguments)]
mod tests {
    use rstest;

    use crate::handler;
    use cognito::access_token_verifier_service::MockAccessTokenVerifierService;
    use common::pagination::cursor::Cursor;
    use common::pagination::cursor::CursoredResult;
    use fake::Fake;
    use fake::Faker;
    use lambda_runtime::LambdaEvent;
    use product::core::product::LocalizedProductView;
    use product::data::product_search_data::ProductSearchData;
    use product::service::personalization_service::MockProductPersonalizationService;
    use product::service::query_service::MockQueryProductService;
    use serde_json::json;
    use test_api::ApiGatewayV2httpRequestProxy;

    #[tokio::test]
    #[trace]
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
                .body_serde(&Faker.fake::<ProductSearchData>())
                .build(),
            context: Default::default(),
        };

        let product_personalization_service = MockProductPersonalizationService::default();
        let mut access_token_verifier_service = MockAccessTokenVerifierService::default();
        access_token_verifier_service
            .expect_verify_extract_user_id()
            .return_once(|_| Box::pin(async { Ok(None) }));
        let mut query_product_service = MockQueryProductService::default();
        query_product_service
            .expect_search_products()
            .return_once(|_, _, cursor| {
                let count = cursor.as_ref().map(|cursor| cursor.size).unwrap_or(20) as usize;
                let search_result = CursoredResult {
                    items: fake::vec![LocalizedProductView;count],
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
            &query_product_service,
            &access_token_verifier_service,
            &product_personalization_service,
        )
        .await
        .unwrap();

        assert_eq!(200, response.status_code);
    }
}
