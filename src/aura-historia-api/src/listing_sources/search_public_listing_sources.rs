use super::types::PublicListingSourceSearchCollectionData;
use crate::auth::{OptionalAuthExtractor, request_metadata};
use crate::error::{ApiError, BAD_QUERY_PARAMETER_VALUE, LISTING_SOURCE_INTERNAL_ERROR};
use crate::state::ListingSourcesState;
use crate::wire::parse_query_object_id;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Response};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use listing_source_core::ListingSourceId;
use listing_source_service::use_cases::queries::search_public_listing_sources::{
    DEFAULT_PUBLIC_LISTING_SOURCE_SEARCH_PAGE_SIZE, PublicListingSourceSearchContinuation,
    PublicListingSourceSearchPosition, PublicListingSourceSearchQuery,
    SearchPublicListingSourcesRequest,
};
use serde::{Deserialize, Serialize};

const MAX_RAW_QUERY_BYTES: usize = 8 * 1024;
const MAX_CURSOR_ENCODED_BYTES: usize = 4 * 1024;
const MAX_CURSOR_DECODED_BYTES: usize = 3 * 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PublicListingSourceSearchCursorData {
    binding: String,
    match_tier: u8,
    name_search: String,
    listing_source_id: String,
}

pub async fn search_public_listing_sources(
    State(state): State<ListingSourcesState>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let request = match parse_request(raw_query.as_deref()) {
        Ok(request) => request,
        Err(error) => return no_store(error.into_response()),
    };
    let Some(budget) = state.public_read_budget.as_ref() else {
        return no_store(
            ApiError::internal_server_error(LISTING_SOURCE_INTERNAL_ERROR)
                .with_detail("Public listing source search is not configured.")
                .into_response(),
        );
    };
    let Some(_permit) = budget.try_acquire() else {
        return overloaded_response();
    };
    let operation = public_search_operation(&request);
    let first_page = request.continuation().is_none();
    let started = std::time::Instant::now();
    let metadata = request_metadata(&headers);
    let principal = match tokio::time::timeout(
        budget.request_timeout(),
        OptionalAuthExtractor::new(state.authenticator.as_ref()).extract(&headers, &metadata),
    )
    .await
    {
        Ok(Ok(principal)) => principal,
        Ok(Err(error)) => return no_store(ApiError::from(error).into_response()),
        Err(_) => {
            return no_store(
                ApiError::service_unavailable(crate::error::LISTING_SOURCE_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Public listing source search timed out.")
                    .into_response(),
            );
        }
    };
    let Some(search_public) = state.search_public.as_ref() else {
        return no_store(
            ApiError::internal_server_error(LISTING_SOURCE_INTERNAL_ERROR)
                .with_detail("Public listing source search is not configured.")
                .into_response(),
        );
    };

    let remaining = budget.request_timeout().saturating_sub(started.elapsed());
    match tokio::time::timeout(
        remaining,
        search_public.execute(&principal.operation_context(metadata), request),
    )
    .await
    {
        Err(_) => no_store(
            ApiError::service_unavailable(crate::error::LISTING_SOURCE_TEMPORARILY_UNAVAILABLE)
                .with_detail("Public listing source search timed out.")
                .into_response(),
        ),
        Ok(result) => match result {
            Ok(result) => {
                tracing::info!(
                    endpoint = "public_listing_source_search",
                    operation,
                    first_page,
                    page_size = result.page_size,
                    returned_items = result.items.len(),
                    has_continuation = result.continuation.is_some(),
                    elapsed_ms = started.elapsed().as_millis(),
                    outcome = "success",
                    "completed bounded public ListingSource read"
                );
                let search_after = result
                    .continuation
                    .as_ref()
                    .map(encode_continuation)
                    .transpose();
                match search_after {
                    Ok(search_after) => crate::transport::cache::anonymous_shared_success(
                        axum::Json(PublicListingSourceSearchCollectionData::new(
                            result,
                            search_after,
                        ))
                        .into_response(),
                        &headers,
                        matches!(&principal, crate::auth::TransportPrincipal::Anonymous),
                        300,
                    ),
                    Err(error) => no_store(error.into_response()),
                }
            }
            Err(error) => no_store(ApiError::from(error).into_response()),
        },
    }
}

fn public_search_operation(request: &SearchPublicListingSourcesRequest) -> &'static str {
    if request.query().is_browse() {
        "browse"
    } else if request.query().is_insufficient_input() {
        "insufficient-input"
    } else {
        "text"
    }
}

fn parse_request(raw_query: Option<&str>) -> Result<SearchPublicListingSourcesRequest, ApiError> {
    let raw_query = raw_query.unwrap_or_default();
    if raw_query.len() > MAX_RAW_QUERY_BYTES {
        return Err(bad_query("query", "Query string exceeds 8 KiB."));
    }

    let mut query = None;
    let mut size = None;
    let mut search_after = None;
    for (field, value) in url::form_urlencoded::parse(raw_query.as_bytes()) {
        let (field, target): (&'static str, &mut Option<String>) = match field.as_ref() {
            "query" => ("query", &mut query),
            "size" => ("size", &mut size),
            "searchAfter" => ("searchAfter", &mut search_after),
            _ => return Err(bad_query("query", "Unknown query parameter.")),
        };
        if target.replace(value.into_owned()).is_some() {
            return Err(bad_query(field, "Repeated query parameter."));
        }
    }

    let page_size = parse_page_size(size.as_deref())?;
    let query = PublicListingSourceSearchQuery::new(query)
        .map_err(|_| bad_query("query", "Query parameter is invalid."))?;
    let continuation = search_after
        .as_deref()
        .map(parse_continuation)
        .transpose()?;
    SearchPublicListingSourcesRequest::new(query, page_size, continuation).map_err(|_| {
        bad_query(
            "searchAfter",
            "Search continuation is invalid for this request.",
        )
    })
}

fn parse_page_size(value: Option<&str>) -> Result<u8, ApiError> {
    let Some(value) = value else {
        return Ok(DEFAULT_PUBLIC_LISTING_SOURCE_SEARCH_PAGE_SIZE);
    };
    let value = value
        .parse::<u16>()
        .map_err(|_| bad_query("size", "Size must be an integer from 1 through 50."))?;
    u8::try_from(value)
        .ok()
        .filter(|value| (1..=50).contains(value))
        .ok_or_else(|| bad_query("size", "Size must be an integer from 1 through 50."))
}

fn parse_continuation(value: &str) -> Result<PublicListingSourceSearchContinuation, ApiError> {
    if value.len() > MAX_CURSOR_ENCODED_BYTES {
        return Err(bad_query(
            "searchAfter",
            "Search continuation exceeds the size limit.",
        ));
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| bad_query("searchAfter", "Search continuation is invalid."))?;
    if bytes.len() > MAX_CURSOR_DECODED_BYTES {
        return Err(bad_query(
            "searchAfter",
            "Search continuation exceeds the size limit.",
        ));
    }
    let data: PublicListingSourceSearchCursorData = serde_json::from_slice(&bytes)
        .map_err(|_| bad_query("searchAfter", "Search continuation is invalid."))?;
    let binding = URL_SAFE_NO_PAD
        .decode(data.binding)
        .map_err(|_| bad_query("searchAfter", "Search continuation is invalid."))?
        .try_into()
        .map_err(|_: Vec<u8>| bad_query("searchAfter", "Search continuation is invalid."))?;
    let listing_source_id: ListingSourceId =
        parse_query_object_id(&data.listing_source_id, "searchAfter", "ListingSource")?;
    let position = PublicListingSourceSearchPosition::new(
        data.match_tier,
        data.name_search,
        listing_source_id,
    )
    .map_err(|_| bad_query("searchAfter", "Search continuation is invalid."))?;
    Ok(PublicListingSourceSearchContinuation::new(
        binding, position,
    ))
}

fn encode_continuation(
    continuation: &PublicListingSourceSearchContinuation,
) -> Result<String, ApiError> {
    let position = continuation.position();
    let payload = PublicListingSourceSearchCursorData {
        binding: URL_SAFE_NO_PAD.encode(continuation.binding()),
        match_tier: position.match_tier(),
        name_search: position.name_search().to_owned(),
        listing_source_id: position.listing_source_id().to_string(),
    };
    let bytes = serde_json::to_vec(&payload).map_err(|_| {
        ApiError::internal_server_error(LISTING_SOURCE_INTERNAL_ERROR)
            .with_detail("Public listing source continuation failed internally.")
    })?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn bad_query(field: &'static str, detail: &str) -> ApiError {
    ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
        .with_query_field(field)
        .with_detail(detail)
}

fn overloaded_response() -> Response {
    let mut response =
        ApiError::service_unavailable(crate::error::LISTING_SOURCE_TEMPORARILY_UNAVAILABLE)
            .with_detail("Public listing source reads are at capacity.")
            .into_response();
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
    no_store(response)
}

fn no_store(response: Response) -> Response {
    tracing::info!(
        endpoint = "public_listing_source_search",
        status = response.status().as_u16(),
        "completed public ListingSource search response"
    );
    crate::transport::cache::private_no_store(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{
        AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal,
    };
    use crate::state::{ListingSourcesState, PublicListingSourceReadBudget};
    use application::operation_context::OperationContext;
    use axum::{
        Router,
        body::Body,
        http::{Request, StatusCode, header},
        routing::get,
    };
    use listing_source_core::ListingSourceName;
    use listing_source_service::use_cases::commands::{
        create_listing_source::{
            CreateListingSourceCommand, CreateListingSourceError, CreateListingSourceResult,
            CreateListingSourceUseCase,
        },
        update_listing_source::{
            UpdateListingSourceCommand, UpdateListingSourceError, UpdateListingSourceResult,
            UpdateListingSourceUseCase,
        },
    };
    use listing_source_service::use_cases::queries::{
        get_listing_source::{
            GetListingSourceError, GetListingSourceRequest, GetListingSourceResult,
            GetListingSourceUseCase,
        },
        get_public_listing_source_by_slug::{
            GetPublicListingSourceBySlugError, GetPublicListingSourceBySlugRequest,
            GetPublicListingSourceBySlugUseCase,
        },
        search_listing_sources::{
            SearchListingSourcesError, SearchListingSourcesRequest, SearchListingSourcesResult,
            SearchListingSourcesUseCase,
        },
        search_public_listing_sources::{
            SearchPublicListingSourcesError, SearchPublicListingSourcesResult,
            SearchPublicListingSourcesUseCase,
        },
    };
    use partnership_service::use_cases::queries::list_administered_listing_sources::{
        ListAdministeredListingSourcesError, ListAdministeredListingSourcesRequest,
        ListAdministeredListingSourcesResult, ListAdministeredListingSourcesUseCase,
    };
    use std::{sync::Arc, time::Duration};
    use tower::ServiceExt;
    use user_core::user_id::UserId;

    struct Unused;

    #[async_trait::async_trait]
    impl CreateListingSourceUseCase for Unused {
        async fn execute(
            &self,
            _: &OperationContext,
            _: CreateListingSourceCommand,
        ) -> Result<CreateListingSourceResult, CreateListingSourceError> {
            Err(CreateListingSourceError::Forbidden)
        }
    }

    #[async_trait::async_trait]
    impl GetListingSourceUseCase for Unused {
        async fn execute(
            &self,
            _: &OperationContext,
            _: GetListingSourceRequest,
        ) -> Result<GetListingSourceResult, GetListingSourceError> {
            Err(GetListingSourceError::Forbidden)
        }
    }

    #[async_trait::async_trait]
    impl UpdateListingSourceUseCase for Unused {
        async fn execute(
            &self,
            _: &OperationContext,
            _: UpdateListingSourceCommand,
        ) -> Result<UpdateListingSourceResult, UpdateListingSourceError> {
            Err(UpdateListingSourceError::Forbidden)
        }
    }

    #[async_trait::async_trait]
    impl ListAdministeredListingSourcesUseCase for Unused {
        async fn execute(
            &self,
            _: &OperationContext,
            _: ListAdministeredListingSourcesRequest,
        ) -> Result<ListAdministeredListingSourcesResult, ListAdministeredListingSourcesError>
        {
            Err(ListAdministeredListingSourcesError::Forbidden)
        }
    }

    #[async_trait::async_trait]
    impl SearchListingSourcesUseCase for Unused {
        async fn execute(
            &self,
            _: &OperationContext,
            _: SearchListingSourcesRequest,
        ) -> Result<SearchListingSourcesResult, SearchListingSourcesError> {
            Err(SearchListingSourcesError::Forbidden)
        }
    }

    struct FakePublicSearch;

    #[async_trait::async_trait]
    impl SearchPublicListingSourcesUseCase for FakePublicSearch {
        async fn execute(
            &self,
            _: &OperationContext,
            request: SearchPublicListingSourcesRequest,
        ) -> Result<SearchPublicListingSourcesResult, SearchPublicListingSourcesError> {
            Ok(SearchPublicListingSourcesResult {
                items: vec![],
                page_size: request.page_size(),
                continuation: None,
            })
        }
    }

    struct FakePublicBySlug;

    #[async_trait::async_trait]
    impl GetPublicListingSourceBySlugUseCase for FakePublicBySlug {
        async fn execute(
            &self,
            _: &OperationContext,
            request: GetPublicListingSourceBySlugRequest,
        ) -> Result<
            listing_source_service::use_cases::queries::get_public_listing_source_by_slug::GetPublicListingSourceBySlugResult,
            GetPublicListingSourceBySlugError,
        >{
            Ok(listing_source_service::use_cases::queries::public_listing_source::PublicListingSourceSummary {
                listing_source_id: ListingSourceId::new(),
                listing_source_slug_id: request.slug_id,
                name: ListingSourceName::try_from("Source")
                    .unwrap_or_else(|error| panic!("valid ListingSource name: {error}")),
                operator: listing_source_service::use_cases::queries::public_listing_source::PublicListingSourceOperatorSummary {
                    name: party_core::party_name::PartyName::try_from("Operator")
                        .unwrap_or_else(|error| panic!("valid operator name: {error}")),
                },
                url: None,
                image: None,
            })
        }
    }

    #[derive(Clone, Copy)]
    struct FakeAuthenticator {
        reject: bool,
    }

    #[async_trait::async_trait]
    impl TokenAuthenticator for FakeAuthenticator {
        async fn authenticate(
            &self,
            _: &str,
            _: &RequestMetadata,
        ) -> Result<TransportPrincipal, AuthError> {
            if self.reject {
                Err(AuthError::InvalidCredentials)
            } else {
                Ok(TransportPrincipal::User {
                    user_id: UserId::new(),
                    auth_method: AuthMethod::CognitoJwt,
                    capabilities: Default::default(),
                })
            }
        }
    }

    fn app() -> Router {
        app_with_auth(FakeAuthenticator { reject: true })
    }

    fn app_with_auth(authenticator: FakeAuthenticator) -> Router {
        let state = ListingSourcesState::new(
            Arc::new(Unused),
            Arc::new(Unused),
            Arc::new(Unused),
            Arc::new(Unused),
            Arc::new(Unused),
            Arc::new(authenticator),
        )
        .with_public_reads(
            Arc::new(FakePublicSearch),
            Arc::new(FakePublicBySlug),
            PublicListingSourceReadBudget::new(2, Duration::from_secs(1)),
        );
        Router::new()
            .route(
                "/api/v1/listing-sources",
                get(search_public_listing_sources),
            )
            .route(
                "/api/v1/listing-sources/by-slug/{listing_source_slug_id}",
                get(crate::listing_sources::get_listing_source_by_slug::get_listing_source_by_slug),
            )
            .with_state(state)
    }

    #[test]
    fn should_parse_public_query_with_default_size() -> Result<(), ApiError> {
        let request = parse_request(Some("query=mul"))?;
        assert_eq!(21, request.page_size());
        assert_eq!(Some("mul"), request.query().canonical_text());
        Ok(())
    }

    #[test]
    fn should_reject_unknown_repeated_and_out_of_range_query_parameters() {
        for query in ["sort=name", "query=mu&query=mul", "size=0", "size=51"] {
            assert!(parse_request(Some(query)).is_err(), "{query}");
        }
    }

    #[test]
    fn should_reject_oversized_or_malformed_continuations() {
        assert!(parse_request(Some("searchAfter=not-base64")).is_err());
        let oversized = "a".repeat(MAX_CURSOR_ENCODED_BYTES + 1);
        assert!(parse_request(Some(&format!("searchAfter={oversized}"))).is_err());
    }

    #[tokio::test]
    async fn should_cache_anonymous_listing_source_search_for_three_hundred_seconds()
    -> Result<(), Box<dyn std::error::Error>> {
        let response = app()
            .oneshot(Request::get("/api/v1/listing-sources?query=source").body(Body::empty())?)
            .await?;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(
            "public, max-age=0, s-maxage=300",
            response.headers()[header::CACHE_CONTROL]
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_keep_cors_headers_for_two_origins_on_the_same_cacheable_search()
    -> Result<(), Box<dyn std::error::Error>> {
        let app = crate::transport::with_transport_middleware(
            app(),
            crate::transport::NATIVE_REQUEST_TIMEOUT,
        );

        // These are distinct origins in the prod Gateway CORS allowlist. Axum's current
        // CORS layer emits `*`; the synthesized CloudFront Origin cache key partitions
        // origin-dependent front-door headers for the same path and query.
        for origin in ["https://aura-historia.com", "https://admin.shopify.com"] {
            let response = app
                .clone()
                .oneshot(
                    Request::get("/api/v1/listing-sources?query=source")
                        .header(header::ORIGIN, origin)
                        .body(Body::empty())?,
                )
                .await?;

            assert_eq!(StatusCode::OK, response.status());
            assert_eq!(
                "public, max-age=0, s-maxage=300",
                response.headers()[header::CACHE_CONTROL]
            );
            assert_eq!("*", response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN]);
            let vary = response.headers()[header::VARY].to_str()?;
            let vary_tokens = vary
                .split(',')
                .map(str::trim)
                .map(str::to_ascii_lowercase)
                .collect::<Vec<_>>();
            assert_eq!(
                1,
                vary_tokens
                    .iter()
                    .filter(|token| *token == "origin")
                    .count()
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn should_cache_anonymous_listing_source_slug_for_three_hundred_seconds()
    -> Result<(), Box<dyn std::error::Error>> {
        let response = app()
            .oneshot(Request::get("/api/v1/listing-sources/by-slug/source").body(Body::empty())?)
            .await?;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(
            "public, max-age=0, s-maxage=300",
            response.headers()[header::CACHE_CONTROL]
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_return_private_no_store_for_valid_credentials_on_search_and_detail()
    -> Result<(), Box<dyn std::error::Error>> {
        let authenticator = FakeAuthenticator { reject: false };

        let search_response = app_with_auth(authenticator)
            .oneshot(
                Request::get("/api/v1/listing-sources?query=source")
                    .header(header::AUTHORIZATION, "Bearer valid")
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(StatusCode::OK, search_response.status());
        assert_eq!(
            "private, no-store",
            search_response.headers()[header::CACHE_CONTROL]
        );

        let detail_response = app_with_auth(authenticator)
            .oneshot(
                Request::get("/api/v1/listing-sources/by-slug/source")
                    .header(header::AUTHORIZATION, "Bearer valid")
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(StatusCode::OK, detail_response.status());
        assert_eq!(
            "private, no-store",
            detail_response.headers()[header::CACHE_CONTROL]
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_invalid_optional_credentials_without_shared_cache()
    -> Result<(), Box<dyn std::error::Error>> {
        let response = app()
            .oneshot(
                Request::get("/api/v1/listing-sources?query=source")
                    .header(header::AUTHORIZATION, "Bearer invalid")
                    .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::UNAUTHORIZED, response.status());
        assert_eq!(
            "private, no-store",
            response.headers()[header::CACHE_CONTROL]
        );
        Ok(())
    }
}
