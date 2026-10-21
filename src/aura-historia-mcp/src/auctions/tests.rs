use super::error::{catalogue as catalogue_error, get as get_error, list as list_error};
use super::*;
use application::{
    operation_context::{CorrelationId, OperationContext, Principal, RequestId},
    pagination::{Cursor, CursoredResult},
};
use auction_core::{AuctionFormat, AuctionSchedule, AuctionSchedulePoint};
use auction_service::use_cases::{GetPublicAuctionError, ListAuctionsError};
use auction_service::{
    ports::{
        AuctionDirectoryCursor, AuctionDirectoryReadError, AuctionDirectoryReader,
        ListAuctionsDirectoryRequest, ListAuctionsDirectoryResult, PublicAuctionDetails,
        PublicAuctionDirectoryItem, PublicAuctionDirectorySourceSummary,
        PublicAuctionSourceSummary,
    },
    use_cases::{GetPublicAuctionResult, GetPublicAuctionUseCase, ListAuctionsHandler},
};
use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
use product_listing_service::use_cases::GetAuctionCatalogueError;
use product_listing_service::{
    ports::AuctionCatalogueCursor,
    use_cases::{
        GetAuctionCatalogueRequest, GetAuctionCatalogueUseCase,
        PersonalizedProductListingDetailsView,
    },
};
use serde_json::{Value, json};
use std::sync::Mutex;
use time::OffsetDateTime;

type Calls<Request> = Arc<Mutex<Vec<(OperationContext, Request)>>>;

#[derive(Clone)]
struct Directory;
#[async_trait::async_trait]
impl AuctionDirectoryReader for Directory {
    async fn list(
        &self,
        request: &ListAuctionsDirectoryRequest,
    ) -> Result<ListAuctionsDirectoryResult, AuctionDirectoryReadError> {
        let scope = request.scope();
        let cursor = request.cursor.clone().unwrap_or_default();
        let auction_id = AuctionId::new();
        Ok(CursoredResult {
            items: vec![PublicAuctionDirectoryItem {
                auction_id,
                source: PublicAuctionDirectorySourceSummary {
                    listing_source_id: request.listing_source_id.unwrap_or_default(),
                    slug_id: ListingSourceSlugId::raw("source").unwrap(),
                    name: ListingSourceName::try_from("Source").unwrap(),
                },
                name: None,
                format: request.format,
                schedule: AuctionSchedule::default(),
                reported_status: request.reported_status,
                created: OffsetDateTime::UNIX_EPOCH,
            }],
            cursor: Cursor {
                size: cursor.size,
                search_after: Some(AuctionDirectoryCursor {
                    created: OffsetDateTime::UNIX_EPOCH,
                    auction_id,
                    scheduled: None,
                    scope,
                }),
            },
            total: None,
        })
    }
}
#[derive(Clone, Default)]
struct List {
    calls: Calls<ListAuctionsDirectoryRequest>,
}
#[async_trait::async_trait]
impl auction_service::use_cases::ListAuctionsUseCase for List {
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListAuctionsDirectoryRequest,
    ) -> Result<ListAuctionsDirectoryResult, ListAuctionsError> {
        self.calls
            .lock()
            .unwrap()
            .push((context.clone(), request.clone()));
        ListAuctionsHandler::new(Directory)
            .execute(context, request)
            .await
    }
}
#[derive(Clone, Default)]
struct Get {
    calls: Calls<AuctionId>,
}
#[async_trait::async_trait]
impl GetPublicAuctionUseCase for Get {
    async fn execute(
        &self,
        context: &OperationContext,
        auction_id: AuctionId,
    ) -> Result<GetPublicAuctionResult, GetPublicAuctionError> {
        self.calls
            .lock()
            .unwrap()
            .push((context.clone(), auction_id));
        Ok(PublicAuctionDetails {
            auction_id,
            source: PublicAuctionSourceSummary {
                listing_source_id: ListingSourceId::new(),
                name: ListingSourceName::try_from("Source").unwrap(),
                slug_id: ListingSourceSlugId::raw("source").unwrap(),
                referral_configuration: None,
            },
            name: None,
            catalogue_url: Some(url::Url::parse("https://example.test/catalogue").unwrap()),
            format: Some(AuctionFormat::Timed),
            schedule: AuctionSchedule::default(),
            reported_status: None,
            reported_lot_count: None,
            visible_active_assigned_listing_count: 0,
        })
    }
}
#[derive(Clone, Default)]
struct Catalogue {
    calls: Calls<GetAuctionCatalogueRequest>,
}
#[async_trait::async_trait]
impl GetAuctionCatalogueUseCase for Catalogue {
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetAuctionCatalogueRequest,
    ) -> Result<
        CursoredResult<PersonalizedProductListingDetailsView, AuctionCatalogueCursor>,
        GetAuctionCatalogueError,
    > {
        self.calls
            .lock()
            .unwrap()
            .push((context.clone(), request.clone()));
        let cursor = request.cursor.unwrap_or_default();
        if cursor
            .search_after
            .is_some_and(|c| c.auction_id != request.auction_id)
        {
            return Err(GetAuctionCatalogueError::CursorScopeMismatch);
        }
        Ok(CursoredResult {
            items: vec![listing()],
            cursor: Cursor {
                size: cursor.size,
                search_after: Some(AuctionCatalogueCursor {
                    auction_id: request.auction_id,
                    catalogue_position: Some(7),
                    product_listing_id:
                        product_listing_core::product_listing_id::ProductListingId::new(),
                }),
            },
            total: None,
        })
    }
}
fn listing() -> PersonalizedProductListingDetailsView {
    use product_listing_core::{
        listing_lifecycle::ListingLifecycle, product_listing::ProductListingPricing,
    };
    use product_listing_service::use_cases::{
        DisplayProductListingPricing, ProductListingDetailsView, ProductListingImageView,
        ProductListingPricingPresentation, ProductListingPricingValuation,
    };
    application::personalized::Personalized {
        item: ProductListingDetailsView {
            shipping: Some(advertised_shipping()),
            product_listing_id: product_listing_core::product_listing_id::ProductListingId::new(),
            product_listing_title_slug_id: None,
            event_id: domain_primitives::event_id::EventId::new(),
            source: product_listing_service::ports::ListingSourceSummary {
                listing_source_id: ListingSourceId::new(),
                name: ListingSourceName::try_from("Source").unwrap(),
                slug_id: ListingSourceSlugId::raw("source").unwrap(),
            },
            source_listing_id: product_listing_core::source_listing_id::SourceListingId::try_from(
                "lot-1",
            )
            .unwrap(),
            product_title: None,
            product_description: None,
            title: Some(localization::Localized::new(
                localization::Language::De,
                product_listing_core::title::Title::from("Schrank"),
            )),
            description: None,
            pricing: ProductListingPricingPresentation {
                source: ProductListingPricing::default(),
                display: DisplayProductListingPricing {
                    price: Some(
                        product_listing_core::product_listing_price::ProductListingPrice::Monetary(
                            money::Price::new(1234_u64.into(), money::Currency::Usd),
                        ),
                    ),
                    price_estimate_min: None,
                    price_estimate_max: None,
                },
                valuation: ProductListingPricingValuation::Current {
                    fx_rate_id: fxrate_core::FxRateId::new(),
                    captured_at: OffsetDateTime::UNIX_EPOCH,
                },
            },
            availability: None,
            lifecycle: ListingLifecycle::Active,
            url: url::Url::parse("https://example.test/lot").unwrap(),
            view_url: url::Url::parse("https://example.test/lot?ref=aura").unwrap(),
            images: vec![ProductListingImageView { url: None }],
            content_policy: None,
            auction: None,
            lot: None,
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
        },
        user_state: None,
    }
}
fn advertised_shipping() -> product_listing_service::shipping::AdvertisedShipping {
    let fixture = shipping_core::fixtures::gallery();
    product_listing_service::shipping::AdvertisedShipping {
        display: None,
        effective: shipping_core::assessment::EffectiveDeliveryTerms::resolve(
            &fixture.painting,
            Some(&fixture.source_default),
            std::slice::from_ref(&fixture.profile),
        ),
        shipping_reasons: vec![],
        collection_reasons: vec![],
        listing_revision: u64::MAX,
        source_revision: 2,
        default_revision: None,
        profile: None,
        listing_evidence: None,
        geographic_valid_until: None,
        evaluated_at: OffsetDateTime::UNIX_EPOCH,
        notices: vec![],
    }
}
fn services() -> (PublicAuctions, List, Get, Catalogue) {
    let list = List::default();
    let get = Get::default();
    let catalogue = Catalogue::default();
    (
        PublicAuctions {
            get: Arc::new(get.clone()),
            list: Arc::new(list.clone()),
            catalogue: Arc::new(catalogue.clone()),
        },
        list,
        get,
        catalogue,
    )
}
fn context() -> crate::InvocationContext {
    crate::InvocationContext {
        operation: OperationContext {
            principal: Principal::Anonymous,
            request_id: RequestId::new("auction-test"),
            correlation_id: CorrelationId::new("correlation-test"),
        },
        deadline: tokio::time::Instant::now() + std::time::Duration::from_secs(5),
        cancellation: tokio_util::sync::CancellationToken::new(),
        max_response_bytes: crate::MAX_RESPONSE_BYTES,
    }
}
async fn call(
    ops: &[OperationDefinition],
    name: &str,
    args: Value,
) -> Result<Value, OperationError> {
    let op = ops.iter().find(|op| op.name() == name).unwrap();
    assert!(
        jsonschema::is_valid(&json!(op.tool().input_schema), &args),
        "{args}"
    );
    let result = op.call(args, context()).await?;
    let value = result.structured_content.unwrap();
    assert!(
        jsonschema::is_valid(&json!(op.tool().output_schema), &value),
        "{value}"
    );
    Ok(value)
}
#[tokio::test]
async fn should_register_exact_anonymous_read_operations_and_complete_input_schemas() {
    let (services, _, _, _) = services();
    let ops = operations(services).unwrap();
    assert_eq!(
        vec!["listAuctions", "getAuction", "getAuctionCatalogue"],
        ops.iter()
            .map(OperationDefinition::name)
            .collect::<Vec<_>>()
    );
    for op in &ops {
        assert_eq!(AccessClass::Public, op.policy().access);
        assert!(op.policy().required_scopes.is_empty());
        assert!(op.policy().read_only);
        assert!(op.policy().idempotent);
        assert!(!op.policy().destructive);
    }
    for (name, fields) in [
        (
            "listAuctions",
            vec![
                "listingSourceId",
                "format",
                "reportedStatus",
                "timeRole",
                "sort",
                "order",
                "from",
                "to",
                "pageSize",
                "searchAfter",
            ],
        ),
        ("getAuction", vec!["auctionId"]),
        (
            "getAuctionCatalogue",
            vec![
                "auctionId",
                "language",
                "currency",
                "pageSize",
                "searchAfter",
            ],
        ),
    ] {
        let schema = json!(
            ops.iter()
                .find(|op| op.name() == name)
                .unwrap()
                .tool()
                .input_schema
        );
        for field in fields {
            assert!(schema["properties"].get(field).is_some(), "{name}: {field}");
        }
    }
}
#[tokio::test]
async fn should_map_all_directory_filters_and_continue_scheduled_and_created_order() {
    let (services, list, _, _) = services();
    let ops = operations(services).unwrap();
    let source = ListingSourceId::new();
    for sort in ["created", "scheduled"] {
        let mut args = json!({
            "listingSourceId": source,
            "format": "TIMED",
            "reportedStatus": "SCHEDULED",
            "sort": sort,
            "order": "desc",
            "timeRole": "LIVE_STARTS",
            "from": "2026-10-01T00:00:00Z",
            "to": "2026-11-01T00:00:00Z",
            "pageSize": 31,
        });
        let first = call(&ops, "listAuctions", args.clone()).await.unwrap();
        assert_eq!(31, first["pageSize"]);
        assert_eq!(
            source.to_string(),
            first["items"][0]["listingSource"]["listingSourceId"]
        );
        args["searchAfter"] = Value::String(first["searchAfter"].to_string());
        call(&ops, "listAuctions", args.clone()).await.unwrap();
        args["listingSourceId"] = json!(ListingSourceId::new());
        assert!(matches!(
            call(&ops, "listAuctions", args).await,
            Err(OperationError::InvalidArguments(_))
        ));
    }
    let requests = list.calls.lock().unwrap();
    let (ctx, req) = &requests[0];
    assert!(matches!(ctx.principal, Principal::Anonymous));
    assert_eq!("auction-test", ctx.request_id.as_str());
    assert_eq!(Some(source), req.listing_source_id);
    assert_eq!(Some(AuctionFormat::Timed), req.format);
    assert_eq!(None, req.sort);
    assert_eq!(
        Some(AuctionSchedulePoint::LiveStarts),
        req.schedule.as_ref().map(|s| s.role)
    );
    assert_eq!(Some(AuctionSchedulePoint::LiveStarts), requests[3].1.sort);
}
#[tokio::test]
async fn should_reject_invalid_sort_schedule_identifiers_and_cursors_explicitly() {
    let (services, _, _, _) = services();
    let ops = operations(services).unwrap();
    let op = ops.iter().find(|op| op.name() == "listAuctions").unwrap();
    for args in [
        json!({"sort":"scheduled"}),
        json!({"sort":"unexpected"}),
        json!({"order":"up"}),
        json!({"timeRole":"LIVE_STARTS"}),
        json!({"from":"2026-10-01T00:00:00Z"}),
        json!({"timeRole":"LIVE_STARTS","from":"2026-11-01T00:00:00Z","to":"2026-10-01T00:00:00Z"}),
        json!({"searchAfter":"invalid"}),
        json!({"format":"timed"}),
        json!({"listingSourceId":"invalid"}),
    ] {
        assert!(
            matches!(
                op.call(args.clone(), context()).await,
                Err(OperationError::InvalidArguments(_))
            ),
            "{args}"
        );
    }
}
#[tokio::test]
async fn should_return_safe_detail_and_pass_catalogue_presentation_and_continuation() {
    let (services, _, get, catalogue) = services();
    let ops = operations(services).unwrap();
    let auction = AuctionId::new();
    let detail = call(&ops, "getAuction", json!({"auctionId":auction}))
        .await
        .unwrap();
    assert_eq!(auction.to_string(), detail["auctionId"]);
    assert!(detail.get("sourceAuctionId").is_none());
    assert!(detail.get("expectedVersion").is_none());
    assert!(
        detail["viewUrl"]
            .as_str()
            .unwrap()
            .contains("utm_source=aura_historia")
    );
    assert_eq!(auction, get.calls.lock().unwrap()[0].1);
    let first = call(
        &ops,
        "getAuctionCatalogue",
        json!({"auctionId":auction,"language":"de-DE","currency":"USD","pageSize":17}),
    )
    .await
    .unwrap();
    assert_eq!(17, first["pageSize"]);
    let item = &first["items"][0];
    assert!(item.get("userState").is_none());
    assert_eq!("Schrank", item["item"]["title"]["text"]);
    assert_eq!("de", item["item"]["title"]["language"]);
    assert!(item["item"]["images"][0]["url"].is_null());
    let shipping = &item["item"]["shipping"];
    assert_eq!(u64::MAX, shipping["listingRevision"]);
    assert!(shipping["terms"]["shipping"]["coverage"]["dispatch_origin"].is_null());
    assert!(shipping["terms"]["collection"]["site"].is_null());
    assert_eq!(
        "USD",
        item["item"]["pricing"]["display"]["price"]["currency"]
    );
    call(
        &ops,
        "getAuctionCatalogue",
        json!({
            "auctionId": auction,
            "language": "de",
            "currency": "USD",
            "pageSize": 17,
            "searchAfter": first["searchAfter"].to_string(),
        }),
    )
    .await
    .unwrap();
    {
        let requests = catalogue.calls.lock().unwrap();
        assert!(matches!(requests[0].0.principal, Principal::Anonymous));
        assert_eq!(localization::Language::De, requests[0].1.language);
        assert_eq!(money::Currency::Usd, requests[0].1.currency);
        assert_eq!(
            Some(7),
            requests[1]
                .1
                .cursor
                .as_ref()
                .unwrap()
                .search_after
                .unwrap()
                .catalogue_position
        );
    }
    assert!(matches!(
        call(
            &ops,
            "getAuctionCatalogue",
            json!({"auctionId":AuctionId::new(),"searchAfter":first["searchAfter"].to_string()})
        )
        .await,
        Err(OperationError::InvalidArguments(_))
    ));
}
#[test]
fn should_classify_application_failures_without_losing_technical_causes() {
    assert!(matches!(
        get_error(GetPublicAuctionError::NotFound),
        OperationError::NotFound
    ));
    assert!(matches!(
        catalogue_error(GetAuctionCatalogueError::AuctionNotFound),
        OperationError::NotFound
    ));
    assert!(matches!(
        catalogue_error(GetAuctionCatalogueError::PricingFxSnapshotMissing),
        OperationError::Unavailable(_)
    ));
    assert!(matches!(
        catalogue_error(GetAuctionCatalogueError::CursorScopeMismatch),
        OperationError::InvalidArguments(_)
    ));
    let error = get_error(GetPublicAuctionError::TemporarilyUnavailable {
        source: box_error(std::io::Error::other("private database detail")),
    });
    assert!(matches!(error, OperationError::Unavailable(_)));
    assert!(!error.to_string().contains("private"));
    assert!(std::error::Error::source(&error).is_some());
    assert!(matches!(
        list_error(ListAuctionsError::InvalidReadModel {
            source: box_error(std::io::Error::other("private"))
        }),
        OperationError::Internal(_)
    ));
}

#[tokio::test]
async fn should_bound_all_auction_calls_by_cancellation_and_deadline() {
    let (services, list, get, catalogue) = services();
    let ops = operations(services).unwrap();
    for expired in [false, true] {
        for op in &ops {
            let mut invocation = context();
            if expired {
                invocation.deadline =
                    tokio::time::Instant::now() - std::time::Duration::from_secs(1);
            } else {
                invocation.cancellation.cancel();
            }
            let args = if op.name() == "listAuctions" {
                json!({})
            } else {
                json!({"auctionId": AuctionId::new()})
            };
            assert!(matches!(
                op.call(args, invocation).await,
                Err(OperationError::Unavailable(_))
            ));
        }
    }
    assert!(list.calls.lock().unwrap().is_empty());
    assert!(get.calls.lock().unwrap().is_empty());
    assert!(catalogue.calls.lock().unwrap().is_empty());
}
