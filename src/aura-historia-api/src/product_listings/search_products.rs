use crate::auth::{OptionalAuthExtractor, request_metadata};
use crate::error::{ApiError, BAD_ORDER_VALUE, BAD_QUERY_PARAMETER_VALUE, BAD_SORT_VALUE};
use crate::state::ProductListingsState;

use application::operation_context::Principal;
use application::pagination::Cursor;
use aura_historia_contracts::product_listings::search::{
    self, CursoredProductListingsData, ProductListingSearchCursorInputData,
    ProductListingSearchData,
};
use axum::{
    Json,
    extract::{RawQuery, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use domain_primitives::sort::Sort;
use product_listing_core::{
    product_listing_search::ProductListingSearch,
    sort_product_listing_field::SortProductListingField,
};
use product_listing_service::use_cases::{
    ProductListingSearchCursor, SearchProductListingsRequest,
};
pub async fn get_products(
    State(state): State<ProductListingsState>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let data = match crate::wire::from_query_str(raw_query.as_deref().unwrap_or_default()) {
        Ok(data) => data,
        Err(error) => {
            return ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
                .with_detail(error.to_string())
                .into_response();
        }
    };
    handle_search(state, headers, raw_query.as_deref(), data).await
}

async fn handle_search(
    state: ProductListingsState,
    headers: HeaderMap,
    raw_query: Option<&str>,
    data: ProductListingSearchData,
) -> Response {
    let metadata = request_metadata(&headers);
    let principal = match OptionalAuthExtractor::new(state.authenticator.as_ref())
        .extract(&headers, &metadata)
        .await
    {
        Ok(principal) => principal,
        Err(error) => return ApiError::from(error).into_response(),
    };
    let sort = match parse_sort(raw_query) {
        Ok(sort) => sort,
        Err(error) => return error.into_response(),
    };
    let cursor = match parse_cursor(raw_query) {
        Ok(cursor) => cursor,
        Err(error) => return error.into_response(),
    };

    let search = match ProductListingSearch::try_from(data) {
        Ok(search) => search,
        Err(error) => return ApiError::from(error).into_response(),
    };
    let geographic = search.has_geography();
    let context = principal.operation_context(metadata);
    match state
        .search_products
        .execute(
            &context,
            SearchProductListingsRequest {
                search,
                sort,
                cursor,
            },
        )
        .await
    {
        Ok(result) => {
            let ttl = crate::shipping::listing::cache_ttl(
                result.items.iter().map(|i| i.item.shipping.as_ref()),
                60,
            );
            let response = Json(CursoredProductListingsData::from(result)).into_response();
            crate::transport::cache::anonymous_shared_success(
                response,
                &headers,
                !geographic && matches!(&context.principal, Principal::Anonymous),
                ttl,
            )
        }
        Err(error) => ApiError::from(error).into_response(),
    }
}

fn parse_sort(raw_query: Option<&str>) -> Result<Option<Sort<SortProductListingField>>, ApiError> {
    search::parse_sort(
        query_value(raw_query, "sort").as_deref(),
        query_value(raw_query, "order").as_deref(),
    )
    .map_err(Into::into)
}

fn parse_cursor(
    raw_query: Option<&str>,
) -> Result<Option<Cursor<ProductListingSearchCursor>>, ApiError> {
    let size = query_value(raw_query, "size")
        .map(|value| value.parse::<u64>())
        .transpose()
        .map_err(|error| {
            ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
                .with_query_field("size")
                .with_detail(error.to_string())
        })?;
    let values = query_values(raw_query, "searchAfter");
    let search_after = match values.as_slice() {
        [] => None,
        [value] => Some(parse_search_after(value)?),
        _ => {
            return Err(ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
                .with_query_field("searchAfter")
                .with_detail("ProductListing search cursor must be supplied once."));
        }
    };

    search::parse_cursor(size, search_after).map_err(Into::into)
}

fn parse_search_after(value: &str) -> Result<ProductListingSearchCursorInputData, ApiError> {
    serde_json::from_str(value).map_err(|error| {
        ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
            .with_query_field("searchAfter")
            .with_detail(error.to_string())
    })
}

fn query_value(raw_query: Option<&str>, key: &str) -> Option<String> {
    query_values(raw_query, key).into_iter().next()
}

fn query_values(raw_query: Option<&str>, key: &str) -> Vec<String> {
    raw_query
        .map(|query| {
            url::form_urlencoded::parse(query.as_bytes())
                .filter(|(name, _)| name == key)
                .map(|(_, value)| value.into_owned())
                .collect()
        })
        .unwrap_or_default()
}

impl From<search::ListingInputError> for ApiError {
    fn from(error: search::ListingInputError) -> Self {
        let code = match error.kind {
            search::ListingInputErrorKind::Value => BAD_QUERY_PARAMETER_VALUE,
            search::ListingInputErrorKind::Identifier => crate::error::INVALID_OBJECT_ID,
            search::ListingInputErrorKind::Sort => BAD_SORT_VALUE,
            search::ListingInputErrorKind::Order => BAD_ORDER_VALUE,
        };
        Self::bad_request(code)
            .with_query_field(error.field)
            .with_detail(error.detail)
    }
}

#[cfg(test)]
mod tests {
    use auction_core::AuctionId;
    use fxrate_core::FxRateId;
    use listing_source_core::ListingSourceId;
    use product_listing_core::{
        listing_availability::ListingAvailability, listing_orderability::ListingOrderability,
        product_listing_id::ProductListingId, product_listing_search::ListingAvailabilityQuery,
    };
    use std::collections::HashSet;
    use time::OffsetDateTime;

    use super::*;
    use crate::auth::{AuthError, RequestMetadata, TokenAuthenticator, TransportPrincipal};
    use application::operation_context::OperationContext;
    use application::pagination::Cursor;

    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use domain_primitives::sort::SortOrder;

    use localization::Language;
    use money::Currency;

    use product_listing_service::use_cases::{
        GetProductListingError, GetProductListingRequest, GetProductListingUseCase,
        GetSimilarProductListingsError, GetSimilarProductListingsRequest,
        GetSimilarProductListingsResult, GetSimilarProductListingsUseCase,
        SearchProductListingsError, SearchProductListingsResult, SearchProductListingsUseCase,
    };
    use serde_json::Value;
    use std::sync::{Arc, Mutex, MutexGuard};
    use tower::ServiceExt;

    type SearchProductListingsCalls =
        Arc<Mutex<Vec<(OperationContext, SearchProductListingsRequest)>>>;

    struct UnusedGetProductListingUseCase;

    #[async_trait::async_trait]
    impl GetProductListingUseCase for UnusedGetProductListingUseCase {
        async fn execute(
            &self,
            _context: &OperationContext,
            _request: GetProductListingRequest,
        ) -> Result<
            product_listing_service::use_cases::PersonalizedProductListingDetailsView,
            GetProductListingError,
        > {
            Err(GetProductListingError::NotFound)
        }
    }

    struct UnusedSimilarProductListingsUseCase;

    #[async_trait::async_trait]
    impl GetSimilarProductListingsUseCase for UnusedSimilarProductListingsUseCase {
        async fn execute(
            &self,
            _context: &OperationContext,
            _request: GetSimilarProductListingsRequest,
        ) -> Result<GetSimilarProductListingsResult, GetSimilarProductListingsError> {
            Err(
                GetSimilarProductListingsError::SimilaritySearchUnavailable {
                    source: application::error::static_error(
                        "similarity search fixture unavailable",
                    ),
                },
            )
        }
    }

    #[derive(Clone)]
    struct FakeSearchProductListingsUseCase {
        calls: SearchProductListingsCalls,
    }

    #[async_trait::async_trait]
    impl SearchProductListingsUseCase for FakeSearchProductListingsUseCase {
        async fn execute(
            &self,
            context: &OperationContext,
            request: SearchProductListingsRequest,
        ) -> Result<SearchProductListingsResult, SearchProductListingsError> {
            lock(&self.calls).push((context.clone(), request));
            Ok(SearchProductListingsResult::default())
        }
    }

    struct AnonymousAuthenticator;

    #[async_trait::async_trait]
    impl TokenAuthenticator for AnonymousAuthenticator {
        async fn authenticate(
            &self,
            _bearer_token: &str,
            _metadata: &RequestMetadata,
        ) -> Result<TransportPrincipal, AuthError> {
            Ok(TransportPrincipal::Anonymous)
        }
    }

    #[tokio::test]
    async fn should_preserve_every_filter_at_the_shared_application_boundary()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture =
            include_str!("../../../aura-historia-contracts/tests/fixtures/listing-search.json");
        let options = serde_json::from_str::<search::ProductListingSearchOptionsData>(fixture)?;
        let expected = options.into_request(serde_json::from_str(fixture)?)?;
        let query =
            include_str!("../../../aura-historia-contracts/tests/fixtures/listing-search.query")
                .trim();
        let (app, calls) = app();
        let response = app
            .oneshot(Request::get(format!("/v1/product-listings?{query}")).body(Body::empty())?)
            .await?;
        assert_eq!(StatusCode::OK, response.status());
        let calls = lock(&calls);
        assert_eq!(1, calls.len());
        assert_eq!(expected, calls[0].1);
        assert!(matches!(calls[0].0.principal, Principal::Anonymous));
        Ok(())
    }

    #[tokio::test]
    async fn should_map_get_search_request_and_add_public_cache_header()
    -> Result<(), Box<dyn std::error::Error>> {
        let (app, calls) = app();
        let fx_rate_id = FxRateId::new();
        let auction_id = AuctionId::new();
        let product_listing_id = ProductListingId::new();
        let cursor = serde_json::json!({
            "fxRateId": fx_rate_id,
            "searchAfter": [1234, product_listing_id]
        })
        .to_string();
        let encoded_cursor: String =
            url::form_urlencoded::byte_serialize(cursor.as_bytes()).collect();

        let response = app
            .oneshot(
                Request::get(format!(
                    "/v1/product-listings?language=de&currency=USD&productQuery=cabinet&auctionId={auction_id}&sort=updated&order=desc&size=200&searchAfter={encoded_cursor}"
                ))
                .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(
            "public, max-age=0, s-maxage=60, stale-if-error=0",
            response.headers()[header::CACHE_CONTROL]
        );
        let calls = lock(&calls);
        assert_eq!(1, calls.len());
        let request = &calls[0].1;
        assert_eq!(Language::De, request.search.language);
        assert_eq!(Currency::Usd, request.search.currency);
        assert_eq!("cabinet", request.search.product_listing_query[0].as_ref());
        assert!(request.search.auction_id_query.contains(&auction_id));
        assert!(matches!(
            request.sort,
            Some(Sort {
                sort: SortProductListingField::Updated,
                order: SortOrder::Desc,
            })
        ));
        assert_eq!(
            Some(Cursor {
                size: 100,
                search_after: Some(ProductListingSearchCursor {
                    fx_rate_id,
                    search_after: serde_json::json!([1234, product_listing_id]),
                }),
            }),
            request.cursor
        );
        Ok(())
    }

    #[tokio::test]
    async fn anonymous_geographic_searches_keep_origins_and_eligibility_results_out_of_shared_cache()
    -> Result<(), Box<dyn std::error::Error>> {
        for query in [
            "geography[countries][0]=DE&geography[subdivisions][0]=DE-BY",
            "geography[origin][latitude]=48.137&geography[origin][longitude]=11.575&geography[radius][amount]=25&geography[radius][unit]=KILOMETERS",
        ] {
            let (app, calls) = app();
            let response = app
                .oneshot(Request::get(format!("/v1/product-listings?{query}")).body(Body::empty())?)
                .await?;
            assert_eq!(StatusCode::OK, response.status());
            assert_eq!(
                "private, no-store",
                response.headers()[header::CACHE_CONTROL]
            );
            let calls = lock(&calls);
            assert_eq!(1, calls.len());
            assert!(calls[0].1.search.has_geography());
            assert!(matches!(
                calls[0].0.principal,
                application::operation_context::Principal::Anonymous
            ));
        }
        Ok(())
    }

    #[tokio::test]
    async fn percent_encoded_deep_object_keys_enforce_the_same_filters_as_literal_brackets()
    -> Result<(), Box<dyn std::error::Error>> {
        let pairs = [
            ("size", "100"),
            ("geography[countries][0]", "DE"),
            ("shipping[destination][kind]", "COUNTRY"),
            ("shipping[destination][country]", "DE"),
        ];
        let literal = pairs
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join("&");
        let encoded = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(pairs)
            .finish();
        assert!(encoded.contains("geography%5Bcountries%5D%5B0%5D=DE"));

        let mut searches = Vec::new();
        for query in [literal, encoded] {
            let (app, calls) = app();
            let response = app
                .oneshot(Request::get(format!("/v1/product-listings?{query}")).body(Body::empty())?)
                .await?;
            assert_eq!(StatusCode::OK, response.status(), "{query}");
            assert_eq!(
                "private, no-store",
                response.headers()[header::CACHE_CONTROL]
            );
            let calls = lock(&calls);
            assert_eq!(1, calls.len());
            let request = &calls[0].1;
            assert!(request.search.has_geography(), "{query}");
            assert!(request.search.shipping.is_some(), "{query}");
            assert_eq!(Some(100), request.cursor.as_ref().map(|cursor| cursor.size));
            searches.push(request.search.clone());
        }
        assert_eq!(searches[0], searches[1]);
        Ok(())
    }

    #[tokio::test]
    async fn should_disable_shared_cache_for_any_authorization_header()
    -> Result<(), Box<dyn std::error::Error>> {
        let (app, _) = app();
        let response = app
            .oneshot(
                Request::get("/v1/product-listings")
                    .header(header::AUTHORIZATION, "Bearer accepted-as-anonymous")
                    .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(
            "private, no-store",
            response.headers()[header::CACHE_CONTROL]
        );
        Ok(())
    }

    #[test]
    fn should_reject_noncanonical_query_object_ids() -> Result<(), Box<dyn std::error::Error>> {
        let listing_source_id = ListingSourceId::new();
        for invalid in [
            ProductListingId::new().to_string(),
            listing_source_id.as_uuid().to_string(),
            "ls_not-a-typeid".to_owned(),
        ] {
            let Err(error) =
                ProductListingSearch::try_from(serde_json::from_value::<ProductListingSearchData>(
                    serde_json::json!({"listingSourceId":[invalid]}),
                )?)
            else {
                return Err("noncanonical ListingSource ID was accepted".into());
            };
            assert_eq!(
                crate::error::INVALID_OBJECT_ID,
                ApiError::from(error).code()
            );
        }

        let auction_id = AuctionId::new();
        for invalid in [
            ProductListingId::new().to_string(),
            auction_id.as_uuid().to_string(),
            "auc_not-a-typeid".to_owned(),
        ] {
            let Err(error) =
                ProductListingSearch::try_from(serde_json::from_value::<ProductListingSearchData>(
                    serde_json::json!({"auctionId":[invalid]}),
                )?)
            else {
                return Err("noncanonical Auction ID was accepted".into());
            };
            assert_eq!(
                crate::error::INVALID_OBJECT_ID,
                ApiError::from(error).code()
            );
        }

        let fx_rate_id = FxRateId::new();
        for invalid in [
            ProductListingId::new().to_string(),
            fx_rate_id.as_uuid().to_string(),
            "fx_not-a-typeid".to_owned(),
        ] {
            let raw = serde_json::json!({
                "fxRateId": invalid,
                "searchAfter": ["next"]
            })
            .to_string();
            let query = url::form_urlencoded::Serializer::new(String::new())
                .append_pair("searchAfter", &raw)
                .finish();
            let Err(error) = parse_cursor(Some(&query)) else {
                return Err("noncanonical FxRate ID was accepted".into());
            };
            assert_eq!(crate::error::INVALID_OBJECT_ID, error.code());
        }
        Ok(())
    }

    #[tokio::test]
    async fn should_map_exact_orderability_and_unspecified_availability_query()
    -> Result<(), Box<dyn std::error::Error>> {
        let (app, calls) = app();

        let response = app
            .oneshot(
                Request::get(
                    "/v1/product-listings?availability=IN_STOCK&orderability=ORDERABLE_NOW&includeUnspecifiedAvailability=true",
                )
                .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::OK, response.status());
        let calls = lock(&calls);
        assert_eq!(
            Some(&ListingAvailabilityQuery {
                any_of: HashSet::from([ListingAvailability::InStock]).into(),
                orderability: HashSet::from([ListingOrderability::OrderableNow]).into(),
                include_unspecified: true,
            }),
            calls[0].1.search.availability_query.as_ref()
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_map_excluded_product_listing_ids() -> Result<(), Box<dyn std::error::Error>> {
        let (app, calls) = app();
        let first = ProductListingId::new();
        let second = ProductListingId::new();

        let response = app
            .oneshot(
                Request::get(format!(
                    "/v1/product-listings?excludeProductListingId={first}&excludeProductListingId={second}"
                ))
                .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::OK, response.status());
        let calls = lock(&calls);
        let excluded = &calls[0].1.search.exclude_product_listing_id_query;
        assert_eq!(2, excluded.len());
        assert!(excluded.contains(&first));
        assert!(excluded.contains(&second));
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_invalid_excluded_product_listing_id()
    -> Result<(), Box<dyn std::error::Error>> {
        let (app, calls) = app();

        let response = app
            .oneshot(
                Request::get(format!(
                    "/v1/product-listings?excludeProductListingId={}",
                    ListingSourceId::new()
                ))
                .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::BAD_REQUEST, response.status());
        assert!(lock(&calls).is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_price_sort() -> Result<(), Box<dyn std::error::Error>> {
        let (app, calls) = app();

        let response = app
            .oneshot(Request::get("/v1/product-listings?sort=price&order=asc").body(Body::empty())?)
            .await?;

        assert_eq!(StatusCode::BAD_REQUEST, response.status());
        assert!(lock(&calls).is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn should_default_sort_order_to_desc_when_only_sort_is_supplied()
    -> Result<(), Box<dyn std::error::Error>> {
        for (query, field) in [
            ("sort=score", SortProductListingField::Score),
            ("sort=updated", SortProductListingField::Updated),
            ("sort=created", SortProductListingField::Created),
        ] {
            let (app, calls) = app();

            let response = app
                .oneshot(Request::get(format!("/v1/product-listings?{query}")).body(Body::empty())?)
                .await?;

            assert_eq!(StatusCode::OK, response.status(), "{query}");
            assert_eq!(
                Some(Sort {
                    sort: field,
                    order: SortOrder::Desc,
                }),
                lock(&calls)[0].1.sort,
                "{query}"
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn should_apply_explicit_ascending_order() -> Result<(), Box<dyn std::error::Error>> {
        let (app, calls) = app();

        let response = app
            .oneshot(
                Request::get("/v1/product-listings?sort=created&order=asc").body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(
            Some(Sort {
                sort: SortProductListingField::Created,
                order: SortOrder::Asc,
            }),
            lock(&calls)[0].1.sort
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_invalid_or_incomplete_sort_parameters()
    -> Result<(), Box<dyn std::error::Error>> {
        for (query, error, field) in [
            ("sort=not-valid", "BAD_SORT_VALUE", "sort"),
            ("sort=not-valid&order=asc", "BAD_SORT_VALUE", "sort"),
            ("order=not-valid", "BAD_ORDER_VALUE", "order"),
            ("sort=updated&order=not-valid", "BAD_ORDER_VALUE", "order"),
            ("order=desc", "BAD_SORT_VALUE", "sort"),
        ] {
            let (app, calls) = app();

            let response = app
                .oneshot(Request::get(format!("/v1/product-listings?{query}")).body(Body::empty())?)
                .await?;

            assert_eq!(StatusCode::BAD_REQUEST, response.status(), "{query}");
            let body = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
            let body = serde_json::from_slice::<Value>(&body)?;
            assert_eq!(error, body["error"], "{query}");
            assert_eq!(field, body["source"]["field"], "{query}");
            assert!(lock(&calls).is_empty(), "{query}");
        }
        Ok(())
    }

    mod cursor_validation {
        use super::*;
        use crate::state::AppState;

        use embedding::{
            EmbeddingError, EmbeddingGenerator, EmbeddingImageUrl, EmbeddingText, EmbeddingVector,
        };

        use fxrate_core::FxRateSnapshot;
        use fxrate_service::ports::{FxRateSnapshotReadError, FxRateSnapshotReader};

        use opensearch::{
            OpenSearch,
            http::transport::{SingleNodeConnectionPool, TransportBuilder},
        };

        use product_listing_opensearch::OpenSearchProductListingSearchReader;
        use product_listing_service::ports::{
            ListingSourceSummaryReadError, ListingSourceSummaryReader,
            ListingSourceSummaryWithReferral, ProductListingContentAssessment,
            ProductListingContentAssessmentReadError, ProductListingContentAssessmentReader,
            ProductListingUserStateLookup, ProductListingUserStateReadError,
            ProductListingUserStateReader,
        };
        use product_listing_service::shipping::ShippingEnrichment;
        use product_listing_service::use_cases::SearchProductListingsHandler;
        use product_listing_service::user_state::ProductListingUserState;
        use serde_json::json;
        use shipping_service::assessment::{
            AssessDeliveryRequest, AssessDeliveryUseCase, AssessmentError, DeliveryAssessmentView,
        };
        use std::collections::HashMap;
        use std::io::ErrorKind;
        use std::net::TcpListener;
        use std::time::Duration;

        struct UnusedSearchDependencies;

        #[async_trait::async_trait]
        impl FxRateSnapshotReader for UnusedSearchDependencies {
            async fn find_by_id(
                &self,
                _id: FxRateId,
            ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotReadError> {
                panic!("invalid cursor must be rejected before pinned FX lookup")
            }

            async fn find_latest_at_or_before(
                &self,
                _at: OffsetDateTime,
            ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotReadError> {
                panic!("invalid cursor must be rejected before latest FX lookup")
            }

            async fn find_latest_at_or_before_many(
                &self,
                _timestamps: &[OffsetDateTime],
            ) -> Result<Vec<FxRateSnapshot>, FxRateSnapshotReadError> {
                panic!("invalid cursor must be rejected before batch FX lookup")
            }
        }

        #[async_trait::async_trait]
        impl EmbeddingGenerator for UnusedSearchDependencies {
            async fn embed_product(
                &self,
                _title: &EmbeddingText,
                _additional_text: Option<&EmbeddingText>,
                _image_url: Option<&EmbeddingImageUrl>,
            ) -> Result<EmbeddingVector, EmbeddingError> {
                panic!("product search must not generate product embeddings")
            }

            async fn embed_search_query(
                &self,
                _query: &EmbeddingText,
            ) -> Result<EmbeddingVector, EmbeddingError> {
                panic!("invalid cursor must be rejected before query embedding")
            }
        }

        #[async_trait::async_trait]
        impl ListingSourceSummaryReader for UnusedSearchDependencies {
            async fn find_summaries(
                &self,
                _listing_source_ids: &[ListingSourceId],
            ) -> Result<
                HashMap<ListingSourceId, ListingSourceSummaryWithReferral>,
                ListingSourceSummaryReadError,
            > {
                panic!("invalid cursor must be rejected before source enrichment")
            }
        }

        #[async_trait::async_trait]
        impl ProductListingUserStateReader for UnusedSearchDependencies {
            async fn find_for_user(
                &self,
                _lookup: &ProductListingUserStateLookup,
            ) -> Result<
                HashMap<ProductListingId, ProductListingUserState>,
                ProductListingUserStateReadError,
            > {
                panic!("invalid cursor must be rejected before user state enrichment")
            }
        }

        #[async_trait::async_trait]
        impl ProductListingContentAssessmentReader for UnusedSearchDependencies {
            async fn find_current_assessments(
                &self,
                _product_listing_ids: &[ProductListingId],
            ) -> Result<
                HashMap<ProductListingId, ProductListingContentAssessment>,
                ProductListingContentAssessmentReadError,
            > {
                panic!("invalid cursor must be rejected before content assessment")
            }
        }

        #[async_trait::async_trait]
        impl AssessDeliveryUseCase for UnusedSearchDependencies {
            async fn execute(
                &self,
                _context: &OperationContext,
                _request: AssessDeliveryRequest,
            ) -> Result<DeliveryAssessmentView, AssessmentError> {
                panic!("invalid cursor must be rejected before shipping enrichment")
            }
        }

        #[tokio::test]
        async fn should_reject_malformed_inner_cursors_before_dependency_io()
        -> Result<(), Box<dyn std::error::Error>> {
            let listener = TcpListener::bind("127.0.0.1:0")?;
            listener.set_nonblocking(true)?;
            let endpoint = url::Url::parse(&format!("http://{}", listener.local_addr()?))?;
            let transport =
                TransportBuilder::new(SingleNodeConnectionPool::new(endpoint)).build()?;
            let handler = SearchProductListingsHandler::new(
                OpenSearchProductListingSearchReader::new(OpenSearch::new(transport)),
                UnusedSearchDependencies,
                UnusedSearchDependencies,
                UnusedSearchDependencies,
                UnusedSearchDependencies,
                UnusedSearchDependencies,
                ShippingEnrichment::new(
                    Arc::new(UnusedSearchDependencies),
                    Arc::new(UnusedSearchDependencies),
                ),
            );
            let state = ProductListingsState::new(
                Arc::new(UnusedGetProductListingUseCase),
                Arc::new(UnusedSimilarProductListingsUseCase),
                Arc::new(handler),
                Arc::new(AnonymousAuthenticator),
            );
            let app = crate::app(AppState::new().with_products(state));
            let fx_rate_id = FxRateId::new();
            let product_listing_id = ProductListingId::new();

            for sort in ["", "&sort=score", "&sort=updated", "&sort=created"] {
                let mut invalid_values = vec![
                    json!({"unexpected": "cursor"}),
                    Value::Null,
                    json!("next"),
                    json!([]),
                    json!(["next"]),
                    json!([1234, product_listing_id, "extra"]),
                    json!([{}, product_listing_id]),
                    json!([1234, {}]),
                ];
                if matches!(sort, "&sort=updated" | "&sort=created") {
                    invalid_values.extend([json!([1234]), json!(1234)]);
                }
                for search_after in invalid_values {
                    let cursor = json!({
                        "fxRateId": fx_rate_id,
                        "searchAfter": search_after,
                    })
                    .to_string();
                    let query = url::form_urlencoded::Serializer::new(String::new())
                        .append_pair("searchAfter", &cursor)
                        .finish();
                    let response = tokio::time::timeout(
                        Duration::from_secs(5),
                        app.clone().oneshot(
                            Request::get(format!(
                                "/v1/product-listings?productQuery=cabinet{sort}&{query}"
                            ))
                            .body(Body::empty())?,
                        ),
                    )
                    .await??;

                    assert_eq!(
                        StatusCode::BAD_REQUEST,
                        response.status(),
                        "{sort}: {cursor}"
                    );
                    assert_eq!(
                        "application/problem+json",
                        response.headers()[header::CONTENT_TYPE],
                        "{sort}: {cursor}"
                    );
                    assert_eq!(
                        "private, no-store",
                        response.headers()[header::CACHE_CONTROL],
                        "{sort}: {cursor}"
                    );
                    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
                    let body = serde_json::from_slice::<Value>(&body)?;
                    assert_eq!(400, body["status"], "{sort}: {cursor}");
                    assert_eq!(
                        BAD_QUERY_PARAMETER_VALUE.to_string(),
                        body["error"],
                        "{sort}: {cursor}"
                    );
                    assert_eq!(
                        json!({"field": "searchAfter", "type": "QUERY"}),
                        body["source"],
                        "{sort}: {cursor}"
                    );
                    assert_eq!(
                        "searchAfter contains invalid sort values or an incorrect number of sort values.",
                        body["detail"],
                        "{sort}: {cursor}"
                    );
                    assert!(
                        matches!(listener.accept(), Err(error) if error.kind() == ErrorKind::WouldBlock),
                        "invalid cursor reached OpenSearch: {sort}: {cursor}"
                    );
                }
            }
            Ok(())
        }
    }

    fn app() -> (Router, SearchProductListingsCalls) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let state = ProductListingsState::new(
            Arc::new(UnusedGetProductListingUseCase),
            Arc::new(UnusedSimilarProductListingsUseCase),
            Arc::new(FakeSearchProductListingsUseCase {
                calls: Arc::clone(&calls),
            }),
            Arc::new(AnonymousAuthenticator),
        );
        (
            Router::new()
                .route("/v1/product-listings", axum::routing::get(get_products))
                .with_state(state),
            calls,
        )
    }

    fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        match mutex.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}
