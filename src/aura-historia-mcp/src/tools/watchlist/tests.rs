mod schema;

use super::*;
use crate::{MAX_RESPONSE_BYTES, Registry};
use application::{
    error::static_error,
    operation_context::{CorrelationId, OperationContext, RequestId},
    pagination::{Cursor, CursoredResult},
    personalized::Personalized,
};
use aura_historia_contracts::watchlist::watchlist_cursor_value;
use fxrate_core::FxRateId;
use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
use localization::{Language, Localized};
use money::{Currency, MonetaryAmount, Price};
use product_listing_core::{
    content_policy::{ContentPolicyDecision, SensitiveContentCategory},
    listing_availability::ListingAvailability,
    listing_lifecycle::ListingLifecycle,
    product_listing::ProductListingPricing,
    source_listing_id::SourceListingId,
    title::Title,
};
use product_listing_service::{
    ports::{ListingSourceSummary, ProductListingWatchlistDetailsCursor},
    use_cases::*,
    user_state::{ProductListingUserStateView, WatchlistUserState},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;
use watchlist_core::{
    NewWatchlistProductListing, WatchlistProductListing, watchlist_state::WatchlistState,
};
use watchlist_service::use_cases::*;

#[derive(Clone, Copy, Default)]
enum Outcome {
    #[default]
    Success,
    Quota,
    Missing,
    Conflict,
    Unavailable,
    Pending,
}

#[derive(Debug, PartialEq)]
enum Call {
    List(OperationContext, ListWatchlistRequest),
    Add(OperationContext, WatchProductListingCommand),
    Update(OperationContext, UpdateWatchlistProductListingCommand),
    Remove(OperationContext, UnwatchProductListingCommand),
}
struct Fixture {
    outcome: Outcome,
    calls: Mutex<Vec<Call>>,
    product: PersonalizedProductListingDetailsView,
}
impl Fixture {
    fn new(outcome: Outcome) -> Arc<Self> {
        Arc::new(Self {
            outcome,
            calls: Mutex::new(vec![]),
            product: product(),
        })
    }
    fn services(self: &Arc<Self>) -> Watchlist {
        Watchlist {
            list: self.clone(),
            add: self.clone(),
            update: self.clone(),
            remove: self.clone(),
        }
    }
}
#[async_trait::async_trait]
impl ListWatchlistUseCase for Fixture {
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListWatchlistRequest,
    ) -> Result<ListWatchlistResult, ListWatchlistError> {
        self.calls
            .lock()
            .unwrap()
            .push(Call::List(context.clone(), request.clone()));
        if matches!(self.outcome, Outcome::Unavailable) {
            return Err(ListWatchlistError::PricingFxSnapshotUnavailable {
                source: static_error("PRIVATE_FX_SENTINEL"),
            });
        }
        let continuation = request.cursor.search_after;
        Ok(CursoredResult {
            items: if continuation.is_some() {
                vec![]
            } else {
                vec![self.product.clone()]
            },
            cursor: Cursor {
                size: request.cursor.size,
                search_after: continuation.is_none().then_some(
                    ProductListingWatchlistDetailsCursor {
                        watchlist_created: OffsetDateTime::UNIX_EPOCH,
                        product_listing_id: self.product.item.product_listing_id,
                    },
                ),
            },
            total: Some(1),
        })
    }
}
#[async_trait::async_trait]
impl WatchProductListingUseCase for Fixture {
    async fn execute(
        &self,
        context: &OperationContext,
        command: WatchProductListingCommand,
    ) -> Result<WatchProductListingResult, WatchProductListingError> {
        self.calls
            .lock()
            .unwrap()
            .push(Call::Add(context.clone(), command.clone()));
        match self.outcome {
            Outcome::Quota => {
                return Err(WatchProductListingError::WatchlistQuotaExceeded {
                    active_count: 10,
                    quota: 10,
                });
            }
            Outcome::Missing => return Err(WatchProductListingError::ProductListingNotFound),
            Outcome::Conflict => return Err(WatchProductListingError::AlreadyExists),
            Outcome::Unavailable => {
                return Err(WatchProductListingError::CommitTransactionFailed(
                    static_error("PRIVATE_COMMIT_SENTINEL"),
                ));
            }
            Outcome::Pending => return std::future::pending().await,
            Outcome::Success => {}
        }
        Ok(WatchProductListingResult {
            entry: WatchlistProductListing::create(NewWatchlistProductListing {
                user_id: command.user_id,
                product_listing_id: command.product_listing_id,
                notifications: command.notifications,
                state: WatchlistState::Active,
            }),
        })
    }
}
#[async_trait::async_trait]
impl UpdateWatchlistProductListingUseCase for Fixture {
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpdateWatchlistProductListingCommand,
    ) -> Result<UpdateWatchlistProductListingResult, UpdateWatchlistProductListingError> {
        self.calls
            .lock()
            .unwrap()
            .push(Call::Update(context.clone(), command));
        match self.outcome {
            Outcome::Quota => {
                return Err(UpdateWatchlistProductListingError::WatchlistQuotaExceeded {
                    active_count: 10,
                    quota: 10,
                });
            }
            Outcome::Missing => return Err(UpdateWatchlistProductListingError::NotFound),
            Outcome::Conflict => {
                return Err(UpdateWatchlistProductListingError::ConcurrencyConflict);
            }
            _ => {}
        }
        Ok(UpdateWatchlistProductListingResult {
            product: self.product.clone(),
        })
    }
}
#[async_trait::async_trait]
impl UnwatchProductListingUseCase for Fixture {
    async fn execute(
        &self,
        context: &OperationContext,
        command: UnwatchProductListingCommand,
    ) -> Result<UnwatchProductListingResult, UnwatchProductListingError> {
        self.calls
            .lock()
            .unwrap()
            .push(Call::Remove(context.clone(), command.clone()));
        match self.outcome {
            Outcome::Missing => return Err(UnwatchProductListingError::NotFound),
            Outcome::Conflict => return Err(UnwatchProductListingError::ConcurrencyConflict),
            _ => {}
        }
        Ok(UnwatchProductListingResult {
            user_id: command.user_id,
            product_listing_id: command.product_listing_id,
        })
    }
}
fn product() -> PersonalizedProductListingDetailsView {
    let id = ProductListingId::new();
    Personalized {
        item: ProductListingDetailsView {
            shipping: None,
            product_listing_id: id,
            product_listing_title_slug_id: None,
            event_id: domain_primitives::event_id::EventId::new(),
            source: ListingSourceSummary {
                listing_source_id: ListingSourceId::new(),
                name: ListingSourceName::try_from("Watchlist source").unwrap(),
                slug_id: ListingSourceSlugId::raw("watchlist-source").unwrap(),
            },
            source_listing_id: SourceListingId::try_from("source:item/42").unwrap(),
            product_title: None,
            product_description: None,
            title: Some(Localized::new(
                Language::De,
                Title::from("Watchlist-Schrank"),
            )),
            description: None,
            pricing: ProductListingPricingPresentation {
                source: ProductListingPricing {
                    price: Some(Price::new(MonetaryAmount::from(u64::MAX), Currency::Eur).into()),
                    ..Default::default()
                },
                display: DisplayProductListingPricing {
                    price: Some(Price::new(MonetaryAmount::from(u64::MAX), Currency::Usd).into()),
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
            url: "https://shop.example.test/42".parse().unwrap(),
            view_url: format!("https://aura.example.test/{id}").parse().unwrap(),
            images: (0..12)
                .map(|index| ProductListingImageView {
                    url: Some(
                        format!(
                            "https://images.example.test/{index}?signature={}",
                            "a".repeat(500)
                        )
                        .parse()
                        .unwrap(),
                    ),
                })
                .collect(),
            content_policy: Some(ContentPolicyDecision::RequiresConsent(
                SensitiveContentCategory::NaziGermany,
            )),
            auction: None,
            lot: None,
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
        },
        user_state: Some(ProductListingUserStateView {
            watchlist: Some(WatchlistUserState {
                watching: true,
                notifications: false,
            }),
            ..Default::default()
        }),
    }
}
fn context(user: UserId) -> InvocationContext {
    InvocationContext {
        operation: OperationContext {
            principal: Principal::DelegatedUser {
                user_id: user,
                capabilities: BTreeSet::from([
                    CredentialCapability::WatchlistRead,
                    CredentialCapability::WatchlistWrite,
                ]),
            },
            request_id: RequestId::new("watchlist-request"),
            correlation_id: CorrelationId::new("watchlist-correlation"),
        },
        deadline: tokio::time::Instant::now() + std::time::Duration::from_secs(10),
        cancellation: CancellationToken::new(),
        max_response_bytes: MAX_RESPONSE_BYTES,
    }
}
fn registry(fixture: &Arc<Fixture>) -> Registry {
    Registry::new(operations(fixture.services()).unwrap()).unwrap()
}
async fn call(
    registry: &Registry,
    name: &str,
    args: Value,
    user: UserId,
) -> Result<Value, OperationError> {
    registry
        .get(name)
        .unwrap()
        .call(args, context(user))
        .await
        .map(|result| result.structured_content.unwrap())
}

#[test]
fn should_register_exact_family_and_manifest_policies_without_public_overlap() {
    let fixture = Fixture::new(Outcome::Success);
    let registry = registry(&fixture);
    let manifest: Value =
        serde_json::from_str(include_str!("../../../authenticated-operations.json")).unwrap();
    let public: Value =
        serde_json::from_str(include_str!("../../../public-operations.json")).unwrap();
    let names: BTreeSet<_> = operations(fixture.services())
        .unwrap()
        .into_iter()
        .map(|op| op.name().to_owned())
        .collect();
    assert_eq!(
        BTreeSet::from([
            "getWatchlistProductListings".to_owned(),
            "addWatchlistProduct".to_owned(),
            "deleteWatchlistProduct".to_owned(),
            "patchWatchlistProduct".to_owned()
        ]),
        names
    );
    let entries: Vec<_> = manifest["operations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|op| op["family"] == "watchlist")
        .collect();
    assert_eq!(4, entries.len());
    for entry in entries {
        let operation = registry.get(entry["name"].as_str().unwrap()).unwrap();
        let tool = serde_json::to_value(operation.tool()).unwrap();
        assert_eq!(entry["operationId"], tool["name"]);
        assert_eq!(entry["annotations"], tool["annotations"]);
        assert_eq!(entry["access"], tool["_meta"]["aura.historia/access"]);
        assert_eq!(
            entry["requiredScopes"],
            tool["_meta"]["aura.historia/requiredScopes"]
        );
        assert!(
            public["operations"]
                .as_array()
                .unwrap()
                .iter()
                .all(|op| op["name"] != entry["name"])
        );
    }
}

#[tokio::test]
async fn should_forward_presentation_and_continuation_without_dropping_listing_fields() {
    let fixture = Fixture::new(Outcome::Success);
    let registry = registry(&fixture);
    let user = UserId::new();
    let result = call(
        &registry,
        "getWatchlistProductListings",
        json!({"language":"de","currency":"USD","size":500}),
        user,
    )
    .await
    .unwrap();
    let expected =
        serde_json::to_value(personalized_product_details_data(fixture.product.clone())).unwrap();
    assert_eq!(expected, result["items"][0]);
    assert_eq!(1, result["size"]);
    assert_eq!(1, result["total"]);
    assert_eq!(
        12,
        result["items"][0]["item"]["images"]
            .as_array()
            .unwrap()
            .len()
    );
    assert_eq!(
        u64::MAX,
        result["items"][0]["item"]["pricing"]["source"]["price"]["amount"]
    );
    assert!(jsonschema::is_valid(
        &serde_json::to_value(
            registry
                .get("getWatchlistProductListings")
                .unwrap()
                .tool()
                .output_schema
                .as_ref()
                .unwrap()
        )
        .unwrap(),
        &result
    ));
    let after = call(
        &registry,
        "getWatchlistProductListings",
        json!({"searchAfter":result["searchAfter"].to_string()}),
        user,
    )
    .await
    .unwrap();
    assert_eq!(json!({"items":[],"size":0,"total":1}), after);
    let calls = fixture.calls.lock().unwrap();
    let Call::List(ctx, first) = &calls[0] else {
        panic!("list request")
    };
    assert_eq!(context(user).operation, *ctx);
    assert_eq!(
        (user, Language::De, Currency::Usd, 100),
        (
            first.user_id,
            first.language,
            first.currency,
            first.cursor.size
        )
    );
    let Call::List(_, second) = &calls[1] else {
        panic!("second list request")
    };
    assert_eq!(
        result["searchAfter"],
        watchlist_cursor_value(second.cursor.search_after.unwrap()).unwrap()
    );
}

#[tokio::test]
async fn should_preserve_add_defaults_and_bind_each_command_to_its_delegated_user() {
    let fixture = Fixture::new(Outcome::Success);
    let registry = registry(&fixture);
    let product = fixture.product.item.product_listing_id;
    let first = UserId::new();
    let second = UserId::new();
    for (user, args, notifications) in [
        (first, json!({"productListingId":product}), true),
        (
            second,
            json!({"productListingId":product,"notifications":null}),
            true,
        ),
        (
            first,
            json!({"productListingId":product,"notifications":false}),
            false,
        ),
    ] {
        let result = call(&registry, "addWatchlistProduct", args, user)
            .await
            .unwrap();
        assert_eq!(
            json!({"userId":user,"productListingId":product,"state":"ACTIVE","notifications":notifications}),
            result
        );
        assert!(jsonschema::is_valid(
            &serde_json::to_value(
                registry
                    .get("addWatchlistProduct")
                    .unwrap()
                    .tool()
                    .output_schema
                    .as_ref()
                    .unwrap()
            )
            .unwrap(),
            &result
        ));
    }
    let calls = fixture.calls.lock().unwrap();
    for (call, user, notifications) in [
        (&calls[0], first, true),
        (&calls[1], second, true),
        (&calls[2], first, false),
    ] {
        let Call::Add(ctx, command) = call else {
            panic!("add request")
        };
        assert_eq!(context(user).operation, *ctx);
        assert_eq!(
            WatchProductListingCommand {
                user_id: user,
                product_listing_id: product,
                notifications
            },
            *command
        );
    }
}

#[tokio::test]
async fn should_preserve_patch_omission_values_and_complete_personalized_output() {
    let fixture = Fixture::new(Outcome::Success);
    let registry = registry(&fixture);
    let user = UserId::new();
    let product = fixture.product.item.product_listing_id;
    for (args, notifications, state, language, currency) in [
        (
            json!({"productListingId":product}),
            None,
            None,
            Language::En,
            Currency::Eur,
        ),
        (
            json!({"productListingId":product,"notifications":false,"state":"INACTIVE_BY_USER","language":"de","currency":"USD"}),
            Some(false),
            Some(WatchlistState::InactiveByUser),
            Language::De,
            Currency::Usd,
        ),
        (
            json!({"productListingId":product,"notifications":true,"state":"ACTIVE"}),
            Some(true),
            Some(WatchlistState::Active),
            Language::En,
            Currency::Eur,
        ),
    ] {
        let result = call(&registry, "patchWatchlistProduct", args, user)
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(personalized_product_details_data(fixture.product.clone()))
                .unwrap(),
            result
        );
        assert!(result.get("result").is_none());
        let calls = fixture.calls.lock().unwrap();
        let Call::Update(ctx, command) = calls.last().unwrap() else {
            panic!("patch request")
        };
        assert_eq!(context(user).operation, *ctx);
        assert_eq!(
            UpdateWatchlistProductListingCommand {
                user_id: user,
                product_listing_id: product,
                notifications,
                state,
                language,
                currency
            },
            *command
        );
    }
    let result = call(
        &registry,
        "deleteWatchlistProduct",
        json!({"productListingId":product}),
        user,
    )
    .await
    .unwrap();
    assert_eq!(json!({"result":null}), result);
    assert!(
        matches!(fixture.calls.lock().unwrap().last(),Some(Call::Remove(_,command)) if command.user_id==user && command.product_listing_id==product)
    );
}

#[tokio::test]
async fn should_reject_invalid_arguments_and_other_user_targets_before_executing() {
    let fixture = Fixture::new(Outcome::Success);
    let registry = registry(&fixture);
    let user = UserId::new();
    let product = fixture.product.item.product_listing_id;
    for (name, args) in [
        (
            "getWatchlistProductListings",
            json!({"userId":UserId::new()}),
        ),
        ("getWatchlistProductListings", json!({"searchAfter":"bad"})),
        ("getWatchlistProductListings", json!({"language":"xx"})),
        (
            "addWatchlistProduct",
            json!({"productListingId":product,"userId":UserId::new()}),
        ),
        ("addWatchlistProduct", json!({"productListingId":user})),
        (
            "deleteWatchlistProduct",
            json!({"productListingId":product.as_uuid()}),
        ),
        (
            "patchWatchlistProduct",
            json!({"productListingId":product,"state":null}),
        ),
        (
            "patchWatchlistProduct",
            json!({"productListingId":product,"notifications":null}),
        ),
        (
            "patchWatchlistProduct",
            json!({"productListingId":product,"state":"INACTIVE_BY_RESTRICTED_PLAN"}),
        ),
        (
            "patchWatchlistProduct",
            json!({"productListingId":product,"userId":UserId::new()}),
        ),
    ] {
        assert!(
            matches!(
                call(&registry, name, args.clone(), user).await,
                Err(OperationError::InvalidArguments(_))
            ),
            "{name} {args}"
        );
    }
    for name in [
        "getWatchlistProductListings",
        "addWatchlistProduct",
        "deleteWatchlistProduct",
        "patchWatchlistProduct",
    ] {
        let mut ctx = context(user);
        ctx.operation.principal = Principal::User(user);
        assert!(matches!(
            registry
                .get(name)
                .unwrap()
                .call(
                    if name == "getWatchlistProductListings" {
                        json!({})
                    } else {
                        json!({"productListingId":product})
                    },
                    ctx
                )
                .await,
            Err(OperationError::Forbidden)
        ));
    }
    assert!(fixture.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn should_preserve_service_conflict_missing_and_quota_errors() {
    let user = UserId::new();
    for (outcome, name, expected) in [
        (Outcome::Conflict, "addWatchlistProduct", "CONFLICT"),
        (Outcome::Conflict, "patchWatchlistProduct", "CONFLICT"),
        (Outcome::Conflict, "deleteWatchlistProduct", "CONFLICT"),
        (Outcome::Missing, "addWatchlistProduct", "NOT_FOUND"),
        (Outcome::Missing, "patchWatchlistProduct", "NOT_FOUND"),
        (Outcome::Missing, "deleteWatchlistProduct", "NOT_FOUND"),
        (
            Outcome::Quota,
            "addWatchlistProduct",
            "WATCHLIST_QUOTA_EXCEEDED",
        ),
        (
            Outcome::Quota,
            "patchWatchlistProduct",
            "WATCHLIST_QUOTA_EXCEEDED",
        ),
    ] {
        let fixture = Fixture::new(outcome);
        let error = call(
            &registry(&fixture),
            name,
            json!({"productListingId":fixture.product.item.product_listing_id}),
            user,
        )
        .await
        .unwrap_err();
        let result = serde_json::to_value(error.into_protocol().unwrap()).unwrap();
        assert_eq!(expected, result["content"][0]["text"]);
        assert_eq!(true, result["isError"]);
    }
}

#[tokio::test]
async fn should_preserve_technical_causes_and_hide_them_from_tool_errors() {
    let fixture = Fixture::new(Outcome::Unavailable);
    let registry = registry(&fixture);
    for (name, args, sentinel) in [
        (
            "getWatchlistProductListings",
            json!({}),
            "PRIVATE_FX_SENTINEL",
        ),
        (
            "addWatchlistProduct",
            json!({"productListingId":fixture.product.item.product_listing_id}),
            "PRIVATE_COMMIT_SENTINEL",
        ),
    ] {
        let error = call(&registry, name, args, UserId::new())
            .await
            .unwrap_err();
        assert_eq!(
            sentinel,
            std::error::Error::source(std::error::Error::source(&error).unwrap())
                .unwrap()
                .to_string()
        );
        let response = serde_json::to_string(&error.into_protocol().unwrap()).unwrap();
        assert!(response.contains("TEMPORARILY_UNAVAILABLE"));
        assert!(!response.contains("PRIVATE"));
    }
}

#[test]
fn should_preserve_update_remove_and_nested_presentation_failure_causes() {
    let errors = [
        (
            error::remove(UnwatchProductListingError::CommitTransactionFailed(
                static_error("PRIVATE_REMOVE_SENTINEL"),
            )),
            "PRIVATE_REMOVE_SENTINEL",
        ),
        (
            error::update(
                UpdateWatchlistProductListingError::WatchlistQuotaReadFailed {
                    source: static_error("PRIVATE_UPDATE_SENTINEL"),
                },
            ),
            "PRIVATE_UPDATE_SENTINEL",
        ),
        (
            error::update(UpdateWatchlistProductListingError::Presentation(
                GetProductListingError::Shipping(
                    product_listing_service::shipping::ShippingReadError::FxRead(
                        fxrate_service::ports::FxRateSnapshotReadError::ReadFailed {
                            source: static_error("PRIVATE_SHIPPING_SENTINEL"),
                        },
                    ),
                ),
            )),
            "PRIVATE_SHIPPING_SENTINEL",
        ),
    ];
    for (error, sentinel) in errors {
        let mut cause = std::error::Error::source(&error).expect("service cause");
        while let Some(source) = cause.source() {
            cause = source;
        }
        assert_eq!(sentinel, cause.to_string());
        let response = serde_json::to_string(&error.into_protocol().unwrap()).unwrap();
        assert!(response.contains("TEMPORARILY_UNAVAILABLE"));
        assert!(!response.contains("PRIVATE"));
    }
}

#[tokio::test(start_paused = true)]
async fn should_cancel_mutations_at_the_deadline_without_retrying() {
    let fixture = Fixture::new(Outcome::Pending);
    let registry = registry(&fixture);
    let ctx = context(UserId::new());
    let future = registry.get("addWatchlistProduct").unwrap().call(
        json!({"productListingId":fixture.product.item.product_listing_id}),
        ctx,
    );
    assert!(matches!(future.await, Err(OperationError::Unavailable(_))));
    assert_eq!(1, fixture.calls.lock().unwrap().len());
}
