//! Deterministic inbound-service fixtures; no HTTP, database or provider calls.
use super::super::PublicServices;
use application::{
    operation_context::{OperationContext, Principal},
    pagination::{Cursor, CursoredResult},
    personalized::Personalized,
};
use async_trait::async_trait;
use auction_core::{AuctionFormat, AuctionId, AuctionSchedule};
use auction_service::{ports::*, use_cases::*};
use aura_historia_runtime::*;
use fxrate_core::FxRateId;
use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
use listing_source_service::use_cases::queries::{
    get_public_listing_source_by_slug::*, public_listing_source::*,
    search_public_listing_sources::*,
};
use localization::{Language, Localized};
use money::{Currency, MonetaryAmount, Price};
use party_core::party_name::PartyName;
use party_service::discovery::*;
use product_listing_core::{
    listing_availability::ListingAvailability, listing_lifecycle::ListingLifecycle,
    product_listing::ProductListingPricing, product_listing_id::ProductListingId,
    product_listing_price::ProductListingPrice, source_listing_id::SourceListingId, title::Title,
};
use product_listing_service::{
    ports::{AuctionCatalogueCursor, ListingSourceSummary},
    use_cases::*,
};
use serde_json::json;
use std::sync::{Arc, Mutex};
use time::{Duration, OffsetDateTime};

#[derive(Default)]
pub enum SearchOutcome {
    #[default]
    Pages,
    Unavailable,
}

pub struct Fixture {
    pub calls: Mutex<Vec<&'static str>>,
    pub searches: Mutex<Vec<SearchProductListingsRequest>>,
    pub history_count: usize,
    pub detail_images: usize,
    pub url_padding: usize,
    pub search_outcome: SearchOutcome,
}
impl Default for Fixture {
    fn default() -> Self {
        Self {
            calls: Mutex::new(vec![]),
            searches: Mutex::new(vec![]),
            history_count: 128,
            detail_images: 175,
            url_padding: 512,
            search_outcome: SearchOutcome::Pages,
        }
    }
}
impl Fixture {
    pub fn services(self: &Arc<Self>) -> PublicServices {
        PublicServices {
            listings: PublicProductListings {
                get: self.clone(),
                search: self.clone(),
                similar: self.clone(),
            },
            history: PublicProductListingHistory { get: self.clone() },
            auctions: PublicAuctions {
                list: self.clone(),
                get: self.clone(),
                catalogue: self.clone(),
            },
            sources: PublicListingSources {
                search: self.clone(),
                get_by_slug: self.clone(),
            },
            source_budget: PublicListingSourceReadBudget::default(),
            locations: PublicPartyLocations {
                discover: self.clone(),
            },
            overview: PublicOverview { get: self.clone() },
        }
    }
    fn record(&self, context: &OperationContext, operation: &'static str) {
        assert!(matches!(context.principal, Principal::Anonymous));
        self.calls.lock().unwrap().push(operation);
    }
    pub fn images(&self, count: usize) -> Vec<ProductListingImageView> {
        (0..count)
            .map(|index| ProductListingImageView {
                url: Some(
                    format!(
                        "https://images.example.test/{index}.jpg?signature={}",
                        "a".repeat(self.url_padding)
                    )
                    .parse()
                    .unwrap(),
                ),
            })
            .collect()
    }
}
#[async_trait]
impl SearchProductListingsUseCase for Fixture {
    async fn execute(
        &self,
        context: &OperationContext,
        request: SearchProductListingsRequest,
    ) -> Result<SearchProductListingsResult, SearchProductListingsError> {
        self.record(context, "simpleSearchProductListings");
        self.searches.lock().unwrap().push(request.clone());
        if matches!(self.search_outcome, SearchOutcome::Unavailable) {
            return Err(SearchProductListingsError::QueryEmbeddingFailed {
                source: application::error::static_error("PRIVATE_PROVIDER_SENTINEL"),
            });
        }
        let continuation = request
            .cursor
            .as_ref()
            .and_then(|cursor| cursor.search_after.as_ref());
        let count = if continuation.is_none() { 100 } else { 1 };
        Ok(CursoredResult {
            items: (0..count)
                .map(|_| {
                    let mut item = summary();
                    item.item.images = self.images(3);
                    item.item.content_policy = None;
                    item
                })
                .collect(),
            total: Some(101),
            cursor: Cursor {
                size: count,
                search_after: continuation.is_none().then(|| ProductListingSearchCursor {
                    fx_rate_id: "fx_01k6k0w000e008000000000005".parse().unwrap(),
                    search_after: json!([u64::MAX, "pl_01k6k0w000e008000000000006"]),
                }),
            },
        })
    }
}
#[async_trait]
impl GetProductListingUseCase for Fixture {
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetProductListingRequest,
    ) -> Result<PersonalizedProductListingDetailsView, GetProductListingError> {
        let name = match request.lookup {
            ProductListingLookup::ById(_) => "getProductListing",
            ProductListingLookup::ByTitleSlug(_) => "getProductListingByTitleSlug",
        };
        self.record(context, name);
        let mut view = details_view();
        view.item.images = self.images(self.detail_images);
        view.item.content_policy = None;
        Ok(view)
    }
}
#[async_trait]
impl GetSimilarProductListingsUseCase for Fixture {
    async fn execute(
        &self,
        context: &OperationContext,
        _: GetSimilarProductListingsRequest,
    ) -> Result<GetSimilarProductListingsResult, GetSimilarProductListingsError> {
        self.record(context, "getSimilarProductListings");
        Ok(GetSimilarProductListingsResult::EmbeddingPending)
    }
}
#[async_trait]
impl GetProductListingHistoryUseCase for Fixture {
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetProductListingHistoryRequest,
    ) -> Result<Vec<ProductListingHistoryEntry>, GetProductListingHistoryError> {
        self.record(context, "getProductListingHistory");
        let ProductListingHistoryLookup::ById(id) = request.lookup else {
            panic!("unexpected lookup")
        };
        Ok(entries(id, self.history_count))
    }
}
#[async_trait]
impl ListAuctionsUseCase for Fixture {
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListAuctionsDirectoryRequest,
    ) -> Result<ListAuctionsDirectoryResult, ListAuctionsError> {
        self.record(context, "listAuctions");
        Ok(CursoredResult {
            items: vec![],
            total: None,
            cursor: request.cursor.unwrap_or_default(),
        })
    }
}
#[async_trait]
impl GetPublicAuctionUseCase for Fixture {
    async fn execute(
        &self,
        context: &OperationContext,
        auction_id: AuctionId,
    ) -> Result<GetPublicAuctionResult, GetPublicAuctionError> {
        self.record(context, "getAuction");
        Ok(PublicAuctionDetails {
            auction_id,
            source: PublicAuctionSourceSummary {
                listing_source_id: ListingSourceId::new(),
                name: ListingSourceName::try_from("Source").unwrap(),
                slug_id: ListingSourceSlugId::raw("source").unwrap(),
                referral_configuration: None,
            },
            name: None,
            catalogue_url: Some("https://source.example.test/catalogue".parse().unwrap()),
            format: Some(AuctionFormat::Timed),
            schedule: AuctionSchedule::default(),
            reported_status: None,
            reported_lot_count: Some(auction_core::ReportedCatalogueLotCount::new(101)),
            visible_active_assigned_listing_count: 101,
        })
    }
}
#[async_trait]
impl GetAuctionCatalogueUseCase for Fixture {
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetAuctionCatalogueRequest,
    ) -> Result<
        CursoredResult<PersonalizedProductListingDetailsView, AuctionCatalogueCursor>,
        GetAuctionCatalogueError,
    > {
        self.record(context, "getAuctionCatalogue");
        Ok(CursoredResult {
            items: vec![details_view()],
            total: None,
            cursor: Cursor {
                size: 1,
                search_after: Some(AuctionCatalogueCursor {
                    auction_id: request.auction_id,
                    catalogue_position: Some(7),
                    product_listing_id: ProductListingId::new(),
                }),
            },
        })
    }
}
#[async_trait]
impl SearchPublicListingSourcesUseCase for Fixture {
    async fn execute(
        &self,
        context: &OperationContext,
        request: SearchPublicListingSourcesRequest,
    ) -> Result<SearchPublicListingSourcesResult, SearchPublicListingSourcesError> {
        self.record(context, "searchPublicListingSources");
        Ok(SearchPublicListingSourcesResult {
            items: vec![public_source()],
            page_size: request.page_size(),
            continuation: None,
        })
    }
}
#[async_trait]
impl GetPublicListingSourceBySlugUseCase for Fixture {
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetPublicListingSourceBySlugRequest,
    ) -> Result<GetPublicListingSourceBySlugResult, GetPublicListingSourceBySlugError> {
        self.record(context, "getPublicListingSourceBySlug");
        Ok(PublicListingSourceSummary {
            listing_source_slug_id: request.slug_id,
            ..public_source()
        })
    }
}
#[async_trait]
impl DiscoverPartyLocationsUseCase for Fixture {
    async fn execute(
        &self,
        request: DiscoverPartyLocationsRequest,
    ) -> Result<DiscoverPartyLocationsResult, DiscoverPartyLocationsError> {
        self.calls.lock().unwrap().push("discoverPartyLocations");
        Ok(CursoredResult {
            items: vec![],
            total: None,
            cursor: request.cursor,
        })
    }
}
#[async_trait]
impl GetPublicCatalogueOverviewUseCase for Fixture {
    async fn execute(
        &self,
        context: &OperationContext,
    ) -> Result<PublicCatalogueOverview, GetPublicCatalogueOverviewError> {
        self.record(context, "getPublicOverview");
        Ok(PublicCatalogueOverview {
            as_of: OffsetDateTime::UNIX_EPOCH,
            sources_indexed: 12,
            live_objects: 101,
            countries: 7,
            languages: 5,
            new_objects_last_7_days: 89,
        })
    }
}
fn public_source() -> PublicListingSourceSummary {
    PublicListingSourceSummary {
        listing_source_id: ListingSourceId::new(),
        listing_source_slug_id: ListingSourceSlugId::raw("source").unwrap(),
        name: ListingSourceName::try_from("Source").unwrap(),
        operator: PublicListingSourceOperatorSummary {
            name: PartyName::try_from("Source Operator").unwrap(),
        },
        url: Some("https://source.example.test".parse().unwrap()),
        image: None,
    }
}
fn source() -> ListingSourceSummary {
    ListingSourceSummary {
        listing_source_id: "ls_01k6k0w000e008000000000002".parse().unwrap(),
        name: ListingSourceName::try_from("Source").unwrap(),
        slug_id: ListingSourceSlugId::raw("source").unwrap(),
    }
}
fn details_view() -> PersonalizedProductListingDetailsView {
    Personalized {
        item: ProductListingDetailsView {
            shipping: None,
            product_listing_id: "pl_01k6k0w000e008000000000001".parse().unwrap(),
            product_listing_title_slug_id: None,
            event_id: "evt_01k6k0w000e008000000000007".parse().unwrap(),
            source: source(),
            source_listing_id: SourceListingId::try_from("merchant:local/42").unwrap(),
            product_title: None,
            product_description: None,
            title: Some(Localized::new(Language::En, Title::from("Cabinet"))),
            description: None,
            pricing: ProductListingPricingPresentation {
                source: ProductListingPricing {
                    price: Some(Price::new(MonetaryAmount::from(u64::MAX), Currency::Eur).into()),
                    price_estimate_min: None,
                    price_estimate_max: None,
                },
                display: DisplayProductListingPricing {
                    price: Some(Price::new(MonetaryAmount::from(u64::MAX), Currency::Jpy).into()),
                    price_estimate_min: None,
                    price_estimate_max: None,
                },
                valuation: ProductListingPricingValuation::Current {
                    fx_rate_id: "fx_01k6k0w000e008000000000005".parse().unwrap(),
                    captured_at: OffsetDateTime::UNIX_EPOCH,
                },
            },
            availability: Some(ListingAvailability::InStock),
            lifecycle: ListingLifecycle::Active,
            url: "https://source.example/product/42?ref=aura"
                .parse()
                .unwrap(),
            view_url: "https://aura.example/product-listings/pl_01k6k0w000e008000000000001"
                .parse()
                .unwrap(),
            images: vec![ProductListingImageView { url: None }],
            content_policy: Some(
                product_listing_core::content_policy::ContentPolicyDecision::RequiresConsent(
                    product_listing_core::content_policy::SensitiveContentCategory::NaziGermany,
                ),
            ),
            auction: None,
            lot: None,
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
        },
        user_state: None,
    }
}
fn summary() -> PersonalizedProductListingSummary {
    let view = details_view().item;
    Personalized {
        item: ProductListingSummary {
            shipping: None,
            geography: None,
            product_listing_id: view.product_listing_id,
            product_listing_title_slug_id: view.product_listing_title_slug_id,
            event_id: view.event_id,
            source: view.source,
            source_listing_id: view.source_listing_id,
            auction_id: None,
            title: view.title,
            display_price: view.pricing.display.price,
            price_valuation: ProductListingSummaryPriceValuation::Current {
                fx_rate_id: "fx_01k6k0w000e008000000000005".parse().unwrap(),
                captured_at: OffsetDateTime::UNIX_EPOCH,
            },
            availability: view.availability,
            lifecycle: view.lifecycle,
            url: view.url,
            view_url: view.view_url,
            images: view.images,
            content_policy: view.content_policy,
            updated: view.updated,
        },
        user_state: None,
    }
}

fn entries(product_listing_id: ProductListingId, count: usize) -> Vec<ProductListingHistoryEntry> {
    let first_fx = FxRateId::new();
    let later_fx = FxRateId::new();
    (0..count)
        .map(|index| {
            let captured_at =
                OffsetDateTime::UNIX_EPOCH + Duration::seconds(if index < 2 { -60 } else { 1 });
            let price = ProductListingHistoryPrice {
                source: ProductListingPrice::Monetary(Price::new(
                    MonetaryAmount::from(100_u64),
                    Currency::Eur,
                )),
                display: Some(ProductListingHistoryPriceDisplay {
                    price: Price::new(
                        MonetaryAmount::from(if index < 2 { 200_u64 } else { 400_u64 }),
                        Currency::Usd,
                    ),
                    fx_rate_id: if index < 2 { first_fx } else { later_fx },
                    captured_at,
                }),
            };
            ProductListingHistoryEntry {
                product_listing_id,
                event_id: Default::default(),
                occurred_at: OffsetDateTime::UNIX_EPOCH + Duration::seconds((index / 2) as i64),
                kind: if index == 0 {
                    ProductListingHistoryEntryKind::Discovered(Box::new(
                        ProductListingDiscoveryHistory {
                            listing_source_id: ListingSourceId::new(),
                            source_listing_id: SourceListingId::try_from("source-42").unwrap(),
                            title: None,
                            description: None,
                            pricing: ProductListingHistoryPricing {
                                price: Some(price),
                                ..Default::default()
                            },
                            availability: None,
                            url: url::Url::parse("https://example.test/listing").unwrap(),
                            image_count: 2,
                            auction: None,
                        },
                    ))
                } else {
                    ProductListingHistoryEntryKind::Changed(
                        vec![ProductListingHistoryChange::MainPriceChanged {
                            previous: None,
                            current: Some(price),
                        }]
                        .try_into()
                        .unwrap(),
                    )
                },
            }
        })
        .collect()
}
