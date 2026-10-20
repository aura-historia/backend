use crate::{
    auth::{OptionalAuthExtractor, request_metadata},
    error::{
        AUCTION_INTERNAL_ERROR, AUCTION_NOT_FOUND, AUCTION_TEMPORARILY_UNAVAILABLE, ApiError,
        BAD_QUERY_PARAMETER_VALUE,
    },
    state::PublicAuctionsState,
    wire::parse_path_object_id,
};
mod data;
use auction_core::AuctionId;
use auction_service::use_cases::{GetPublicAuctionError, ListAuctionsError};
use axum::{
    Json,
    extract::{Path, RawQuery, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use data::*;

pub async fn get_public_auction(
    State(state): State<PublicAuctionsState>,
    headers: HeaderMap,
    Path(raw_auction_id): Path<String>,
) -> Response {
    let metadata = request_metadata(&headers);
    let principal = match OptionalAuthExtractor::new(state.authenticator.as_ref())
        .extract(&headers, &metadata)
        .await
    {
        Ok(principal) => principal,
        Err(error) => return no_store(ApiError::from(error).into_response()),
    };
    let auction_id =
        match parse_path_object_id::<AuctionId>(&raw_auction_id, "auctionId", "Auction") {
            Ok(value) => value,
            Err(error) => return no_store(error.into_response()),
        };
    match state
        .get
        .execute(&principal.operation_context(metadata), auction_id)
        .await
    {
        Ok(view) => match PublicAuctionData::try_from(view) {
            Ok(data) => crate::transport::cache::anonymous_shared_success(
                Json(data).into_response(),
                &headers,
                matches!(&principal, crate::auth::TransportPrincipal::Anonymous),
                60,
            ),
            Err(error) => no_store(error.into_response()),
        },
        Err(error) => no_store(ApiError::from(error).into_response()),
    }
}

pub async fn get_auction_catalogue(
    State(state): State<PublicAuctionsState>,
    headers: HeaderMap,
    Path(raw_auction_id): Path<String>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let auction_id =
        match parse_path_object_id::<AuctionId>(&raw_auction_id, "auctionId", "Auction") {
            Ok(value) => value,
            Err(error) => return no_store(error.into_response()),
        };
    let query = match crate::wire::from_query_str::<CatalogueQuery>(
        raw_query.as_deref().unwrap_or_default(),
    ) {
        Ok(value) => value,
        Err(error) => {
            return no_store(
                ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
                    .with_detail(error.to_string())
                    .into_response(),
            );
        }
    };
    let cursor = match query.search_after.map(parse_catalogue_cursor).transpose() {
        Ok(value) => application::pagination::Cursor {
            size: query.page_size.unwrap_or(21),
            search_after: value,
        },
        Err(error) => return no_store(error.into_response()),
    };
    let metadata = request_metadata(&headers);
    let principal = match OptionalAuthExtractor::new(state.authenticator.as_ref())
        .extract(&headers, &metadata)
        .await
    {
        Ok(value) => value,
        Err(error) => return no_store(ApiError::from(error).into_response()),
    };
    match state
        .catalogue
        .execute(
            &principal.operation_context(metadata),
            product_listing_service::use_cases::GetAuctionCatalogueRequest {
                auction_id,
                language: query.language,
                currency: query.currency,
                cursor: Some(cursor),
            },
        )
        .await
    {
        Ok(page) => {
            let ttl = crate::shipping::listing::cache_ttl(
                page.items.iter().map(|i| i.item.shipping.as_ref()),
                60,
            );
            crate::transport::cache::anonymous_shared_success(
                Json(CatalogueData::from(page)).into_response(),
                &headers,
                matches!(&principal, crate::auth::TransportPrincipal::Anonymous),
                ttl,
            )
        }
        Err(error) => no_store(ApiError::from(error).into_response()),
    }
}

pub async fn list_public_auctions(
    State(state): State<PublicAuctionsState>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let data = match crate::wire::from_query_str::<DirectoryQuery>(
        raw_query.as_deref().unwrap_or_default(),
    ) {
        Ok(value) => value,
        Err(error) => {
            return no_store(
                ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
                    .with_detail(error.to_string())
                    .into_response(),
            );
        }
    };
    let request = match data.try_into_request() {
        Ok(value) => value,
        Err(error) => return no_store(error.into_response()),
    };
    let metadata = request_metadata(&headers);
    let principal = match OptionalAuthExtractor::new(state.authenticator.as_ref())
        .extract(&headers, &metadata)
        .await
    {
        Ok(principal) => principal,
        Err(error) => return no_store(ApiError::from(error).into_response()),
    };
    match state
        .list
        .execute(&principal.operation_context(metadata), request)
        .await
    {
        Ok(result) => crate::transport::cache::anonymous_shared_success(
            Json(PublicAuctionDirectoryData::from(result)).into_response(),
            &headers,
            matches!(&principal, crate::auth::TransportPrincipal::Anonymous),
            60,
        ),
        Err(error) => no_store(ApiError::from(error).into_response()),
    }
}

impl From<GetPublicAuctionError> for ApiError {
    fn from(error: GetPublicAuctionError) -> Self {
        match error {
            GetPublicAuctionError::NotFound => ApiError::not_found(AUCTION_NOT_FOUND),
            GetPublicAuctionError::TemporarilyUnavailable { .. } => {
                ApiError::service_unavailable(AUCTION_TEMPORARILY_UNAVAILABLE)
            }
            GetPublicAuctionError::InvalidReadModel { .. } => {
                ApiError::internal_server_error(AUCTION_INTERNAL_ERROR)
            }
        }
    }
}
impl From<product_listing_service::use_cases::GetAuctionCatalogueError> for ApiError {
    fn from(error: product_listing_service::use_cases::GetAuctionCatalogueError) -> Self {
        match error {
            product_listing_service::use_cases::GetAuctionCatalogueError::Shipping(error) => error.into(),
            product_listing_service::use_cases::GetAuctionCatalogueError::AuctionNotFound => {
                ApiError::not_found(AUCTION_NOT_FOUND)
            }
            product_listing_service::use_cases::GetAuctionCatalogueError::CursorScopeMismatch => {
                ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
            }
            product_listing_service::use_cases::GetAuctionCatalogueError::TemporarilyUnavailable { .. }
            | product_listing_service::use_cases::GetAuctionCatalogueError::PricingFxSnapshotMissing => {
                ApiError::service_unavailable(AUCTION_TEMPORARILY_UNAVAILABLE)
            }
            product_listing_service::use_cases::GetAuctionCatalogueError::InvalidReadModel { .. }
            | product_listing_service::use_cases::GetAuctionCatalogueError::PricingPresentationFailed { .. } => {
                ApiError::internal_server_error(AUCTION_INTERNAL_ERROR)
            }
        }
    }
}
impl From<ListAuctionsError> for ApiError {
    fn from(error: ListAuctionsError) -> Self {
        match error {
            ListAuctionsError::IncompleteScheduleInstantRange
            | ListAuctionsError::ScheduledSortRoleMismatch
            | ListAuctionsError::InvalidScheduleInstantRange
            | ListAuctionsError::CursorScopeMismatch => {
                ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
            }
            ListAuctionsError::TemporarilyUnavailable { .. } => {
                ApiError::service_unavailable(AUCTION_TEMPORARILY_UNAVAILABLE)
            }
            ListAuctionsError::InvalidReadModel { .. } => {
                ApiError::internal_server_error(AUCTION_INTERNAL_ERROR)
            }
        }
    }
}
fn no_store(response: Response) -> Response {
    crate::transport::cache::private_no_store(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{
        AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal,
    };
    use application::{operation_context::OperationContext, pagination::CursoredResult};
    use auction_core::AuctionSchedulePoint;
    use auction_service::{
        ports::{PublicAuctionDetails, PublicAuctionSourceSummary},
        use_cases::{
            GetPublicAuctionResult, GetPublicAuctionUseCase, ListAuctionsRequest,
            ListAuctionsResult, ListAuctionsUseCase,
        },
    };
    use axum::{
        Router,
        body::Body,
        http::{Request, StatusCode, header},
        routing::get,
    };
    use domain_primitives::sort::SortOrder;
    use listing_source_core::ListingSourceId;
    use money::Currency;
    use std::sync::{Arc, Mutex, MutexGuard};
    use time::OffsetDateTime;
    use tower::ServiceExt;
    use user_core::user_id::UserId;

    #[derive(Clone)]
    struct FakeGet {
        result: GetPublicAuctionResult,
    }

    #[async_trait::async_trait]
    impl GetPublicAuctionUseCase for FakeGet {
        async fn execute(
            &self,
            _context: &OperationContext,
            _auction_id: AuctionId,
        ) -> Result<GetPublicAuctionResult, GetPublicAuctionError> {
            Ok(self.result.clone())
        }
    }

    #[derive(Clone, Copy)]
    struct FakeList;

    #[async_trait::async_trait]
    impl ListAuctionsUseCase for FakeList {
        async fn execute(
            &self,
            _context: &OperationContext,
            _request: ListAuctionsRequest,
        ) -> Result<ListAuctionsResult, ListAuctionsError> {
            Ok(CursoredResult::default())
        }
    }

    #[derive(Clone, Default)]
    struct FakeCatalogue {
        requests: Arc<Mutex<Vec<product_listing_service::use_cases::GetAuctionCatalogueRequest>>>,
    }

    #[async_trait::async_trait]
    impl product_listing_service::use_cases::GetAuctionCatalogueUseCase for FakeCatalogue {
        async fn execute(
            &self,
            _context: &OperationContext,
            request: product_listing_service::use_cases::GetAuctionCatalogueRequest,
        ) -> Result<
            CursoredResult<
                product_listing_service::use_cases::PersonalizedProductListingDetailsView,
                product_listing_service::ports::AuctionCatalogueCursor,
            >,
            product_listing_service::use_cases::GetAuctionCatalogueError,
        > {
            lock(&self.requests).push(request);
            Ok(CursoredResult::default())
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
            _bearer_token: &str,
            _metadata: &RequestMetadata,
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

    fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
        match value.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn detail() -> PublicAuctionDetails {
        PublicAuctionDetails {
            auction_id: AuctionId::new(),
            source: PublicAuctionSourceSummary {
                listing_source_id: ListingSourceId::new(),
                slug_id: listing_source_core::ListingSourceSlugId::raw("source")
                    .unwrap_or_else(|error| panic!("valid source slug: {error}")),
                name: listing_source_core::ListingSourceName::try_from("Source")
                    .unwrap_or_else(|error| panic!("valid source name: {error}")),
                referral_configuration: None,
            },
            name: None,
            catalogue_url: None,
            format: None,
            schedule: auction_core::AuctionSchedule::default(),
            reported_status: None,
            reported_lot_count: None,
            visible_active_assigned_listing_count: 0,
        }
    }

    fn app(catalogue: FakeCatalogue) -> Router {
        app_with_auth(catalogue, FakeAuthenticator { reject: true })
    }

    fn app_with_auth(catalogue: FakeCatalogue, authenticator: FakeAuthenticator) -> Router {
        Router::new()
            .route("/v1/auctions", get(list_public_auctions))
            .route("/v1/auctions/{auction_id}", get(get_public_auction))
            .route(
                "/v1/auctions/{auction_id}/product-listings",
                get(get_auction_catalogue),
            )
            .with_state(PublicAuctionsState::new(
                Arc::new(FakeGet { result: detail() }),
                Arc::new(FakeList),
                Arc::new(catalogue),
                Arc::new(authenticator),
            ))
    }

    #[tokio::test]
    async fn should_return_safe_public_auction_detail_with_shared_cache_for_anonymous_request()
    -> Result<(), Box<dyn std::error::Error>> {
        let auction_id = detail().auction_id;
        let response = app(FakeCatalogue::default())
            .oneshot(Request::get(format!("/v1/auctions/{auction_id}")).body(Body::empty())?)
            .await?;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(
            "public, max-age=0, s-maxage=60, stale-if-error=0",
            response.headers()[header::CACHE_CONTROL]
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
        let body: serde_json::Value = serde_json::from_slice(&body)?;
        assert!(body.get("sourceAuctionId").is_none());
        assert!(body.get("description").is_none());
        assert!(body.get("evidence").is_none());
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_invalid_public_auction_id_without_store()
    -> Result<(), Box<dyn std::error::Error>> {
        let response = app(FakeCatalogue::default())
            .oneshot(Request::get("/v1/auctions/not-an-auction").body(Body::empty())?)
            .await?;

        assert_eq!(StatusCode::BAD_REQUEST, response.status());
        assert_eq!(
            "private, no-store",
            response.headers()[header::CACHE_CONTROL]
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_map_catalogue_currency_and_page_size_for_anonymous_request()
    -> Result<(), Box<dyn std::error::Error>> {
        let catalogue = FakeCatalogue::default();
        let auction_id = detail().auction_id;
        let response = app(catalogue.clone())
            .oneshot(
                Request::get(format!(
                    "/v1/auctions/{auction_id}/product-listings?currency=USD&pageSize=25"
                ))
                .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(
            "public, max-age=0, s-maxage=60, stale-if-error=0",
            response.headers()[header::CACHE_CONTROL]
        );
        assert!(matches!(
            lock(&catalogue.requests).as_slice(),
            [request] if request.auction_id == auction_id
                && request.currency == Currency::Usd
                && request.cursor.as_ref().map(|cursor| cursor.size) == Some(25)
        ));
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_an_invalid_optional_public_credential()
    -> Result<(), Box<dyn std::error::Error>> {
        let response = app(FakeCatalogue::default())
            .oneshot(
                Request::get("/v1/auctions")
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

    #[tokio::test]
    async fn should_return_private_no_store_for_valid_credential_on_detail_and_catalogue()
    -> Result<(), Box<dyn std::error::Error>> {
        let auction_id = detail().auction_id;
        let authenticator = FakeAuthenticator { reject: false };

        let detail_response = app_with_auth(FakeCatalogue::default(), authenticator)
            .oneshot(
                Request::get(format!("/v1/auctions/{auction_id}"))
                    .header(header::AUTHORIZATION, "Bearer valid")
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(StatusCode::OK, detail_response.status());
        assert_eq!(
            "private, no-store",
            detail_response.headers()[header::CACHE_CONTROL]
        );

        let catalogue_response = app_with_auth(FakeCatalogue::default(), authenticator)
            .oneshot(
                Request::get(format!("/v1/auctions/{auction_id}/product-listings"))
                    .header(header::AUTHORIZATION, "Bearer valid")
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(StatusCode::OK, catalogue_response.status());
        assert_eq!(
            "private, no-store",
            catalogue_response.headers()[header::CACHE_CONTROL]
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_cache_anonymous_auction_directory_for_sixty_seconds()
    -> Result<(), Box<dyn std::error::Error>> {
        let response = app(FakeCatalogue::default())
            .oneshot(Request::get("/v1/auctions").body(Body::empty())?)
            .await?;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(
            "public, max-age=0, s-maxage=60, stale-if-error=0",
            response.headers()[header::CACHE_CONTROL]
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_invalid_directory_sort_parameters_without_store()
    -> Result<(), Box<dyn std::error::Error>> {
        for query in [
            "sort=scheduled",
            "sort=bogus",
            "order=bogus",
            "sort=scheduled&timeRole=LIVE_STARTS&to=2026-01-01T00%3A00%3A00Z",
        ] {
            let response = app(FakeCatalogue::default())
                .oneshot(Request::get(format!("/v1/auctions?{query}")).body(Body::empty())?)
                .await?;
            assert_eq!(StatusCode::BAD_REQUEST, response.status(), "{query}");
            assert_eq!(
                "private, no-store",
                response.headers()[header::CACHE_CONTROL]
            );
        }
        Ok(())
    }

    #[test]
    fn should_parse_catalogue_cursor_as_complete_json_string()
    -> Result<(), Box<dyn std::error::Error>> {
        let auction_id = AuctionId::new();
        let product_listing_id = product_listing_core::product_listing_id::ProductListingId::new();
        let value = serde_json::json!({
            "auctionId": auction_id.to_string(),
            "cataloguePosition": 7,
            "productListingId": product_listing_id.to_string(),
        });

        let cursor = parse_catalogue_cursor(value.to_string())?;

        assert_eq!(auction_id, cursor.auction_id);
        assert_eq!(Some(7), cursor.catalogue_position);
        assert_eq!(product_listing_id, cursor.product_listing_id);
        Ok(())
    }

    #[test]
    fn should_parse_directory_cursor_as_complete_json_string()
    -> Result<(), Box<dyn std::error::Error>> {
        let auction_id = AuctionId::new();
        let value = serde_json::json!({
            "created": "1970-01-01T00:00:00Z",
            "auctionId": auction_id.to_string(),
            "scope": {
                "listingSourceId": null,
                "format": null,
                "reportedStatus": null,
                "timeRole": null,
                "from": null,
                "to": null,
            },
        });

        let cursor = parse_directory_cursor(value.to_string())?;

        assert_eq!(auction_id, cursor.auction_id);
        assert_eq!(None, cursor.scope.listing_source_id);
        assert_eq!(None, cursor.scope.sort);
        assert_eq!(None, cursor.scheduled);
        Ok(())
    }

    #[test]
    fn should_round_trip_returned_cursor_through_url_encoded_json()
    -> Result<(), Box<dyn std::error::Error>> {
        let auction_id = AuctionId::new();
        let product_listing_id = product_listing_core::product_listing_id::ProductListingId::new();
        let catalogue_json = serde_json::to_string(&CatalogueCursorData {
            auction_id: auction_id.to_string(),
            catalogue_position: Some(7),
            product_listing_id: product_listing_id.to_string(),
        })?;
        let catalogue_query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("searchAfter", &catalogue_json)
            .finish();
        let catalogue_query = crate::wire::from_query_str::<CatalogueQuery>(&catalogue_query)?;
        let catalogue_cursor = parse_catalogue_cursor(
            catalogue_query
                .search_after
                .ok_or("catalogue cursor missing after URL decoding")?,
        )?;
        assert_eq!(auction_id, catalogue_cursor.auction_id);
        assert_eq!(Some(7), catalogue_cursor.catalogue_position);

        let directory_json = serde_json::to_string(&DirectoryCursorData {
            created: OffsetDateTime::UNIX_EPOCH,
            auction_id: auction_id.to_string(),
            scheduled: None,
            scope: DirectoryCursorScopeData {
                listing_source_id: None,
                format: None,
                reported_status: None,
                time_role: None,
                sort: None,
                order: None,
                from: None,
                to: None,
            },
        })?;
        let directory_query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("searchAfter", &directory_json)
            .finish();
        let directory_query = crate::wire::from_query_str::<DirectoryQuery>(&directory_query)?;
        let directory_cursor = parse_directory_cursor(
            directory_query
                .search_after
                .ok_or("directory cursor missing after URL decoding")?,
        )?;
        assert_eq!(auction_id, directory_cursor.auction_id);
        assert_eq!(OffsetDateTime::UNIX_EPOCH, directory_cursor.created);
        Ok(())
    }

    #[test]
    fn should_parse_directory_sort_order_and_unbounded_schedule()
    -> Result<(), Box<dyn std::error::Error>> {
        let default = crate::wire::from_query_str::<DirectoryQuery>("")?.try_into_request()?;
        assert_eq!(None, default.sort);
        assert_eq!(SortOrder::Desc, default.order());
        assert_eq!(None, default.scope().order);

        let scheduled =
            crate::wire::from_query_str::<DirectoryQuery>("sort=scheduled&timeRole=LIVE_STARTS")?
                .try_into_request()?;
        assert_eq!(Some(AuctionSchedulePoint::LiveStarts), scheduled.sort);
        assert_eq!(SortOrder::Asc, scheduled.order());
        assert_eq!(None, scheduled.schedule);
        assert_eq!(Some(SortOrder::Asc), scheduled.scope().order);

        let descending = crate::wire::from_query_str::<DirectoryQuery>(
            "sort=scheduled&timeRole=SCHEDULED_END&order=desc",
        )?
        .try_into_request()?;
        assert_eq!(SortOrder::Desc, descending.order());
        let ascending_created =
            crate::wire::from_query_str::<DirectoryQuery>("sort=created&order=asc")?
                .try_into_request()?;
        assert_eq!(SortOrder::Asc, ascending_created.order());
        Ok(())
    }

    #[test]
    fn should_reject_invalid_directory_sort_and_partial_schedule_ranges() {
        for query in [
            "sort=scheduled",
            "sort=unexpected",
            "order=up",
            "timeRole=LIVE_STARTS",
            "sort=scheduled&timeRole=LIVE_STARTS&from=2026-01-01T00%3A00%3A00Z",
            "from=2026-01-01T00%3A00%3A00Z",
        ] {
            let parsed = crate::wire::from_query_str::<DirectoryQuery>(query)
                .unwrap_or_else(|error| panic!("failed to parse {query}: {error}"));
            assert!(parsed.try_into_request().is_err(), "accepted {query}");
        }
    }

    #[test]
    fn should_round_trip_nullable_scheduled_cursor_and_reject_missing_instant()
    -> Result<(), Box<dyn std::error::Error>> {
        let auction_id = AuctionId::new();
        let cursor = DirectoryCursorData {
            created: OffsetDateTime::UNIX_EPOCH,
            auction_id: auction_id.to_string(),
            scheduled: Some(None),
            scope: DirectoryCursorScopeData {
                listing_source_id: None,
                format: None,
                reported_status: None,
                time_role: Some("LIVE_STARTS".to_owned()),
                sort: Some("scheduled".to_owned()),
                order: Some(SortOrder::Asc),
                from: None,
                to: None,
            },
        };
        let value = serde_json::to_value(&cursor)?;
        assert!(
            value
                .get("scheduled")
                .is_some_and(serde_json::Value::is_null)
        );
        let parsed = parse_directory_cursor(value.to_string())?;
        assert_eq!(None, parsed.scheduled);
        assert_eq!(Some(AuctionSchedulePoint::LiveStarts), parsed.scope.sort);
        assert!(
            parse_directory_cursor(
                serde_json::json!({
                    "created": "1970-01-01T00:00:00Z", "auctionId": auction_id.to_string(),
                    "scope": value["scope"]
                })
                .to_string()
            )
            .is_err()
        );
        let instant = time::macros::datetime!(2026-01-01 00:00 UTC);
        let mut cursor = cursor;
        cursor.scheduled = Some(Some(instant));
        assert_eq!(
            Some(instant),
            parse_directory_cursor(serde_json::to_string(&cursor)?)?.scheduled
        );
        Ok(())
    }

    #[test]
    fn should_keep_legacy_directory_cursor_json_shape() -> Result<(), Box<dyn std::error::Error>> {
        let cursor = DirectoryCursorData {
            created: OffsetDateTime::UNIX_EPOCH,
            auction_id: AuctionId::new().to_string(),
            scheduled: None,
            scope: DirectoryCursorScopeData {
                listing_source_id: None,
                format: None,
                reported_status: None,
                time_role: None,
                sort: None,
                order: None,
                from: None,
                to: None,
            },
        };
        let value = serde_json::to_value(cursor)?;
        assert!(value.get("scheduled").is_none());
        assert!(value["scope"].get("sort").is_none());
        assert!(value["scope"].get("order").is_none());
        Ok(())
    }

    #[test]
    fn should_reject_malformed_json_cursors() {
        assert!(parse_catalogue_cursor("not-json".to_owned()).is_err());
        assert!(parse_directory_cursor("not-json".to_owned()).is_err());
    }
}
