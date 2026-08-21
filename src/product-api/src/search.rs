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
use embedding::{EmbeddingGenerator, EmbeddingText};
use lambda_runtime::LambdaEvent;
use product::core::sort_product_field::SortProductField;
use product::data::sort_product_field_data::SortProductFieldData;
use product::data::{
    get_summary_data::GetProductSummaryData, product_search_data::ProductSearchData,
    user_state_data::ProductUserStateData,
};
use product::service::query_service::QueryProductService;
use product_personalization::service::ProductPersonalizationService;
use tracing::warn;

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl QueryProductService,
    embedding_service: Option<&dyn EmbeddingGenerator>,
    access_token_verifier_service: &(impl AccessTokenVerifierService + Sync),
    product_personalization_service: &impl ProductPersonalizationService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id_opt = access_token_verifier_service
        .verify_extract_user_id(&event.payload.headers)
        .await
        .map_err(crate::map_access_token_error)?;
    if let Some(user_id) = user_id_opt {
        tracing::Span::current().record("userId", user_id.to_string());
    }

    let sort = extract_sort_query::<SortProductFieldData>(&event.payload.query_string_parameters)?
        .map(|sort_data| sort_data.map(SortProductField::from));
    let cursor =
        extract_json_cursor_query(&event.payload.query_string_parameters)?.unwrap_or_default();
    let product_search_data: ProductSearchData =
        if event.payload.route_key.as_deref() == Some("GET /api/v1/products") {
            let query = event
                .payload
                .raw_query_string
                .clone()
                .filter(|query| !query.is_empty())
                .unwrap_or_else(|| {
                    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
                    for (key, value) in event.payload.query_string_parameters.iter() {
                        serializer.append_pair(key, value);
                    }
                    serializer.finish()
                });
            serde_qs::from_str(&query).map_err(|err| {
                let err_msg = err.to_string();
                ApiError::bad_request(BAD_BODY_VALUE, Box::new(err)).with_detail(err_msg)
            })?
        } else {
            let body = event
                .payload
                .body
                .filter(|str| !str.is_empty())
                .ok_or_else(|| {
                    let err_msg = "Body cannot be empty";
                    ApiError::bad_request(BAD_BODY_VALUE, err_msg.into()).with_detail(err_msg)
                })?;
            serde_json::from_str(&body).map_err(|err| {
                let err_msg = err.to_string();
                ApiError::bad_request(BAD_BODY_VALUE, Box::new(err)).with_detail(err_msg)
            })?
        };

    let product_search: product::core::product_search::ProductSearch = product_search_data.into();

    // OpenSearch-native hybrid retrieval is only chosen when:
    //   * an embedding service is configured (Lambda has Vertex ADC configured), AND
    //   * the request carries at least one non-empty textual `product_query`, AND
    //   * the user did not request a non-score sort (e.g. price/created/updated).
    // Otherwise we fall back to the existing pure-BM25 path.
    let use_hybrid = embedding_service.is_some()
        && product_search
            .product_query
            .iter()
            .any(|q| !q.as_ref().trim().is_empty())
        && matches!(
            sort.as_ref().map(|s| s.sort),
            None | Some(SortProductField::Score)
        );

    let search_result = if use_hybrid {
        let es = embedding_service.expect("guarded above");
        let query_text = product_search
            .product_query
            .iter()
            .map(AsRef::as_ref)
            .collect::<Vec<_>>()
            .join(" ");
        let embedding = match EmbeddingText::new(query_text) {
            Ok(text) => es.embed_search_query(&text).await,
            Err(error) => Err(error),
        };
        match embedding {
            Ok(embedding) => {
                service
                    .search_products_hybrid(
                        &product_search,
                        &embedding.into_values(),
                        &Some(cursor),
                    )
                    .await?
            }
            Err(err) => {
                // Fail-open: log and fall back to BM25 so we never break the search path
                // because of an embedding-service hiccup.
                warn!(error = %err, "query embedding failed; falling back to BM25 path");
                service
                    .search_products(&product_search, &sort, &Some(cursor))
                    .await?
            }
        }
    } else {
        service
            .search_products(&product_search, &sort, &Some(cursor))
            .await?
    };

    let cursored_result = match user_id_opt {
        Some(user_id) => {
            let personalized_products = product_personalization_service
                .personalize_all(&user_id, search_result.items)
                .await?
                .into_iter()
                .map(|personalized| {
                    let consent = personalized
                        .user_state
                        .clone()
                        .map(|s| s.prohibited_content.consent)
                        .unwrap_or(false);
                    Personalized {
                        item: GetProductSummaryData::from_view(personalized.item, consent),
                        user_state: personalized.user_state.map(ProductUserStateData::from),
                    }
                })
                .collect();
            CursoredResult {
                items: personalized_products,
                cursor: search_result.cursor,
                total: search_result.total,
            }
        }
        None => search_result.map_item(|item| Personalized {
            item: GetProductSummaryData::from_view(item, false),
            user_state: None,
        }),
    };

    let json_cursored_data: JsonCursoredData<
        PersonalizedData<GetProductSummaryData, ProductUserStateData>,
    > = JsonCursoredData::from(cursored_result);

    let response_builder = ApiGatewayV2HttpResponseBuilder::json(200);
    let response_builder = match (event.payload.route_key.as_deref(), user_id_opt) {
        (Some("GET /api/v1/products"), Some(_)) => {
            response_builder.cache_control("no-store", None, None)
        }
        (Some("GET /api/v1/products"), None) => {
            response_builder.cache_control("public", Some(60), Some(300))
        }
        _ => response_builder,
    };

    Ok(response_builder.body_serde(json_cursored_data)?.build())
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
mod tests {
    use super::handle;
    use cognito::access_token_verifier_service::MockAccessTokenVerifierService;
    use common::pagination::cursor::Cursor;
    use common::pagination::cursor::CursoredResult;
    use fake::Fake;
    use fake::Faker;
    use lambda_runtime::LambdaEvent;
    use product::core::product::LocalizedProductView;
    use product::data::product_search_data::ProductSearchData;
    use product::service::query_service::MockQueryProductService;
    use product_personalization::service::MockProductPersonalizationService;
    use serde_json::json;
    use test_api::ApiGatewayV2httpRequestProxy;

    #[tokio::test]
    #[rstest::rstest]
    #[case(Some("price"), Some("asc"))]
    #[case(Some("created"), Some("desc"))]
    #[case(None, None)]
    #[case(Some("updated"), Some("desc"))]
    #[case(None, None)]
    #[trace]
    async fn should_handle_request_when_anon(
        #[case] sort: Option<&str>,
        #[case] order: Option<&str>,
    ) {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .route_key("POST /api/v1/products/search")
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
        let response = handle(
            lambda_event,
            &query_product_service,
            None,
            &access_token_verifier_service,
            &product_personalization_service,
        )
        .await
        .unwrap();

        assert_eq!(200, response.status_code);
        assert!(response.headers.get(http::header::CACHE_CONTROL).is_none());
    }

    #[tokio::test]
    async fn should_handle_get_simple_search_with_query_params() {
        let search = ProductSearchData {
            language: common::language::data::LanguageData::En,
            currency: common::currency::data::CurrencyData::Eur,
            product_query: vec!["chair".try_into().unwrap()],
            enhanced_search_description: None,
            exclude_product_id_query: Default::default(),
            shop_name_query: Default::default(),
            exclude_shop_name_query: Default::default(),
            seller_name_query: Default::default(),
            exclude_seller_name_query: Default::default(),
            shop_slug_id_query: Default::default(),
            exclude_shop_slug_id_query: Default::default(),
            seller_slug_id_query: Default::default(),
            exclude_seller_slug_id_query: Default::default(),
            shop_type_query: Default::default(),
            country_query: Default::default(),
            continent_query: Default::default(),
            geo_address_distance_query: None,
            price_query: None,
            state_query: Default::default(),

            created_query: None,
            updated_query: None,
            auction_start_query: None,
            auction_end_query: None,
        };
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/products")
                .raw_query_string("language=en&currency=EUR&productQuery=chair".to_string())
                .query_string_parameter("language", "en")
                .query_string_parameter("currency", "EUR")
                .query_string_parameter("productQuery", "chair")
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
            .withf(move |actual, _, _| actual == &search.clone().into())
            .return_once(|_, _, _| {
                Box::pin(async move {
                    Ok(CursoredResult {
                        items: vec![],
                        cursor: Cursor::default(),
                        total: Some(0),
                    })
                })
            });

        let response = handle(
            lambda_event,
            &query_product_service,
            None,
            &access_token_verifier_service,
            &product_personalization_service,
        )
        .await
        .unwrap();

        assert_eq!(200, response.status_code);
        assert_eq!(
            "public, max-age=60, s-maxage=300",
            response
                .headers
                .get(http::header::CACHE_CONTROL)
                .unwrap()
                .to_str()
                .unwrap()
        );
    }
}
