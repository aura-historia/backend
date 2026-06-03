use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::{
    api::{
        api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder, error::ApiError,
        error_code::BAD_QUERY_PARAMETER_VALUE,
    },
    currency::data::api::extract_currency_query,
    language::data::api::extract_language_query,
    pagination::cursor::{
        Cursor, CursoredResult,
        api::{JsonCursoredData, extract_json_cursor_query},
    },
    personalized::{Personalized, api::PersonalizedData},
    user_id::api::extract_user_id_request_context,
    user_search_filter_id::api::extract_user_search_filter_id_path,
};
use lambda_runtime::LambdaEvent;
use product::core::sort_product_field::SortProductField;
use product::core::user_state::SearchFilterUserState;
use product::data::{
    get_summary_data::GetProductSummaryData, user_state_data::ProductUserStateData,
};
use product::service::query_service::QueryProductService;
use product_personalization::service::ProductPersonalizationService;
use product_pipeline_embed_text::service::MultimodalEmbeddingService;
use search_filter::service::enhanced_search_match_service::EnhancedSearchMatchService;
use search_filter::service::user_search_filter_service::UserSearchFilterService;
use tracing::warn;

const MAX_PAGE_SIZE: u64 = 10;

#[allow(clippy::too_many_arguments)]
pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl UserSearchFilterService,
    query_service: &(impl QueryProductService + Sync),
    embedding_service: Option<&(dyn MultimodalEmbeddingService + Sync + Send)>,
    enhanced_match_service: Option<&(dyn EnhancedSearchMatchService + Sync + Send)>,
    personalization_service: &(impl ProductPersonalizationService + Sync),
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id = extract_user_id_request_context(&event.payload.request_context)?;
    tracing::Span::current().record("userId", user_id.to_string());
    let search_filter_id = extract_user_search_filter_id_path(&event.payload.path_parameters)?;
    let language = extract_language_query(&event.payload.query_string_parameters)?;
    let currency = extract_currency_query(&event.payload.query_string_parameters)?;

    // Forbid paging: reject any searchAfter parameter
    let cursor_from_query = extract_json_cursor_query(&event.payload.query_string_parameters)?;
    if let Some(ref cursor) = cursor_from_query
        && cursor.search_after.is_some()
    {
        return Err(ApiError::bad_request(
            BAD_QUERY_PARAMETER_VALUE,
            "Paging is not supported for this endpoint.".into(),
        )
        .with_query_field("searchAfter")
        .with_detail(
            "Remove the 'searchAfter' parameter; this endpoint does not support pagination.",
        ));
    }
    let size = cursor_from_query
        .map(|c| c.size)
        .unwrap_or(MAX_PAGE_SIZE)
        .min(MAX_PAGE_SIZE);

    // Load the user search filter to get search criteria and optional enhanced description
    let search_filter = service
        .find_user_search_filter(&user_id, &search_filter_id)
        .await?;

    // Build product search from the filter's search, overriding language and currency
    let mut product_search = search_filter.search.clone();
    product_search.language = language.into();
    product_search.currency = currency.into();

    // Adaptive hybrid retrieval: only when an embedding service is available and the filter
    // contains a non-empty textual product_query.
    let sort = None::<common::sort::Sort<SortProductField>>;
    let use_hybrid = embedding_service.is_some()
        && product_search
            .product_query
            .as_ref()
            .map(|q| !q.as_ref().trim().is_empty())
            .unwrap_or(false);

    let cursor = Some(Cursor {
        size,
        search_after: None,
    });

    let search_result = if use_hybrid {
        let es = embedding_service.expect("guarded above");
        let query_text = product_search
            .product_query
            .as_ref()
            .map(|q| q.as_ref().to_string())
            .unwrap_or_default();
        match es.embed_query(&query_text).await {
            Ok(embedding) => {
                query_service
                    .search_products_with_dynamic_semantics(&product_search, &embedding, &cursor)
                    .await?
            }
            Err(err) => {
                // Fail-open: log and fall back to BM25 so we never break the search path
                // because of an embedding-service hiccup.
                warn!(error = %err, "embed_query failed; falling back to BM25 path");
                query_service
                    .search_products(&product_search, &sort, &cursor)
                    .await?
            }
        }
    } else {
        query_service
            .search_products(&product_search, &sort, &cursor)
            .await?
    };

    // Personalize all products for the authenticated user
    let personalized = personalization_service
        .personalize_all(&user_id, search_result.items)
        .await?;

    // If the filter has an EnhancedSearchDescription, evaluate each product and
    // overwrite user_state.search_filter with the LLM match result.
    let filter_language: common::language::domain::Language = language.into();
    let items: Vec<Personalized<GetProductSummaryData, ProductUserStateData>> = if let Some(
        enhanced_desc,
    ) =
        &search_filter.enhanced_search_description
    {
        if let Some(match_service) = enhanced_match_service {
            let mut result_items = Vec::with_capacity(personalized.len());
            for p in personalized {
                let title = p.item.title.payload.clone();
                let description = p
                    .item
                    .description
                    .as_ref()
                    .map(|d| d.payload.clone())
                    .unwrap_or_else(|| product::core::description::Description::from(""));
                let images: Vec<_> = p.item.images.iter().take(5).cloned().collect();

                let match_evaluation = match match_service
                    .evaluate(
                        enhanced_desc,
                        &title,
                        &description,
                        filter_language,
                        &images,
                    )
                    .await
                {
                    Ok(result) => SearchFilterUserState {
                        matched: result.matches,
                        match_reason: result.reason,
                        ..Default::default()
                    },
                    Err(err) => {
                        warn!(
                            error = %err,
                            productId = %p.item.product_id,
                            "Enhanced search match evaluation failed. Returning product without match reason."
                        );
                        SearchFilterUserState::default()
                    }
                };

                let consent = p
                    .user_state
                    .as_ref()
                    .map(|s| s.prohibited_content.consent)
                    .unwrap_or(false);
                let user_state = p.user_state.map(|mut state| {
                    state.search_filter = match_evaluation;
                    state
                });

                result_items.push(Personalized {
                    item: GetProductSummaryData::from_view(p.item, consent),
                    user_state: user_state.map(ProductUserStateData::from),
                });
            }
            result_items
        } else {
            // EnhancedSearchDescription present but no match service available — fall through
            // to standard mapping so the endpoint still works without the optional
            // enhanced-match LLM configuration.
            convert_to_summary_response(personalized)
        }
    } else {
        // No enhanced description: act like the normal product-search endpoint.
        convert_to_summary_response(personalized)
    };

    let cursored_result = CursoredResult {
        items,
        cursor: search_result.cursor,
        total: search_result.total,
    };

    let json_cursored_data: JsonCursoredData<
        PersonalizedData<GetProductSummaryData, ProductUserStateData>,
    > = JsonCursoredData::from(cursored_result);

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .cache_control("no-store", None, None)
        .body_serde(json_cursored_data)?
        .build())
}

fn convert_to_summary_response(
    personalized: Vec<
        common::personalized::Personalized<
            product::core::product::LocalizedProductView,
            product::core::user_state::ProductUserState,
        >,
    >,
) -> Vec<Personalized<GetProductSummaryData, ProductUserStateData>> {
    personalized
        .into_iter()
        .map(|p| {
            let consent = p
                .user_state
                .as_ref()
                .map(|s| s.prohibited_content.consent)
                .unwrap_or(false);
            Personalized {
                item: GetProductSummaryData::from_view(p.item, consent),
                user_state: p.user_state.map(ProductUserStateData::from),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::handle;
    use common::{
        pagination::cursor::{Cursor, CursoredResult},
        user_id::UserId,
        user_search_filter_id::UserSearchFilterId,
    };
    use fake::{Fake, Faker};
    use http::header::CACHE_CONTROL;
    use lambda_runtime::LambdaEvent;
    use product::{
        core::product::LocalizedProductView, service::query_service::MockQueryProductService,
    };
    use product_personalization::service::MockProductPersonalizationService;
    use search_filter::{
        core::user_search_filter::UserSearchFilter,
        service::user_search_filter_service::MockUserSearchFilterService,
    };
    use serde_json::json;
    use test_api::ApiGatewayV2httpRequestProxy;

    fn filter_without_enhanced_description() -> UserSearchFilter {
        let mut filter: UserSearchFilter = Faker.fake();
        filter.enhanced_search_description = None;
        filter
    }

    fn filter_with_enhanced_description() -> UserSearchFilter {
        use search_filter::core::user_search_filter::EnhancedSearchDescription;
        let mut filter: UserSearchFilter = Faker.fake();
        filter.enhanced_search_description =
            Some(EnhancedSearchDescription::from("golden cufflinks"));
        filter
    }

    #[tokio::test]
    async fn should_200_when_success_without_enhanced_description() {
        let mut service = MockUserSearchFilterService::default();
        service
            .expect_find_user_search_filter()
            .return_once(|_, _| Box::pin(async { Ok(filter_without_enhanced_description()) }));
        let mut query_service = MockQueryProductService::default();
        query_service
            .expect_search_products()
            .return_once(|_, _, _| {
                Box::pin(async {
                    Ok(CursoredResult {
                        items: vec![],
                        cursor: Cursor::default(),
                        total: Some(0),
                    })
                })
            });
        let mut personalization_service = MockProductPersonalizationService::default();
        personalization_service
            .expect_personalize_all()
            .return_once(|_, _| Box::pin(async { Ok(vec![]) }));

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .jwt_claim("sub", UserId::new())
                .path_parameter("userSearchFilterId", UserSearchFilterId::new())
                .query_string_parameter("language", "de")
                .query_string_parameter("currency", "EUR")
                .build(),
            context: Default::default(),
        };

        let response = handle(
            lambda_event,
            &service,
            &query_service,
            None,
            None,
            &personalization_service,
        )
        .await
        .unwrap();

        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_200_when_success_with_enhanced_description() {
        use search_filter::service::enhanced_search_match_service::{
            EnhancedSearchMatchResult, MockEnhancedSearchMatchService,
        };

        let mut service = MockUserSearchFilterService::default();
        service
            .expect_find_user_search_filter()
            .return_once(|_, _| Box::pin(async { Ok(filter_with_enhanced_description()) }));

        let mut query_service = MockQueryProductService::default();
        query_service
            .expect_search_products()
            .return_once(|_, _, _| {
                Box::pin(async {
                    Ok(CursoredResult {
                        items: fake::vec![LocalizedProductView; 2],
                        cursor: Cursor::default(),
                        total: Some(2),
                    })
                })
            });

        let mut personalization_service = MockProductPersonalizationService::default();
        personalization_service
            .expect_personalize_all()
            .return_once(|_, products| {
                Box::pin(async move {
                    Ok(products
                        .into_iter()
                        .map(|p| common::personalized::Personalized {
                            item: p,
                            user_state: None,
                        })
                        .collect())
                })
            });

        let mut match_service = MockEnhancedSearchMatchService::default();
        match_service
            .expect_evaluate()
            .times(2)
            .returning(|_, _, _, _, _| {
                Box::pin(async {
                    Ok(EnhancedSearchMatchResult {
                        matches: true,
                        reason: Some(common::enhanced_match_reason::EnhancedMatchReason::from(
                            "Vintage cufflinks match",
                        )),
                    })
                })
            });

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .jwt_claim("sub", UserId::new())
                .path_parameter("userSearchFilterId", UserSearchFilterId::new())
                .query_string_parameter("language", "de")
                .query_string_parameter("currency", "EUR")
                .build(),
            context: Default::default(),
        };

        let response = handle(
            lambda_event,
            &service,
            &query_service,
            None,
            Some(&match_service),
            &personalization_service,
        )
        .await
        .unwrap();

        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_401_when_sub_missing() {
        let service = MockUserSearchFilterService::default();
        let query_service = MockQueryProductService::default();
        let personalization_service = MockProductPersonalizationService::default();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .path_parameter("userSearchFilterId", UserSearchFilterId::new())
                .query_string_parameter("language", "de")
                .query_string_parameter("currency", "EUR")
                .build(),
            context: Default::default(),
        };

        let actual = handle(
            lambda_event,
            &service,
            &query_service,
            None,
            None,
            &personalization_service,
        )
        .await
        .unwrap_err();

        assert_eq!(401, actual.status);
    }

    #[tokio::test]
    async fn should_400_when_search_after_provided() {
        let mut service = MockUserSearchFilterService::default();
        service.expect_find_user_search_filter().never();
        let query_service = MockQueryProductService::default();
        let personalization_service = MockProductPersonalizationService::default();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .jwt_claim("sub", UserId::new())
                .path_parameter("userSearchFilterId", UserSearchFilterId::new())
                .query_string_parameter("language", "de")
                .query_string_parameter("currency", "EUR")
                .query_string_parameter("searchAfter", "somevalue")
                .query_string_parameter("size", "5")
                .build(),
            context: Default::default(),
        };

        let actual = handle(
            lambda_event,
            &service,
            &query_service,
            None,
            None,
            &personalization_service,
        )
        .await
        .unwrap_err();

        assert_eq!(400, actual.status);
    }

    #[tokio::test]
    async fn should_cap_size_at_max_page_size() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU64, Ordering};

        let mut service = MockUserSearchFilterService::default();
        service
            .expect_find_user_search_filter()
            .return_once(|_, _| Box::pin(async { Ok(filter_without_enhanced_description()) }));

        let captured_size = Arc::new(AtomicU64::new(0));
        let captured_size_clone = captured_size.clone();

        let mut query_service = MockQueryProductService::default();
        query_service
            .expect_search_products()
            .return_once(move |_, _, cursor| {
                if let Some(c) = cursor {
                    captured_size_clone.store(c.size, Ordering::SeqCst);
                }
                Box::pin(async {
                    Ok(CursoredResult {
                        items: vec![],
                        cursor: Cursor::default(),
                        total: Some(0),
                    })
                })
            });

        let mut personalization_service = MockProductPersonalizationService::default();
        personalization_service
            .expect_personalize_all()
            .return_once(|_, _| Box::pin(async { Ok(vec![]) }));

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .jwt_claim("sub", UserId::new())
                .path_parameter("userSearchFilterId", UserSearchFilterId::new())
                .query_string_parameter("language", "de")
                .query_string_parameter("currency", "EUR")
                .query_string_parameter("size", "100") // request 100, should be capped at 10
                .build(),
            context: Default::default(),
        };

        let response = handle(
            lambda_event,
            &service,
            &query_service,
            None,
            None,
            &personalization_service,
        )
        .await
        .unwrap();

        assert_eq!(200, response.status_code);
        assert_eq!(10, captured_size.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn should_set_cache_control_to_no_store() {
        let mut service = MockUserSearchFilterService::default();
        service
            .expect_find_user_search_filter()
            .return_once(|_, _| Box::pin(async { Ok(filter_without_enhanced_description()) }));
        let mut query_service = MockQueryProductService::default();
        query_service
            .expect_search_products()
            .return_once(|_, _, _| {
                Box::pin(async {
                    Ok(CursoredResult {
                        items: vec![],
                        cursor: Cursor::default(),
                        total: Some(0),
                    })
                })
            });
        let mut personalization_service = MockProductPersonalizationService::default();
        personalization_service
            .expect_personalize_all()
            .return_once(|_, _| Box::pin(async { Ok(vec![]) }));

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .jwt_claim("sub", UserId::new())
                .path_parameter("userSearchFilterId", UserSearchFilterId::new())
                .query_string_parameter("language", "de")
                .query_string_parameter("currency", "EUR")
                .build(),
            context: Default::default(),
        };

        let response = handle(
            lambda_event,
            &service,
            &query_service,
            None,
            None,
            &personalization_service,
        )
        .await
        .unwrap();

        assert_eq!(200, response.status_code);
        assert_eq!(
            "no-store",
            response
                .headers
                .get(CACHE_CONTROL)
                .unwrap()
                .to_str()
                .unwrap()
        );
    }

    #[tokio::test]
    async fn should_use_hybrid_search_when_embedding_service_available_and_query_present() {
        use product_pipeline_embed_text::service::MockMultimodalEmbeddingService;

        let mut service = MockUserSearchFilterService::default();
        service
            .expect_find_user_search_filter()
            .return_once(|_, _| {
                Box::pin(async {
                    use common::query::text_query::TextQuery;
                    use product::core::product_search::ProductSearch;
                    let mut filter: UserSearchFilter = Faker.fake();
                    filter.enhanced_search_description = None;
                    filter.search = {
                        let mut search: ProductSearch = Faker.fake();
                        search.product_query = Some(TextQuery::try_from("antique vase").unwrap());
                        search
                    };
                    Ok(filter)
                })
            });

        let mut embedding_service = MockMultimodalEmbeddingService::default();
        embedding_service
            .expect_embed_query()
            .return_once(|_| Box::pin(async { Ok(vec![0.1f32; 768]) }));

        let mut query_service = MockQueryProductService::default();
        query_service
            .expect_search_products_with_dynamic_semantics()
            .return_once(|_, _, _| {
                Box::pin(async {
                    Ok(CursoredResult {
                        items: vec![],
                        cursor: Cursor {
                            size: 10,
                            search_after: Some(json!([0.99])),
                        },
                        total: Some(0),
                    })
                })
            });

        let mut personalization_service = MockProductPersonalizationService::default();
        personalization_service
            .expect_personalize_all()
            .return_once(|_, _| Box::pin(async { Ok(vec![]) }));

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .jwt_claim("sub", UserId::new())
                .path_parameter("userSearchFilterId", UserSearchFilterId::new())
                .query_string_parameter("language", "de")
                .query_string_parameter("currency", "EUR")
                .build(),
            context: Default::default(),
        };

        let response = handle(
            lambda_event,
            &service,
            &query_service,
            Some(&embedding_service),
            None,
            &personalization_service,
        )
        .await
        .unwrap();

        assert_eq!(200, response.status_code);
    }
}
