use super::types::{PatchWatchlistData, watchlist_state};
use super::util::{no_store, parse_json};
use crate::auth::protected_context;
use crate::error::{ApiError, BAD_QUERY_PARAMETER_VALUE};
use crate::patch_value::non_nullable_option;
use crate::product_listings::product_data::personalized_product_details_data;
use crate::state::WatchlistState;
use crate::wire::parse_path_object_id;
use axum::Json;
use axum::extract::{Path, RawQuery, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use localization::Language;
use money::Currency;
use product_listing_core::product_listing_id::ProductListingId;
use serde::Deserialize;
use watchlist_service::use_cases::UpdateWatchlistProductListingCommand;

#[derive(Debug, Deserialize)]
struct PatchWatchlistQuery {
    #[serde(default, with = "crate::wire::language")]
    language: Language,
    #[serde(default, with = "crate::wire::currency")]
    currency: Currency,
}

pub async fn patch_watchlist(
    State(state): State<WatchlistState>,
    headers: HeaderMap,
    Path(raw_product_listing_id): Path<String>,
    RawQuery(raw_query): RawQuery,
    body: String,
) -> Response {
    let query: PatchWatchlistQuery =
        match crate::wire::from_query_str(raw_query.as_deref().unwrap_or_default()) {
            Ok(query) => query,
            Err(error) => {
                return ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
                    .with_detail(error.to_string())
                    .into_response();
            }
        };
    let (ctx, user_id) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let product_listing_id = match parse_path_object_id::<ProductListingId>(
        &raw_product_listing_id,
        "productListingId",
        "ProductListing",
    ) {
        Ok(value) => value,
        Err(error) => return error.into_response(),
    };
    let data: PatchWatchlistData = match parse_json(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let notifications = match non_nullable_option(data.notifications, "notifications") {
        Ok(notifications) => notifications,
        Err(error) => return error.into_response(),
    };
    let state_field = match non_nullable_option(data.state, "state") {
        Ok(state_field) => state_field.map(watchlist_state),
        Err(error) => return error.into_response(),
    };
    match state
        .update_watchlist_product
        .execute(
            &ctx,
            UpdateWatchlistProductListingCommand {
                user_id,
                product_listing_id,
                notifications,
                state: state_field,
                language: query.language,
                currency: query.currency,
            },
        )
        .await
    {
        Ok(result) => {
            no_store(Json(personalized_product_details_data(result.product)).into_response())
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{
        AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal,
    };
    use application::operation_context::{OperationContext, Principal};
    use application::personalized::Personalized;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use domain_primitives::event_id::EventId;
    use fxrate_core::FxRateId;
    use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
    use localization::Localized;
    use product_listing_core::{
        listing_availability::ListingAvailability, listing_lifecycle::ListingLifecycle,
        product_listing::ProductListingPricing, source_listing_id::SourceListingId, title::Title,
    };
    use product_listing_service::ports::ListingSourceSummary;
    use product_listing_service::use_cases::{
        DisplayProductListingPricing, PersonalizedProductListingDetailsView,
        ProductListingDetailsView, ProductListingPricingPresentation,
        ProductListingPricingValuation,
    };
    use product_listing_service::user_state::{ProductListingUserStateView, WatchlistUserState};
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};
    use time::OffsetDateTime;
    use tower::ServiceExt;
    use url::Url;
    use user_core::user_id::UserId;
    use watchlist_core::watchlist_state::WatchlistState as EntryState;
    use watchlist_service::use_cases::{
        ListWatchlistError, ListWatchlistRequest, ListWatchlistResult, ListWatchlistUseCase,
        UnwatchProductListingCommand, UnwatchProductListingError, UnwatchProductListingResult,
        UnwatchProductListingUseCase, UpdateWatchlistProductListingError,
        UpdateWatchlistProductListingResult, UpdateWatchlistProductListingUseCase,
        WatchProductListingCommand, WatchProductListingError, WatchProductListingResult,
        WatchProductListingUseCase,
    };

    type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;
    type RecordedCommands =
        Arc<Mutex<Vec<(OperationContext, UpdateWatchlistProductListingCommand)>>>;

    struct FakeUpdate {
        result: UpdateWatchlistProductListingResult,
        commands: RecordedCommands,
    }

    #[async_trait::async_trait]
    impl UpdateWatchlistProductListingUseCase for FakeUpdate {
        async fn execute(
            &self,
            context: &OperationContext,
            command: UpdateWatchlistProductListingCommand,
        ) -> Result<UpdateWatchlistProductListingResult, UpdateWatchlistProductListingError>
        {
            self.commands
                .lock()
                .expect("recorded commands lock")
                .push((context.clone(), command));
            Ok(self.result.clone())
        }
    }

    struct FakeAuthenticator(UserId);

    #[async_trait::async_trait]
    impl TokenAuthenticator for FakeAuthenticator {
        async fn authenticate(
            &self,
            bearer_token: &str,
            _: &RequestMetadata,
        ) -> Result<TransportPrincipal, AuthError> {
            if bearer_token != "valid" {
                return Err(AuthError::InvalidCredentials);
            }
            Ok(TransportPrincipal::User {
                user_id: self.0,
                auth_method: AuthMethod::CognitoJwt,
                capabilities: Default::default(),
            })
        }
    }

    struct UnusedUseCases;

    #[async_trait::async_trait]
    impl ListWatchlistUseCase for UnusedUseCases {
        async fn execute(
            &self,
            _: &OperationContext,
            _: ListWatchlistRequest,
        ) -> Result<ListWatchlistResult, ListWatchlistError> {
            unreachable!("PATCH must not invoke the list use case")
        }
    }

    #[async_trait::async_trait]
    impl WatchProductListingUseCase for UnusedUseCases {
        async fn execute(
            &self,
            _: &OperationContext,
            _: WatchProductListingCommand,
        ) -> Result<WatchProductListingResult, WatchProductListingError> {
            unreachable!("PATCH must not invoke the create use case")
        }
    }

    #[async_trait::async_trait]
    impl UnwatchProductListingUseCase for UnusedUseCases {
        async fn execute(
            &self,
            _: &OperationContext,
            _: UnwatchProductListingCommand,
        ) -> Result<UnwatchProductListingResult, UnwatchProductListingError> {
            unreachable!("PATCH must not invoke the delete use case")
        }
    }

    fn product(
        product_listing_id: ProductListingId,
        watchlist: WatchlistUserState,
    ) -> PersonalizedProductListingDetailsView {
        let url = Url::parse("https://example.test/product").expect("valid product URL");
        Personalized {
            item: ProductListingDetailsView {
                shipping: None,
                product_listing_id,
                product_listing_title_slug_id: None,
                event_id: EventId::new(),
                source: ListingSourceSummary {
                    listing_source_id: ListingSourceId::new(),
                    name: ListingSourceName::try_from("Source").expect("valid source name"),
                    slug_id: ListingSourceSlugId::raw("source").expect("valid source slug"),
                },
                source_listing_id: SourceListingId::try_from("product")
                    .expect("valid source listing ID"),
                product_title: None,
                product_description: None,
                title: Some(Localized::new(
                    Language::En,
                    Title::from("Watchlist cabinet"),
                )),
                description: None,
                pricing: ProductListingPricingPresentation {
                    source: ProductListingPricing::default(),
                    display: DisplayProductListingPricing {
                        price: None,
                        price_estimate_min: None,
                        price_estimate_max: None,
                    },
                    valuation: ProductListingPricingValuation::Current {
                        fx_rate_id: FxRateId::new(),
                        captured_at: OffsetDateTime::UNIX_EPOCH,
                    },
                },
                availability: Some(ListingAvailability::Available),
                lifecycle: ListingLifecycle::Active,
                url: url.clone(),
                view_url: url,
                images: Vec::new(),
                content_policy: None,
                auction: None,
                lot: None,
                created: OffsetDateTime::UNIX_EPOCH,
                updated: OffsetDateTime::UNIX_EPOCH,
            },
            user_state: Some(ProductListingUserStateView {
                watchlist: Some(watchlist),
                ..Default::default()
            }),
        }
    }

    fn app(
        user_id: UserId,
        product_listing_id: ProductListingId,
        watchlist: WatchlistUserState,
    ) -> (Router, RecordedCommands) {
        let commands = Arc::new(Mutex::new(Vec::new()));
        let state = WatchlistState::new(
            Arc::new(UnusedUseCases),
            Arc::new(UnusedUseCases),
            Arc::new(FakeUpdate {
                result: UpdateWatchlistProductListingResult {
                    product: product(product_listing_id, watchlist),
                },
                commands: Arc::clone(&commands),
            }),
            Arc::new(UnusedUseCases),
            Arc::new(FakeAuthenticator(user_id)),
        );
        (
            Router::new()
                .route(
                    "/api/v1/me/watchlist/{product_listing_id}",
                    axum::routing::patch(patch_watchlist),
                )
                .with_state(state),
            commands,
        )
    }

    fn request(
        product_listing_id: ProductListingId,
        query: &str,
        body: &'static str,
    ) -> Request<Body> {
        Request::patch(format!("/api/v1/me/watchlist/{product_listing_id}{query}"))
            .header(header::AUTHORIZATION, "Bearer valid")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .expect("valid PATCH request")
    }

    async fn json(response: Response) -> TestResult<Value> {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    async fn assert_product_response(
        response: Response,
        product_listing_id: ProductListingId,
        watchlist: WatchlistUserState,
    ) -> TestResult {
        assert_eq!(StatusCode::OK, response.status());
        assert_eq!("no-store", response.headers()[header::CACHE_CONTROL]);
        let body = json(response).await?;
        assert_eq!(Some(2), body.as_object().map(|object| object.len()));
        assert_eq!(
            product_listing_id.to_string(),
            body["item"]["productListingId"]
        );
        assert_eq!(
            json!({"text": "Watchlist cabinet", "language": "en"}),
            body["item"]["title"]
        );
        assert_eq!("CURRENT", body["item"]["pricing"]["valuation"]["type"]);
        // Sections without a matching read scope are omitted, not defaulted.
        assert_eq!(
            json!({
                "watchlist": {"watching": watchlist.watching, "notifications": watchlist.notifications}
            }),
            body["userState"]
        );
        for field in [
            "userId",
            "productListingId",
            "state",
            "notifications",
            "created",
            "updated",
        ] {
            assert!(
                body.get(field).is_none(),
                "unexpected bare entry field: {field}"
            );
        }
        Ok(())
    }

    fn assert_command(commands: &RecordedCommands, expected: UpdateWatchlistProductListingCommand) {
        let commands = commands.lock().expect("recorded commands lock");
        assert_eq!(1, commands.len());
        assert_eq!(Principal::User(expected.user_id), commands[0].0.principal);
        assert_eq!(expected, commands[0].1);
    }

    #[tokio::test]
    async fn should_return_canonical_product_for_empty_patch_and_forward_presentation_defaults()
    -> TestResult {
        let user_id = UserId::new();
        let product_listing_id = ProductListingId::new();
        let watchlist = WatchlistUserState {
            watching: false,
            notifications: true,
        };
        let (app, commands) = app(user_id, product_listing_id, watchlist);

        let response = app.oneshot(request(product_listing_id, "", "{}")).await?;

        assert_product_response(response, product_listing_id, watchlist).await?;
        assert_command(
            &commands,
            UpdateWatchlistProductListingCommand {
                user_id,
                product_listing_id,
                notifications: None,
                state: None,
                language: Language::En,
                currency: Currency::Eur,
            },
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_return_canonical_changed_product_and_forward_state_notifications_and_presentation()
    -> TestResult {
        let user_id = UserId::new();
        let product_listing_id = ProductListingId::new();
        for (body, state, watching, notifications) in [
            (
                r#"{"state":"ACTIVE","notifications":false}"#,
                EntryState::Active,
                true,
                false,
            ),
            (
                r#"{"state":"INACTIVE_BY_USER","notifications":true}"#,
                EntryState::InactiveByUser,
                false,
                true,
            ),
        ] {
            let watchlist = WatchlistUserState {
                watching,
                notifications,
            };
            let (app, commands) = app(user_id, product_listing_id, watchlist);

            let response = app
                .oneshot(request(
                    product_listing_id,
                    "?language=de&currency=USD",
                    body,
                ))
                .await?;

            assert_product_response(response, product_listing_id, watchlist).await?;
            assert_command(
                &commands,
                UpdateWatchlistProductListingCommand {
                    user_id,
                    product_listing_id,
                    notifications: Some(notifications),
                    state: Some(state),
                    language: Language::De,
                    currency: Currency::Usd,
                },
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_invalid_language_or_currency_without_executing_update() -> TestResult {
        let product_listing_id = ProductListingId::new();
        let (app, commands) = app(
            UserId::new(),
            product_listing_id,
            WatchlistUserState::default(),
        );
        for query in ["?language=invalid", "?currency=XXX"] {
            let response = app
                .clone()
                .oneshot(request(product_listing_id, query, r#"{"state":"ACTIVE"}"#))
                .await?;

            assert_eq!(StatusCode::BAD_REQUEST, response.status());
            assert_eq!("BAD_QUERY_PARAMETER_VALUE", json(response).await?["error"]);
        }
        assert!(commands.lock().expect("recorded commands lock").is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_null_state_without_executing_update() -> TestResult {
        let product_listing_id = ProductListingId::new();
        let (app, commands) = app(
            UserId::new(),
            product_listing_id,
            WatchlistUserState::default(),
        );

        let response = app
            .oneshot(request(
                product_listing_id,
                "",
                r#"{"state":null,"notifications":false}"#,
            ))
            .await?;

        assert_eq!(StatusCode::BAD_REQUEST, response.status());
        let body = json(response).await?;
        assert_eq!("BAD_BODY_VALUE", body["error"]);
        assert_eq!("Body field 'state' must not be null.", body["detail"]);
        assert!(commands.lock().expect("recorded commands lock").is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_missing_or_invalid_auth_without_executing_update() -> TestResult {
        let product_listing_id = ProductListingId::new();
        let (app, commands) = app(
            UserId::new(),
            product_listing_id,
            WatchlistUserState::default(),
        );
        for authorization in [None, Some("Bearer invalid")] {
            let mut request = request(product_listing_id, "", "{}");
            request.headers_mut().remove(header::AUTHORIZATION);
            if let Some(authorization) = authorization {
                request
                    .headers_mut()
                    .insert(header::AUTHORIZATION, authorization.parse()?);
            }
            let response = app.clone().oneshot(request).await?;

            assert_eq!(StatusCode::UNAUTHORIZED, response.status());
            assert_eq!("INVALID_CREDENTIALS", json(response).await?["error"]);
        }
        assert!(commands.lock().expect("recorded commands lock").is_empty());
        Ok(())
    }
}
