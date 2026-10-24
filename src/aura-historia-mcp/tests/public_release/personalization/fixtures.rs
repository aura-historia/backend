//! Real listing and shipping handlers with deterministic outbound-port fixtures.
use application::{
    pagination::{Cursor, CursoredResult},
    personalized::Personalized,
};
use aura_historia_runtime::PublicProductListings;
use fxrate_core::{
    FX_RATE_SCALE, FxRateGeneration, FxRateId, FxRateQuote, FxRateSnapshot, FxRateSource,
};
use listing_source_core::{ListingSourceName, ListingSourceSlugId};
use localization::{Language, Localized};
use money::{Currency, Price};
use product_listing_core::{
    content_policy::{ContentPolicyDecision, SensitiveContentCategory},
    listing_availability::ListingAvailability,
    listing_lifecycle::ListingLifecycle,
    product_listing::ProductListingPricing,
    product_listing_id::ProductListingId,
    product_listing_image::ProductListingImage,
    product_listing_slug_id::ProductListingSlugId,
    source_listing_id::SourceListingId,
    title::Title,
};
use product_listing_service::{
    ports::*, shipping::ShippingEnrichment, use_cases::*, user_state::*,
};
use std::sync::{Arc, atomic::AtomicUsize};
use time::OffsetDateTime;
use user_core::user_id::UserId;

pub(super) const FILTER_ID: &str = "sf_01k6k0w000e008000000000008";
pub(super) const NOTIFICATION_ID: &str = "ntf_01k6k0w000e008000000000009";

#[derive(Clone)]
pub(super) struct ListingFixture {
    pub user_id: UserId,
    pub hidden: bool,
    pub consent: bool,
    pub destination_reads: Arc<AtomicUsize>,
}
impl ListingFixture {
    pub fn new(hidden: bool, consent: bool) -> Self {
        Self {
            user_id: UserId::new(),
            hidden,
            consent,
            destination_reads: Arc::default(),
        }
    }
    pub fn source(&self) -> ListingSourceSummary {
        ListingSourceSummary {
            listing_source_id: "ls_01k6k0w000e008000000000002".parse().unwrap(),
            name: ListingSourceName::try_from("Source").unwrap(),
            slug_id: ListingSourceSlugId::raw("source").unwrap(),
        }
    }
    pub fn id(&self) -> ProductListingId {
        super::super::LISTING_ID.parse().unwrap()
    }
    pub fn slug(&self) -> ProductListingSlugId {
        ProductListingSlugId::raw("cabinet-abcdef").unwrap()
    }
    pub fn state(&self) -> ProductListingUserState {
        ProductListingUserState {
            watchlist: WatchlistUserState {
                watching: true,
                notifications: true,
            },
            notification: NotificationUserState {
                unseen_notification_ids: vec![NOTIFICATION_ID.parse().unwrap()],
            },
            content_visibility: ContentVisibilityUserState {
                show_unassessed_or_sensitive_content: self.consent,
            },
            search_filter: SearchFilterUserState {
                matched: true,
                hidden: self.hidden,
                user_search_filter_id: Some(FILTER_ID.parse().unwrap()),
                user_search_filter_name: Some("Cabinets".to_owned().into()),
                match_feedback: Some(true),
                shipping_match_valuation: None,
            },
            shipping: None,
        }
    }
    pub fn details(&self, user: Option<UserId>) -> PersonalizedProductListingDetailsReadModel {
        if let Some(user) = user {
            assert_eq!(self.user_id, user);
        }
        Personalized {
            item: ProductListingDetailsReadModel {
                product_listing_id: self.id(),
                product_listing_title_slug_id: self.slug(),
                event_id: "evt_01k6k0w000e008000000000007".parse().unwrap(),
                source: self.source(),
                source_listing_id: SourceListingId::try_from("merchant:42").unwrap(),
                product_title: None,
                product_description: None,
                title: Some(Localized::new(Language::En, Title::from("Cabinet"))),
                description: None,
                pricing: ProductListingPricing {
                    price: Some(Price::new(125_u64.into(), Currency::Eur).into()),
                    ..Default::default()
                },
                sale_observation: None,
                availability: Some(ListingAvailability::InStock),
                lifecycle: ListingLifecycle::Active,
                url: "https://source.example.test/listing/42".parse().unwrap(),
                view_url: "https://source.example.test/listing/42".parse().unwrap(),
                images: [ProductListingImage::new(
                    "https://images.example.test/sensitive.jpg".parse().unwrap(),
                )]
                .into_iter()
                .collect(),
                content_policy: Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany,
                )),
                auction: None,
                lot: None,
                created: OffsetDateTime::UNIX_EPOCH,
                updated: OffsetDateTime::UNIX_EPOCH,
            },
            user_state: user.map(|_| self.state()),
        }
    }
    pub fn search_item(&self) -> ProductListingSearchItem {
        let item = self.details(None).item;
        ProductListingSearchItem {
            geography: None,
            product_listing_id: item.product_listing_id,
            product_listing_title_slug_id: item.product_listing_title_slug_id,
            event_id: item.event_id,
            listing_source_id: item.source.listing_source_id,
            source_listing_id: item.source_listing_id,
            auction_id: None,
            title: item.title,
            display_price: item.pricing.price,
            price_valuation: ProductListingSummaryPriceValuation::Current {
                fx_rate_id: self.fx_snapshot().id(),
                captured_at: OffsetDateTime::UNIX_EPOCH,
            },
            availability: item.availability,
            lifecycle: item.lifecycle,
            url: item.url,
            images: item.images,
            updated: item.updated,
        }
    }
    pub fn page<T>(&self, item: T) -> CursoredResult<T, AuctionCatalogueCursor> {
        CursoredResult {
            items: vec![item],
            total: Some(1),
            cursor: Cursor {
                size: 1,
                search_after: Some(AuctionCatalogueCursor {
                    auction_id: super::super::AUCTION_ID.parse().unwrap(),
                    catalogue_position: Some(7),
                    product_listing_id: self.id(),
                }),
            },
        }
    }
    pub fn fx_snapshot(&self) -> FxRateSnapshot {
        let currencies = [
            Currency::Eur,
            Currency::Gbp,
            Currency::Usd,
            Currency::Aud,
            Currency::Cad,
            Currency::Nzd,
            Currency::Cny,
            Currency::Brl,
            Currency::Pln,
            Currency::Try,
            Currency::Jpy,
            Currency::Czk,
            Currency::Rub,
            Currency::Aed,
            Currency::Sar,
            Currency::Hkd,
            Currency::Sgd,
            Currency::Chf,
            Currency::Zar,
        ];
        FxRateSnapshot::rehydrate(
            "fx_01k6k0w000e008000000000005".parse::<FxRateId>().unwrap(),
            FxRateGeneration::try_from(1).unwrap(),
            OffsetDateTime::UNIX_EPOCH,
            FxRateSource::FxRatesApi,
            currencies
                .into_iter()
                .map(|currency| FxRateQuote::new(currency, FX_RATE_SCALE)),
        )
        .unwrap()
    }
    fn shipping(&self) -> ShippingEnrichment {
        ShippingEnrichment::new(
            Arc::new(shipping_service::assessment::AssessDeliveryHandler::new(
                self.clone(),
                self.clone(),
                Arc::new(self.clone()),
            )),
            Arc::new(self.clone()),
        )
    }
    pub fn listings(&self) -> PublicProductListings {
        PublicProductListings {
            get: Arc::new(GetProductListingHandler::new(
                self.clone(),
                self.clone(),
                self.clone(),
                self.shipping(),
            )),
            search: Arc::new(SearchProductListingsHandler::new(
                self.clone(),
                self.clone(),
                self.clone(),
                self.clone(),
                self.clone(),
                self.clone(),
                self.shipping(),
            )),
            similar: Arc::new(GetSimilarProductListingsHandler::new(
                self.clone(),
                self.clone(),
                self.clone(),
                self.clone(),
                self.clone(),
                self.clone(),
                self.clone(),
                self.shipping(),
            )),
        }
    }
    pub fn catalogue(&self) -> Arc<dyn GetAuctionCatalogueUseCase> {
        Arc::new(GetAuctionCatalogueHandler::new(
            self.clone(),
            self.clone(),
            self.clone(),
            self.clone(),
            self.shipping(),
        ))
    }
}
