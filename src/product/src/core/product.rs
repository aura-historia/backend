use crate::core::description::Description;
use crate::core::heuristics;
use crate::core::product_event::domain::{
    ProductAuctionTimeChangeDomainEventPayload, ProductCreatedDomainEventPayload,
    ProductDomainEventPayload, ProductEstimatePriceChangeDomainEventPayload,
    ProductImagesChangeDomainEventPayload, ProductPriceChangeDomainEventPayload,
    ProductStateChangeDomainEventPayload, ProductUrlChangeDomainEventPayload,
};
use crate::core::product_event::enrichment::{
    EmbeddedProductEnrichmentEventPayload, ProductEnrichmentEventPayload,
    TranslationProductEnrichmentEventPayload,
};
use crate::core::product_event::lifecycle::{
    ProductDeletedLifecycleEventPayload, ProductLifecycleEventPayload,
};
use crate::core::product_event::policy::{
    ProductPolicyEventPayload, ProhibitedContentProductPolicyEventPayload,
};
use crate::core::product_event::{
    ProductDomainEvent, ProductEnrichmentEvent, ProductEvent, ProductEventPayload,
    ProductLifecycleEvent, ProductPolicyEvent,
};
use crate::core::product_image::ProductImage;
use crate::core::product_search::ProductSearch;
use crate::core::prohibited_content::{ProhibitedContent, ProhibitedContentReason};
use crate::core::title::Title;
use common::actor::domain::Actor;
use common::currency::domain::Currency;
use common::event::Event;
use common::event_id::EventId;
use common::has_key::HasKey;
use common::language::domain::Language;
use common::localized::Localized;
use common::price::domain::{FxRate, MonetaryAmount, Price};
use common::product_id::{ProductId, ProductKey};
use common::product_lifecycle::domain::ProductLifecycle;
use common::product_slug_id::ProductSlugId;
use common::product_state::domain::ProductState;
use common::seller_slug_id::SellerSlugId;
use common::shop_id::ShopId;
use common::shop_name::ShopName;
use common::shop_slug_id::ShopSlugId;
use common::shops_product_id::ShopsProductId;
use geo::core::address::{GeoAddress, StructuredAddress};
use indexmap::IndexSet;
use shop::core::shop_type::ShopType;
use std::collections::{HashMap, HashSet};
use time::OffsetDateTime;
use tracing::{error, warn};
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct Product {
    pub product_id: ProductId,
    pub product_slug_id: ProductSlugId,
    pub shop_slug_id: ShopSlugId,
    pub seller_slug_id: SellerSlugId,
    pub event_id: EventId,
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub shop_name: ShopName,
    pub seller_name: ShopName,
    pub shop_type: ShopType,
    pub structured_address: Option<StructuredAddress>,
    pub geo_address: Option<GeoAddress>,
    pub native_title: Localized<Language, Title>,
    pub other_title: HashMap<Language, Title>,
    pub native_description: Option<Localized<Language, Description>>,
    pub native_price: Option<Price>,
    pub other_price: HashMap<Currency, MonetaryAmount>,
    pub native_price_estimate_min: Option<Price>,
    pub other_price_estimate_min: HashMap<Currency, MonetaryAmount>,
    pub native_price_estimate_max: Option<Price>,
    pub other_price_estimate_max: HashMap<Currency, MonetaryAmount>,
    pub state: ProductState,
    pub lifecycle: ProductLifecycle,
    pub url: Url,
    pub view_url: Url,
    pub images: IndexSet<ProductImage>,
    pub embedding: Option<Vec<f32>>,
    pub auction_start: Option<OffsetDateTime>,
    pub auction_end: Option<OffsetDateTime>,
    pub created_by: Actor,
    pub updated_by: Actor,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

impl Product {
    #[allow(clippy::too_many_arguments)]
    pub fn create(
        shop_id: ShopId,
        seller_id: ShopId,
        shops_product_id: ShopsProductId,
        shop_name: ShopName,
        seller_name: ShopName,
        shop_type: ShopType,
        structured_address: Option<StructuredAddress>,
        geo_address: Option<GeoAddress>,
        native_title: Localized<Language, Title>,
        native_description: Option<Localized<Language, Description>>,
        native_price: Option<Price>,
        other_price: HashMap<Currency, MonetaryAmount>,
        native_price_estimate_min: Option<Price>,
        other_price_estimate_min: HashMap<Currency, MonetaryAmount>,
        native_price_estimate_max: Option<Price>,
        other_price_estimate_max: HashMap<Currency, MonetaryAmount>,
        state: ProductState,
        url: Url,
        view_url: Url,
        images: impl IntoIterator<Item = ProductImage>,
        auction_start: Option<OffsetDateTime>,
        auction_end: Option<OffsetDateTime>,
    ) -> ProductDomainEvent {
        let decision = heuristics::classify_by_text(
            native_title.payload.as_ref(),
            native_description.as_ref().map(|d| d.payload.as_ref()),
        );
        let images = images
            .into_iter()
            .map(|mut img| {
                img.prohibited_content = decision;
                img
            })
            .collect();
        let payload = ProductCreatedDomainEventPayload {
            product_slug_id: ProductSlugId::from(native_title.payload.as_ref()),
            shop_slug_id: ShopSlugId::from(shop_name.as_ref()),
            seller_slug_id: SellerSlugId::from(seller_name.as_ref()),
            shop_id,
            seller_id,
            shops_product_id,
            shop_name,
            seller_name,
            shop_type,
            structured_address,
            geo_address,
            native_title,
            native_description,
            native_price,
            other_price,
            native_price_estimate_min,
            other_price_estimate_min,
            native_price_estimate_max,
            other_price_estimate_max,
            state,
            url,
            view_url,
            images,
            auction_start,
            auction_end,
        };
        ProductDomainEvent {
            aggregate_id: ProductId::new(),
            event_id: EventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ProductDomainEventPayload::Created(payload),
        }
    }

    pub fn delete(&mut self) -> Option<ProductLifecycleEvent> {
        if self.lifecycle == ProductLifecycle::Deleted {
            None
        } else {
            let old_lifecycle = self.lifecycle;
            self.lifecycle = ProductLifecycle::Deleted;
            Some(Event {
                aggregate_id: self.product_id,
                event_id: EventId::new(),
                timestamp: OffsetDateTime::now_utc(),
                payload: ProductLifecycleEventPayload::Deleted(
                    ProductDeletedLifecycleEventPayload {
                        shop_id: self.shop_id,
                        seller_id: self.seller_id,
                        shops_product_id: self.shops_product_id.clone(),
                        old_lifecycle,
                        new_lifecycle: ProductLifecycle::Deleted,
                    },
                ),
            })
        }
    }

    pub fn change_state(&mut self, new_state: ProductState) -> Option<ProductDomainEvent> {
        if self.state == new_state {
            None
        } else {
            let old_state = self.state;
            self.state = new_state;
            Some(Event {
                aggregate_id: self.product_id,
                event_id: EventId::new(),
                timestamp: OffsetDateTime::now_utc(),
                payload: ProductDomainEventPayload::StateChanged(
                    ProductStateChangeDomainEventPayload {
                        shop_id: self.shop_id,
                        seller_id: self.seller_id,
                        shops_product_id: self.shops_product_id.clone(),
                        old_state,
                        new_state,
                    },
                ),
            })
        }
    }

    pub fn change_price(
        &mut self,
        new_native_price: Price,
        fx_rate: &impl FxRate,
    ) -> Option<ProductDomainEvent> {
        let old_native_price_opt = self.native_price;
        let old_other_price = self.other_price.clone();

        let new_other_price = fx_rate
            .exchange_all(new_native_price.currency, new_native_price.monetary_amount)
            .unwrap_or_default();
        self.native_price = Some(new_native_price);
        self.other_price = new_other_price.clone();

        match old_native_price_opt {
            None => Some(Event {
                aggregate_id: self.product_id,
                event_id: EventId::new(),
                timestamp: OffsetDateTime::now_utc(),
                payload: ProductDomainEventPayload::PriceChanged(
                    ProductPriceChangeDomainEventPayload {
                        shop_id: self.shop_id,
                        seller_id: self.seller_id,
                        shops_product_id: self.shops_product_id.clone(),
                        old_native_price: None,
                        old_other_price: HashMap::new(),
                        new_native_price: Some(new_native_price),
                        new_other_price,
                    },
                ),
            }),
            Some(old_native_price) => {
                let old_price_for_new_currency = old_native_price
                    .into_exchanged(fx_rate, new_native_price.currency)
                    .unwrap_or(old_native_price);
                if old_price_for_new_currency.monetary_amount == new_native_price.monetary_amount {
                    return None;
                }
                Some(Event {
                    aggregate_id: self.product_id,
                    event_id: EventId::new(),
                    timestamp: OffsetDateTime::now_utc(),
                    payload: ProductDomainEventPayload::PriceChanged(
                        ProductPriceChangeDomainEventPayload {
                            shop_id: self.shop_id,
                            seller_id: self.seller_id,
                            shops_product_id: self.shops_product_id.clone(),
                            old_native_price: Some(old_native_price),
                            old_other_price,
                            new_native_price: Some(new_native_price),
                            new_other_price,
                        },
                    ),
                })
            }
        }
    }

    #[deprecated(
        note = "Removing price yields little value and makes DTO-side handling of nullable but optional fields more complex."
    )]
    pub fn remove_price(&mut self) -> Option<ProductDomainEvent> {
        match self.native_price {
            Some(old_native_price) => {
                self.native_price = None;
                let old_other_price = std::mem::take(&mut self.other_price);
                Some(Event {
                    aggregate_id: self.product_id,
                    event_id: EventId::new(),
                    timestamp: OffsetDateTime::now_utc(),
                    payload: ProductDomainEventPayload::PriceChanged(
                        ProductPriceChangeDomainEventPayload {
                            shop_id: self.shop_id,
                            seller_id: self.seller_id,
                            shops_product_id: self.shops_product_id.clone(),
                            old_native_price: Some(old_native_price),
                            old_other_price,
                            new_native_price: None,
                            new_other_price: HashMap::new(),
                        },
                    ),
                })
            }
            None => None,
        }
    }

    #[allow(deprecated)]
    #[deprecated(note = "Deprecated because remove_price is deprecated.")]
    pub fn new_price(
        &mut self,
        new_price_opt: Option<Price>,
        fx_rate: &impl FxRate,
    ) -> Option<ProductDomainEvent> {
        match new_price_opt {
            Some(new_price) => self.change_price(new_price, fx_rate),
            None => self.remove_price(),
        }
    }

    pub fn change_estimate_price(
        &mut self,
        native_price_estimate_min: Option<Price>,
        native_price_estimate_max: Option<Price>,
        fx_rate: &impl FxRate,
    ) -> Option<ProductDomainEvent> {
        let mut changed = false;
        let mut min_price = None;
        let mut min_other = HashMap::new();
        let mut max_price = None;
        let mut max_other = HashMap::new();

        if let Some(new_min) = native_price_estimate_min {
            let differs = match self.native_price_estimate_min {
                Some(old) => {
                    let old_exchanged =
                        old.into_exchanged(fx_rate, new_min.currency).unwrap_or(old);
                    old_exchanged.monetary_amount != new_min.monetary_amount
                }
                None => true,
            };
            if differs {
                let other = fx_rate
                    .exchange_all(new_min.currency, new_min.monetary_amount)
                    .unwrap_or_default();
                self.native_price_estimate_min = Some(new_min);
                self.other_price_estimate_min = other.clone();
                min_price = Some(new_min);
                min_other = other;
                changed = true;
            }
        }

        if let Some(new_max) = native_price_estimate_max {
            let differs = match self.native_price_estimate_max {
                Some(old) => {
                    let old_exchanged =
                        old.into_exchanged(fx_rate, new_max.currency).unwrap_or(old);
                    old_exchanged.monetary_amount != new_max.monetary_amount
                }
                None => true,
            };
            if differs {
                let other = fx_rate
                    .exchange_all(new_max.currency, new_max.monetary_amount)
                    .unwrap_or_default();
                self.native_price_estimate_max = Some(new_max);
                self.other_price_estimate_max = other.clone();
                max_price = Some(new_max);
                max_other = other;
                changed = true;
            }
        }

        if changed {
            Some(Event {
                aggregate_id: self.product_id,
                event_id: EventId::new(),
                timestamp: OffsetDateTime::now_utc(),
                payload: ProductDomainEventPayload::EstimatePriceChanged(
                    ProductEstimatePriceChangeDomainEventPayload {
                        shop_id: self.shop_id,
                        seller_id: self.seller_id,
                        shops_product_id: self.shops_product_id.clone(),
                        native_price_estimate_min: min_price,
                        other_price_estimate_min: min_other,
                        native_price_estimate_max: max_price,
                        other_price_estimate_max: max_other,
                    },
                ),
            })
        } else {
            None
        }
    }

    pub fn change_url(&mut self, url: Url, view_url: Url) -> Option<ProductDomainEvent> {
        if self.url == url {
            return None;
        }
        self.url = url.clone();
        self.view_url = view_url.clone();
        Some(Event {
            aggregate_id: self.product_id,
            event_id: EventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ProductDomainEventPayload::UrlChanged(ProductUrlChangeDomainEventPayload {
                shop_id: self.shop_id,
                seller_id: self.seller_id,
                shops_product_id: self.shops_product_id.clone(),
                url,
                view_url,
            }),
        })
    }

    pub fn change_images(
        &mut self,
        images: impl IntoIterator<Item = ProductImage>,
    ) -> Vec<ProductEvent> {
        let existing_urls: HashSet<&Url> = self.images.iter().map(|img| &img.url).collect();

        // Text-based heuristic decision for new or previously-unclassified images.
        let decision = heuristics::classify_by_text(
            self.native_title.payload.as_ref(),
            self.native_description.as_ref().map(|d| d.payload.as_ref()),
        );

        // Build a lookup of existing image classifications by URL so that
        // already-classified images (None or NaziGermany) are not downgraded
        // to Unknown when the caller sends raw/unclassified images.
        let existing_by_url: HashMap<&Url, ProhibitedContent> = self
            .images
            .iter()
            .map(|img| (&img.url, img.prohibited_content))
            .collect();

        let new_images: IndexSet<ProductImage> = images
            .into_iter()
            .map(|mut img| {
                img.prohibited_content = match existing_by_url.get(&img.url).copied() {
                    // Preserve a previously established (non-Unknown) classification.
                    Some(pc @ (ProhibitedContent::None | ProhibitedContent::NaziGermany)) => pc,
                    // Unknown or new image: apply the text-based heuristic.
                    _ => decision,
                };
                img
            })
            .collect();

        let has_new_images = new_images
            .iter()
            .any(|img| !existing_urls.contains(&img.url));

        if self.images.len() == new_images.len() && self.images.iter().eq(new_images.iter()) {
            return vec![];
        }

        self.images = new_images.clone();

        let timestamp = OffsetDateTime::now_utc();
        let mut events: Vec<ProductEvent> = Vec::new();

        events.push(Event {
            aggregate_id: self.product_id,
            event_id: EventId::new(),
            timestamp,
            payload: ProductEventPayload::ProductDomainEvent(
                ProductDomainEventPayload::ImagesChanged(ProductImagesChangeDomainEventPayload {
                    shop_id: self.shop_id,
                    seller_id: self.seller_id,
                    shops_product_id: self.shops_product_id.clone(),
                    images: new_images,
                }),
            ),
        });

        // Emit a policy event whenever new images are added, recording the
        // heuristic classification decision (None or NaziGermany) so that
        // downstream consumers can act on it regardless of the decision outcome.
        if has_new_images {
            events.push(Event {
                aggregate_id: self.product_id,
                event_id: EventId::new(),
                timestamp,
                payload: ProductEventPayload::ProductPolicyEvent(
                    ProductPolicyEventPayload::ProhibitedContentDecision(
                        ProhibitedContentProductPolicyEventPayload {
                            shop_id: self.shop_id,
                            seller_id: self.seller_id,
                            shops_product_id: self.shops_product_id.clone(),
                            decision,
                            reason: ProhibitedContentReason::ProductText,
                        },
                    ),
                ),
            });
        }

        events
    }

    pub fn change_auction_time(
        &mut self,
        auction_start: Option<OffsetDateTime>,
        auction_end: Option<OffsetDateTime>,
    ) -> Option<ProductDomainEvent> {
        let start_changed = auction_start.is_some() && self.auction_start != auction_start;
        let end_changed = auction_end.is_some() && self.auction_end != auction_end;
        if !start_changed && !end_changed {
            return None;
        }
        if let Some(s) = auction_start {
            self.auction_start = Some(s);
        }
        if let Some(e) = auction_end {
            self.auction_end = Some(e);
        }
        Some(Event {
            aggregate_id: self.product_id,
            event_id: EventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ProductDomainEventPayload::AuctionTimeChanged(
                ProductAuctionTimeChangeDomainEventPayload {
                    shop_id: self.shop_id,
                    seller_id: self.seller_id,
                    shops_product_id: self.shops_product_id.clone(),
                    auction_start: self.auction_start,
                    auction_end: self.auction_end,
                },
            ),
        })
    }

    pub fn translate_title(
        &mut self,
        source_language: Language,
        target_language: Language,
        title: Title,
    ) -> Option<ProductEnrichmentEvent> {
        if self
            .other_title
            .get(&target_language)
            .is_some_and(|existing| existing == &title)
        {
            None
        } else {
            self.other_title.insert(target_language, title.clone());
            Some(Event {
                aggregate_id: self.product_id,
                event_id: EventId::new(),
                timestamp: OffsetDateTime::now_utc(),
                payload: ProductEnrichmentEventPayload::TranslatedTitle(
                    TranslationProductEnrichmentEventPayload {
                        shop_id: self.shop_id,
                        seller_id: self.seller_id,
                        shops_product_id: self.shops_product_id.clone(),
                        source_language,
                        target_language,
                        target: title,
                    },
                ),
            })
        }
    }

    pub fn embed(&mut self, embedding: Vec<f32>) -> Option<ProductEnrichmentEvent> {
        if self
            .embedding
            .as_ref()
            .is_some_and(|existing| existing == &embedding)
        {
            None
        } else {
            self.embedding = Some(embedding.clone());
            Some(Event {
                aggregate_id: self.product_id,
                event_id: EventId::new(),
                timestamp: OffsetDateTime::now_utc(),
                payload: ProductEnrichmentEventPayload::Embedded(
                    EmbeddedProductEnrichmentEventPayload {
                        shop_id: self.shop_id,
                        seller_id: self.seller_id,
                        shops_product_id: self.shops_product_id.clone(),
                        embedding,
                        native_title: Some(self.native_title.clone()),
                    },
                ),
            })
        }
    }

    pub fn prohibit_content(
        &mut self,
        decision: ProhibitedContent,
        reason: ProhibitedContentReason,
    ) -> Option<ProductPolicyEvent> {
        let event = Event {
            aggregate_id: self.product_id,
            event_id: EventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ProductPolicyEventPayload::ProhibitedContentDecision(
                ProhibitedContentProductPolicyEventPayload {
                    shop_id: self.shop_id,
                    seller_id: self.seller_id,
                    shops_product_id: self.shops_product_id.clone(),
                    decision,
                    reason,
                },
            ),
        };

        match decision {
            ProhibitedContent::Unknown => None,
            prohibited_content => {
                self.images = self
                    .images
                    .iter()
                    .cloned()
                    .map(|mut image| {
                        image.prohibited_content = prohibited_content;
                        image
                    })
                    .collect();
                Some(event)
            }
        }
    }

    pub fn apply(&mut self, event: ProductEvent) {
        self.event_id = event.event_id;
        self.updated = event.timestamp;

        match event.payload {
            ProductEventPayload::ProductDomainEvent(payload) => match payload {
                ProductDomainEventPayload::Created(_) => {
                    warn!("Received Created event on an existing Product. This should not happen.");
                }
                ProductDomainEventPayload::StateChanged(p) => self.state = p.new_state,
                ProductDomainEventPayload::PriceChanged(p) => {
                    self.native_price = p.new_native_price;
                    self.other_price = p.new_other_price;
                }
                ProductDomainEventPayload::EstimatePriceChanged(p) => {
                    if let Some(price) = p.native_price_estimate_min {
                        self.native_price_estimate_min = Some(price);
                        self.other_price_estimate_min = p.other_price_estimate_min;
                    }
                    if let Some(price) = p.native_price_estimate_max {
                        self.native_price_estimate_max = Some(price);
                        self.other_price_estimate_max = p.other_price_estimate_max;
                    }
                }
                ProductDomainEventPayload::UrlChanged(p) => {
                    self.url = p.url;
                }
                ProductDomainEventPayload::ImagesChanged(p) => {
                    self.images = p.images;
                }
                ProductDomainEventPayload::AuctionTimeChanged(p) => {
                    if let Some(start) = p.auction_start {
                        self.auction_start = Some(start);
                    }
                    if let Some(end) = p.auction_end {
                        self.auction_end = Some(end);
                    }
                }
            },
            ProductEventPayload::ProductEnrichmentEvent(payload) => match payload {
                ProductEnrichmentEventPayload::TranslatedTitle(p) => {
                    self.other_title.insert(p.target_language, p.target);
                }
                ProductEnrichmentEventPayload::Embedded(p) => {
                    self.embedding = Some(p.embedding);
                }
            },
            ProductEventPayload::ProductPolicyEvent(payload) => match payload {
                ProductPolicyEventPayload::ProhibitedContentDecision(p) => {
                    if p.decision != ProhibitedContent::Unknown {
                        self.images = self
                            .images
                            .iter()
                            .cloned()
                            .map(|mut image| {
                                if image.prohibited_content == ProhibitedContent::Unknown {
                                    image.prohibited_content = p.decision;
                                }
                                image
                            })
                            .collect();
                    }
                }
            },
            ProductEventPayload::ProductLifecycleEvent(payload) => match payload {
                ProductLifecycleEventPayload::Deleted(p) => {
                    self.lifecycle = p.new_lifecycle;
                }
            },
        }
    }

    pub fn localized(
        self,
        currency: &Currency,
        preferred_languages: &[Language],
    ) -> LocalizedProductView {
        let mut available_titles: HashMap<Language, Title> = self.other_title;
        available_titles
            .entry(self.native_title.localization)
            .or_insert(self.native_title.payload);

        let mut available_descriptions: HashMap<Language, Description> = HashMap::new();
        if let Some(description_native) = self.native_description {
            available_descriptions
                .entry(description_native.localization)
                .or_insert(description_native.payload);
        }

        let mut available_prices = self.other_price;
        if let Some(native_price) = self.native_price {
            available_prices
                .entry(native_price.currency)
                .or_insert(native_price.monetary_amount);
        }

        let mut available_price_estimates_min = self.other_price_estimate_min;
        if let Some(price_estimates_min) = self.native_price_estimate_min {
            available_price_estimates_min
                .entry(price_estimates_min.currency)
                .or_insert(price_estimates_min.monetary_amount);
        }

        let mut available_price_estimates_max = self.other_price_estimate_max;
        if let Some(price_estimates_max) = self.native_price_estimate_max {
            available_price_estimates_max
                .entry(price_estimates_max.currency)
                .or_insert(price_estimates_max.monetary_amount);
        }

        let title = Language::resolve(preferred_languages, available_titles).unwrap_or_else(|| {
            error!("Failed resolving title. This SHOULD be impossible because the native title always exists.");
            Localized::new(Language::En, "Unknown title".into())
        });
        let description = Language::resolve(preferred_languages, available_descriptions);
        let price = Currency::resolve(&[*currency], available_prices);
        let price_estimate_min = Currency::resolve(&[*currency], available_price_estimates_min);
        let price_estimate_max = Currency::resolve(&[*currency], available_price_estimates_max);

        LocalizedProductView {
            product_id: self.product_id,
            product_slug_id: self.product_slug_id,
            shop_slug_id: self.shop_slug_id,
            seller_slug_id: self.seller_slug_id,
            event_id: self.event_id,
            shop_id: self.shop_id,
            seller_id: self.seller_id,
            shops_product_id: self.shops_product_id,
            shop_name: self.shop_name,
            seller_name: self.seller_name,
            shop_type: self.shop_type,
            structured_address: self.structured_address,
            geo_address: self.geo_address,
            title,
            description,
            price,
            price_estimate_min,
            price_estimate_max,
            state: self.state,
            lifecycle: self.lifecycle,
            url: self.url,
            view_url: self.view_url,
            images: self.images,
            auction_start: self.auction_start,
            auction_end: self.auction_end,
            created_by: self.created_by,
            updated_by: self.updated_by,
            created: self.created,
            updated: self.updated,
        }
    }

    pub fn titles(&self) -> HashMap<Language, Title> {
        let mut titles: HashMap<Language, Title> = self.other_title.clone();
        titles
            .entry(self.native_title.localization)
            .or_insert(self.native_title.payload.clone());
        titles
    }

    pub fn descriptions(&self) -> HashMap<Language, Description> {
        let mut descriptions: HashMap<Language, Description> = HashMap::new();
        if let Some(description_native) = &self.native_description {
            descriptions
                .entry(description_native.localization)
                .or_insert(description_native.payload.clone());
        }
        descriptions
    }

    pub fn embedding_text(search: &ProductSearch) -> Option<String> {
        let mut parts: Vec<String> = search
            .product_query
            .iter()
            .map(|query| query.as_ref().trim().to_owned())
            .collect();

        if let Some(enhanced_description) = &search.enhanced_search_description {
            let enhanced_description = enhanced_description.as_ref().trim();
            if !enhanced_description.is_empty() {
                parts.push(enhanced_description.to_owned());
            }
        }

        if parts.is_empty() {
            None
        } else {
            Some(parts.join("\n"))
        }
    }
}

impl HasKey for Product {
    type Key = ProductKey;

    fn key(&self) -> Self::Key {
        ProductKey {
            shop_id: self.shop_id,
            shops_product_id: self.shops_product_id.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalizedProductView {
    pub product_id: ProductId,
    pub product_slug_id: ProductSlugId,
    pub shop_slug_id: ShopSlugId,
    pub seller_slug_id: SellerSlugId,
    pub event_id: EventId,
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub shop_name: ShopName,
    pub seller_name: ShopName,
    pub shop_type: ShopType,
    pub structured_address: Option<StructuredAddress>,
    pub geo_address: Option<GeoAddress>,
    pub title: Localized<Language, Title>,
    pub description: Option<Localized<Language, Description>>,
    pub price: Option<Price>,
    pub price_estimate_min: Option<Price>,
    pub price_estimate_max: Option<Price>,
    pub state: ProductState,
    pub lifecycle: ProductLifecycle,
    pub url: Url,
    pub view_url: Url,
    pub images: IndexSet<ProductImage>,
    pub auction_start: Option<OffsetDateTime>,
    pub auction_end: Option<OffsetDateTime>,
    pub created_by: Actor,
    pub updated_by: Actor,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use common::price::domain::FixedFxRate;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for Product {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let native_price: Option<Price> = config.fake_with_rng(rng);
            let other_price = match native_price {
                None => HashMap::new(),
                Some(price) => FixedFxRate()
                    .exchange_all(price.currency, price.monetary_amount)
                    .unwrap(),
            };
            let native_price_estimate_min: Option<Price> = config.fake_with_rng(rng);
            let other_price_estimate_min = match native_price_estimate_min {
                None => HashMap::new(),
                Some(price) => FixedFxRate()
                    .exchange_all(price.currency, price.monetary_amount)
                    .unwrap(),
            };
            let native_price_estimate_max: Option<Price> = config.fake_with_rng(rng);
            let other_price_estimate_max = match native_price_estimate_max {
                None => HashMap::new(),
                Some(price) => FixedFxRate()
                    .exchange_all(price.currency, price.monetary_amount)
                    .unwrap(),
            };
            let native_title: Localized<Language, Title> = config.fake_with_rng(rng);
            let shop_name: ShopName = config.fake_with_rng(rng);
            let seller_name: ShopName = config.fake_with_rng(rng);
            Product {
                product_id: config.fake_with_rng(rng),
                product_slug_id: ProductSlugId::from(native_title.payload.as_ref()),
                shop_slug_id: ShopSlugId::from(shop_name.as_ref()),
                seller_slug_id: SellerSlugId::from(seller_name.as_ref()),
                event_id: config.fake_with_rng(rng),
                shop_id: config.fake_with_rng(rng),
                seller_id: config.fake_with_rng(rng),
                shops_product_id: config.fake_with_rng(rng),
                shop_name,
                seller_name,
                shop_type: config.fake_with_rng(rng),
                structured_address: config.fake_with_rng(rng),
                geo_address: config.fake_with_rng(rng),
                native_title,
                other_title: config.fake_with_rng(rng),
                native_description: config.fake_with_rng(rng),
                native_price,
                other_price,
                native_price_estimate_min,
                other_price_estimate_min,
                native_price_estimate_max,
                other_price_estimate_max,
                state: config.fake_with_rng(rng),
                lifecycle: ProductLifecycle::Active,
                url: Url::parse("https://www.example.com/product").unwrap(),
                view_url: Url::parse(
                    "https://www.example.com/product?utm_source=aura_historia&utm_medium=referral",
                )
                .unwrap(),
                images: config
                    .fake_with_rng::<Vec<ProductImage>, _>(rng)
                    .into_iter()
                    .collect(),
                embedding: config.fake_with_rng(rng),
                auction_start: if config.fake_with_rng(rng) {
                    Some(OffsetDateTime::now_utc())
                } else {
                    None
                },
                auction_end: if config.fake_with_rng(rng) {
                    Some(OffsetDateTime::now_utc())
                } else {
                    None
                },
                created_by: config.fake_with_rng(rng),
                updated_by: config.fake_with_rng(rng),
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        }
    }

    impl Dummy<Faker> for LocalizedProductView {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let product: Product = config.fake_with_rng(rng);
            product.localized(&Currency::Eur, &[Language::En])
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::product_event::domain::ProductDomainEventPayload;
    use crate::core::product_event::policy::ProductPolicyEventPayload;
    use fake::{Fake, Faker};
    use url::Url;

    // -------------------------------------------------------------------------
    // Helpers
    // -------------------------------------------------------------------------

    fn make_url(path: &str) -> Url {
        Url::parse(&format!("https://example.com{path}")).unwrap()
    }

    fn img(url: &str) -> ProductImage {
        ProductImage {
            url: Url::parse(url).unwrap(),
            prohibited_content: ProhibitedContent::Unknown,
        }
    }

    fn img_with(url: &str, pc: ProhibitedContent) -> ProductImage {
        ProductImage {
            url: Url::parse(url).unwrap(),
            prohibited_content: pc,
        }
    }

    fn image_set(images: impl IntoIterator<Item = ProductImage>) -> IndexSet<ProductImage> {
        images.into_iter().collect()
    }

    #[test]
    fn should_build_embedding_text_from_query_and_enhanced_description() {
        let search = ProductSearch::new(Language::En, Currency::Eur)
            .with_product_query("antique vase".try_into().unwrap())
            .with_enhanced_search_description(
                crate::core::product_search::EnhancedSearchDescription::from(
                    "blue ceramic with floral pattern",
                ),
            );

        let actual = Product::embedding_text(&search);

        assert_eq!(
            actual.as_deref(),
            Some("antique vase\nblue ceramic with floral pattern")
        );
    }

    #[test]
    fn should_return_none_for_embedding_text_when_search_has_no_text() {
        let search = ProductSearch::new(Language::En, Currency::Eur);

        assert_eq!(Product::embedding_text(&search), None);
    }

    #[test]
    fn should_preserve_actor_metadata_when_localizing_product() {
        let product = Product {
            created_by: Actor::System,
            updated_by: Actor::User(common::user_id::UserId::new()),
            ..Faker.fake()
        };

        let localized = product.clone().localized(&Currency::Eur, &[Language::En]);

        assert_eq!(localized.created_by, product.created_by);
        assert_eq!(localized.updated_by, product.updated_by);
    }

    fn product_with_nazi_title() -> Product {
        let mut p: Product = Faker.fake();
        p.native_title = Localized::new(Language::De, Title::from("NSDAP Abzeichen 1935"));
        p.native_description = None;
        p
    }

    fn product_with_benign_title() -> Product {
        let mut p: Product = Faker.fake();
        p.native_title = Localized::new(Language::De, Title::from("Barocker Schrank Mahagoni"));
        p.native_description = None;
        p
    }

    fn create_event_for(title: &str, images: Vec<ProductImage>) -> ProductDomainEvent {
        Product::create(
            Faker.fake(),
            Faker.fake(),
            Faker.fake(),
            Faker.fake(),
            Faker.fake(),
            ShopType::AuctionHouse,
            None,
            None,
            Localized::new(Language::De, Title::from(title)),
            None,
            None,
            HashMap::new(),
            None,
            HashMap::new(),
            None,
            HashMap::new(),
            ProductState::Available,
            make_url("/item"),
            common::utm::append_utm_params(make_url("/item")),
            images,
            None,
            None,
        )
    }

    // -------------------------------------------------------------------------
    // Product::create — image classification
    // -------------------------------------------------------------------------

    mod create_image_classification {
        use super::*;

        #[test]
        fn should_classify_images_as_nazi_germany_when_title_contains_nazi_keyword() {
            let images = vec![img("https://img.example.com/a.jpg")];
            let event = create_event_for("Drittes Reich Orden", images);
            if let ProductDomainEventPayload::Created(payload) = event.payload {
                assert!(
                    payload
                        .images
                        .iter()
                        .all(|i| i.prohibited_content == ProhibitedContent::NaziGermany),
                    "all images should be NaziGermany for a nazi listing"
                );
            } else {
                panic!("expected Created event");
            }
        }

        #[test]
        fn should_classify_images_as_none_when_title_is_benign() {
            let images = vec![img("https://img.example.com/b.jpg")];
            let event = create_event_for("Antiker Stuhl 19. Jahrhundert", images);
            if let ProductDomainEventPayload::Created(payload) = event.payload {
                assert!(
                    payload
                        .images
                        .iter()
                        .all(|i| i.prohibited_content == ProhibitedContent::None),
                    "all images should be None for a benign listing"
                );
            } else {
                panic!("expected Created event");
            }
        }

        #[test]
        fn should_classify_all_images_uniformly_when_multiple_images_and_nazi_title() {
            let images = vec![
                img("https://img.example.com/1.jpg"),
                img("https://img.example.com/2.jpg"),
                img("https://img.example.com/3.jpg"),
            ];
            let event = create_event_for("Hakenkreuz Wanddeko 1938", images);
            if let ProductDomainEventPayload::Created(payload) = event.payload {
                assert_eq!(3, payload.images.len());
                assert!(
                    payload
                        .images
                        .iter()
                        .all(|i| i.prohibited_content == ProhibitedContent::NaziGermany)
                );
            } else {
                panic!("expected Created event");
            }
        }

        #[test]
        fn should_return_empty_images_in_created_event_when_no_images_provided() {
            let event = create_event_for("Waffen-SS Uniform 1943", vec![]);
            if let ProductDomainEventPayload::Created(payload) = event.payload {
                assert!(payload.images.is_empty());
            } else {
                panic!("expected Created event");
            }
        }

        #[test]
        fn should_override_any_incoming_prohibited_content_with_heuristic_decision_for_nazi() {
            // Even if the caller sets Unknown, the heuristic should classify correctly.
            let images = vec![img_with(
                "https://img.example.com/c.jpg",
                ProhibitedContent::Unknown,
            )];
            let event = create_event_for("NSDAP Abzeichen 1935", images);
            if let ProductDomainEventPayload::Created(payload) = event.payload {
                assert_eq!(
                    payload.images.get_index(0).unwrap().prohibited_content,
                    ProhibitedContent::NaziGermany
                );
            } else {
                panic!("expected Created event");
            }
        }

        #[test]
        fn should_override_any_incoming_prohibited_content_with_heuristic_decision_for_benign() {
            // Even if the caller sets Unknown, benign heuristic gives None.
            let images = vec![img_with(
                "https://img.example.com/d.jpg",
                ProhibitedContent::Unknown,
            )];
            let event = create_event_for("Antiker Schrank", images);
            if let ProductDomainEventPayload::Created(payload) = event.payload {
                assert_eq!(
                    payload.images.get_index(0).unwrap().prohibited_content,
                    ProhibitedContent::None
                );
            } else {
                panic!("expected Created event");
            }
        }
    }

    // -------------------------------------------------------------------------
    // Product::change_images — change detection
    // -------------------------------------------------------------------------

    mod change_images_detection {
        use super::*;

        #[test]
        fn should_return_empty_when_same_images_same_order() {
            let mut product = product_with_benign_title();
            product.images = image_set(vec![
                img_with("https://img.example.com/1.jpg", ProhibitedContent::None),
                img_with("https://img.example.com/2.jpg", ProhibitedContent::None),
            ]);
            let events = product.change_images(vec![
                img("https://img.example.com/1.jpg"),
                img("https://img.example.com/2.jpg"),
            ]);
            assert!(events.is_empty(), "no change expected when same URLs");
        }

        #[test]
        fn should_emit_event_when_same_images_different_order() {
            let mut product = product_with_benign_title();
            product.images = image_set(vec![
                img_with("https://img.example.com/1.jpg", ProhibitedContent::None),
                img_with("https://img.example.com/2.jpg", ProhibitedContent::None),
            ]);
            let events = product.change_images(vec![
                img("https://img.example.com/2.jpg"),
                img("https://img.example.com/1.jpg"),
            ]);
            assert!(!events.is_empty(), "reordering images should emit an event");
            assert_eq!(
                product.images.get_index(0).unwrap().url,
                Url::parse("https://img.example.com/2.jpg").unwrap()
            );
            assert_eq!(
                product.images.get_index(1).unwrap().url,
                Url::parse("https://img.example.com/1.jpg").unwrap()
            );
        }

        #[test]
        fn should_deduplicate_duplicate_images_while_preserving_first_seen_order() {
            let mut product = product_with_benign_title();
            product.images = image_set(vec![]);

            product.change_images(vec![
                img("https://img.example.com/1.jpg"),
                img("https://img.example.com/1.jpg"),
                img("https://img.example.com/2.jpg"),
                img("https://img.example.com/2.jpg"),
            ]);

            assert_eq!(2, product.images.len());
            assert_eq!(
                product.images.get_index(0).unwrap().url,
                Url::parse("https://img.example.com/1.jpg").unwrap()
            );
            assert_eq!(
                product.images.get_index(1).unwrap().url,
                Url::parse("https://img.example.com/2.jpg").unwrap()
            );
        }

        #[test]
        fn should_emit_domain_event_when_image_is_added() {
            let mut product = product_with_benign_title();
            product.images = image_set(vec![img_with(
                "https://img.example.com/existing.jpg",
                ProhibitedContent::None,
            )]);
            let events = product.change_images(vec![
                img("https://img.example.com/existing.jpg"),
                img("https://img.example.com/new.jpg"),
            ]);
            assert!(!events.is_empty(), "expected events when image is added");
            assert!(events.iter().any(|e| matches!(
                &e.payload,
                ProductEventPayload::ProductDomainEvent(ProductDomainEventPayload::ImagesChanged(
                    _
                ))
            )));
        }

        #[test]
        fn should_emit_domain_event_when_image_is_removed() {
            let mut product = product_with_benign_title();
            product.images = image_set(vec![
                img_with("https://img.example.com/1.jpg", ProhibitedContent::None),
                img_with("https://img.example.com/2.jpg", ProhibitedContent::None),
            ]);
            // Remove the second image
            let events = product.change_images(vec![img("https://img.example.com/1.jpg")]);
            assert!(!events.is_empty(), "expected event when image is removed");
        }

        #[test]
        fn should_emit_domain_event_when_all_images_replaced() {
            let mut product = product_with_benign_title();
            product.images = image_set(vec![img_with(
                "https://img.example.com/old.jpg",
                ProhibitedContent::None,
            )]);
            let events = product.change_images(vec![img("https://img.example.com/new.jpg")]);
            assert!(
                !events.is_empty(),
                "expected event when all images replaced"
            );
        }

        #[test]
        fn should_update_product_images_after_change() {
            let mut product = product_with_benign_title();
            product.images = image_set(vec![]);
            let new_url = "https://img.example.com/added.jpg";
            product.change_images(vec![img(new_url)]);
            assert_eq!(1, product.images.len());
            assert_eq!(
                product.images.get_index(0).unwrap().url,
                Url::parse(new_url).unwrap()
            );
        }
    }

    // -------------------------------------------------------------------------
    // Product::change_images — ProhibitedContent preservation
    // -------------------------------------------------------------------------

    mod change_images_preservation {
        use super::*;

        #[test]
        fn should_preserve_none_classification_for_continuing_image_when_incoming_is_unknown() {
            let mut product = product_with_nazi_title(); // Nazi title → heuristic = NaziGermany
            product.images = image_set(vec![img_with(
                "https://img.example.com/1.jpg",
                ProhibitedContent::None, // Explicitly classified as safe
            )]);
            // Update with the same URL but Unknown (as external system would send)
            let events = product.change_images(vec![img("https://img.example.com/1.jpg")]);
            assert!(events.is_empty(), "no change if same URLs");
            // The image should still be None, not overridden by Nazi heuristic
            assert_eq!(
                product.images.get_index(0).unwrap().prohibited_content,
                ProhibitedContent::None
            );
        }

        #[test]
        fn should_preserve_nazi_germany_classification_for_continuing_image_when_incoming_is_unknown()
         {
            let mut product = product_with_benign_title(); // Benign title → heuristic = None
            product.images = image_set(vec![img_with(
                "https://img.example.com/1.jpg",
                ProhibitedContent::NaziGermany, // Explicitly classified as prohibited
            )]);
            // Caller sends same URL with Unknown – benign product
            let events = product.change_images(vec![img("https://img.example.com/1.jpg")]);
            assert!(events.is_empty(), "no change if same URLs");
            // Should preserve NaziGermany, not reset to None from benign heuristic
            assert_eq!(
                product.images.get_index(0).unwrap().prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_apply_heuristic_to_new_images_when_nazi_title() {
            let mut product = product_with_nazi_title();
            product.images = image_set(vec![img_with(
                "https://img.example.com/existing.jpg",
                ProhibitedContent::None,
            )]);
            // Add a brand-new image
            let events = product.change_images(vec![
                img("https://img.example.com/existing.jpg"),
                img("https://img.example.com/new.jpg"),
            ]);
            assert!(!events.is_empty());
            // New image should be classified as NaziGermany
            let new_img = product
                .images
                .iter()
                .find(|i| i.url == Url::parse("https://img.example.com/new.jpg").unwrap())
                .expect("new image should be present");
            assert_eq!(
                new_img.prohibited_content,
                ProhibitedContent::NaziGermany,
                "new image should be classified by heuristic"
            );
            // Existing None image should be preserved
            let existing = product
                .images
                .iter()
                .find(|i| i.url == Url::parse("https://img.example.com/existing.jpg").unwrap())
                .expect("existing image should be present");
            assert_eq!(
                existing.prohibited_content,
                ProhibitedContent::None,
                "existing None classification should be preserved"
            );
        }

        #[test]
        fn should_apply_heuristic_to_new_images_when_benign_title() {
            let mut product = product_with_benign_title();
            product.images = image_set(vec![img_with(
                "https://img.example.com/existing.jpg",
                ProhibitedContent::NaziGermany,
            )]);
            // Add a brand-new image
            let events = product.change_images(vec![
                img("https://img.example.com/existing.jpg"),
                img("https://img.example.com/new.jpg"),
            ]);
            assert!(!events.is_empty());
            // New image should be classified as None (benign title)
            let new_img = product
                .images
                .iter()
                .find(|i| i.url == Url::parse("https://img.example.com/new.jpg").unwrap())
                .unwrap();
            assert_eq!(new_img.prohibited_content, ProhibitedContent::None);
            // Existing NaziGermany should be preserved
            let existing = product
                .images
                .iter()
                .find(|i| i.url == Url::parse("https://img.example.com/existing.jpg").unwrap())
                .unwrap();
            assert_eq!(existing.prohibited_content, ProhibitedContent::NaziGermany);
        }

        #[test]
        fn should_apply_heuristic_to_unknown_continuing_images() {
            let mut product = product_with_nazi_title();
            product.images = image_set(vec![img_with(
                "https://img.example.com/1.jpg",
                ProhibitedContent::Unknown, // Not yet classified
            )]);
            // Force a change by removing then re-adding (simulating a new image)
            // Actually, same URL = no change. Let's instead test with a second image
            // to force a change event, and keep the Unknown image continuing.
            let events = product.change_images(vec![
                img("https://img.example.com/1.jpg"), // continuing Unknown → apply heuristic
                img("https://img.example.com/2.jpg"), // new image
            ]);
            assert!(!events.is_empty());
            // The Unknown continuing image should be reclassified via heuristic
            let first = product
                .images
                .iter()
                .find(|i| i.url == Url::parse("https://img.example.com/1.jpg").unwrap())
                .unwrap();
            assert_eq!(
                first.prohibited_content,
                ProhibitedContent::NaziGermany,
                "Unknown continuing image should be classified by heuristic"
            );
        }
    }

    // -------------------------------------------------------------------------
    // Product::change_images — policy event emission
    // -------------------------------------------------------------------------

    mod change_images_policy_events {
        use super::*;

        #[test]
        fn should_emit_policy_event_when_nazi_content_and_new_images_added() {
            let mut product = product_with_nazi_title();
            product.images = image_set(vec![]);
            let events = product.change_images(vec![img("https://img.example.com/new.jpg")]);
            assert!(
                events.iter().any(|e| matches!(
                    &e.payload,
                    ProductEventPayload::ProductPolicyEvent(
                        ProductPolicyEventPayload::ProhibitedContentDecision(p)
                    ) if p.decision == ProhibitedContent::NaziGermany
                )),
                "expected a NaziGermany policy event when new images added to Nazi listing"
            );
        }

        #[test]
        fn should_emit_policy_event_with_none_decision_when_benign_content_and_new_images_added() {
            let mut product = product_with_benign_title();
            product.images = image_set(vec![]);
            let events = product.change_images(vec![img("https://img.example.com/new.jpg")]);
            assert!(
                events.iter().any(|e| matches!(
                    &e.payload,
                    ProductEventPayload::ProductPolicyEvent(
                        ProductPolicyEventPayload::ProhibitedContentDecision(p)
                    ) if p.decision == ProhibitedContent::None
                )),
                "expected a None policy event when new images added to benign listing"
            );
        }

        #[test]
        fn should_not_emit_policy_event_when_only_images_removed_with_nazi_title() {
            let mut product = product_with_nazi_title();
            product.images = image_set(vec![
                img_with(
                    "https://img.example.com/1.jpg",
                    ProhibitedContent::NaziGermany,
                ),
                img_with(
                    "https://img.example.com/2.jpg",
                    ProhibitedContent::NaziGermany,
                ),
            ]);
            // Remove an image – no new images added
            let events = product.change_images(vec![img_with(
                "https://img.example.com/1.jpg",
                ProhibitedContent::NaziGermany,
            )]);
            assert!(
                !events
                    .iter()
                    .any(|e| matches!(&e.payload, ProductEventPayload::ProductPolicyEvent(_))),
                "no policy event when only removing images (no new images)"
            );
        }

        #[test]
        fn should_emit_both_domain_and_policy_events_when_nazi_new_images() {
            let mut product = product_with_nazi_title();
            product.images = image_set(vec![]);
            let events = product.change_images(vec![img("https://img.example.com/new.jpg")]);
            assert_eq!(2, events.len(), "expected domain + policy event");
            assert!(events.iter().any(|e| matches!(
                &e.payload,
                ProductEventPayload::ProductDomainEvent(ProductDomainEventPayload::ImagesChanged(
                    _
                ))
            )));
            assert!(
                events
                    .iter()
                    .any(|e| matches!(&e.payload, ProductEventPayload::ProductPolicyEvent(_)))
            );
        }

        #[test]
        fn should_emit_both_domain_and_policy_events_when_benign_new_images() {
            let mut product = product_with_benign_title();
            product.images = image_set(vec![]);
            let events = product.change_images(vec![img("https://img.example.com/new.jpg")]);
            assert_eq!(2, events.len(), "expected domain + policy event");
            assert!(events.iter().any(|e| matches!(
                &e.payload,
                ProductEventPayload::ProductDomainEvent(ProductDomainEventPayload::ImagesChanged(
                    _
                ))
            )));
            assert!(events.iter().any(|e| matches!(
                &e.payload,
                ProductEventPayload::ProductPolicyEvent(
                    ProductPolicyEventPayload::ProhibitedContentDecision(p)
                ) if p.decision == ProhibitedContent::None
            )));
        }
    }

    // -------------------------------------------------------------------------
    // Product::apply — policy event replay
    // -------------------------------------------------------------------------

    mod apply_policy_event {
        use super::*;
        use crate::core::product_event::ProductEventPayload;
        use crate::core::product_event::policy::ProductPolicyEventPayload;

        fn make_policy_event(product: &Product, decision: ProhibitedContent) -> ProductEvent {
            Event {
                aggregate_id: product.product_id,
                event_id: EventId::new(),
                timestamp: OffsetDateTime::now_utc(),
                payload: ProductEventPayload::ProductPolicyEvent(
                    ProductPolicyEventPayload::ProhibitedContentDecision(
                        ProhibitedContentProductPolicyEventPayload {
                            shop_id: product.shop_id,
                            seller_id: product.seller_id,
                            shops_product_id: product.shops_product_id.clone(),
                            decision,
                            reason: ProhibitedContentReason::ProductText,
                        },
                    ),
                ),
            }
        }

        #[test]
        fn should_classify_unknown_images_when_policy_event_applied() {
            let mut product: Product = Faker.fake();
            product.images = image_set(vec![img("https://img.example.com/1.jpg")]); // Unknown

            let policy = make_policy_event(&product, ProhibitedContent::NaziGermany);
            product.apply(policy);

            assert_eq!(
                product.images.get_index(0).unwrap().prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_not_override_none_classification_when_policy_event_applied() {
            let mut product: Product = Faker.fake();
            product.images = image_set(vec![img_with(
                "https://img.example.com/1.jpg",
                ProhibitedContent::None,
            )]);

            let policy = make_policy_event(&product, ProhibitedContent::NaziGermany);
            product.apply(policy);

            assert_eq!(
                product.images.get_index(0).unwrap().prohibited_content,
                ProhibitedContent::None,
                "None classification must not be overridden by policy event"
            );
        }

        #[test]
        fn should_not_override_nazi_germany_classification_when_policy_event_applied() {
            let mut product: Product = Faker.fake();
            product.images = image_set(vec![img_with(
                "https://img.example.com/1.jpg",
                ProhibitedContent::NaziGermany,
            )]);

            // Even if a second NaziGermany policy event is applied, it's a no-op on Unknown check
            let policy = make_policy_event(&product, ProhibitedContent::NaziGermany);
            product.apply(policy);

            assert_eq!(
                product.images.get_index(0).unwrap().prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_only_update_unknown_images_when_policy_event_applied_to_mixed_images() {
            let mut product: Product = Faker.fake();
            product.images = image_set(vec![
                img_with(
                    "https://img.example.com/classified.jpg",
                    ProhibitedContent::None,
                ),
                img("https://img.example.com/unclassified.jpg"), // Unknown
                img_with(
                    "https://img.example.com/nazi.jpg",
                    ProhibitedContent::NaziGermany,
                ),
            ]);

            let policy = make_policy_event(&product, ProhibitedContent::NaziGermany);
            product.apply(policy);

            // None → unchanged
            assert_eq!(
                product.images.get_index(0).unwrap().prohibited_content,
                ProhibitedContent::None
            );
            // Unknown → NaziGermany
            assert_eq!(
                product.images.get_index(1).unwrap().prohibited_content,
                ProhibitedContent::NaziGermany
            );
            // NaziGermany → unchanged
            assert_eq!(
                product.images.get_index(2).unwrap().prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }
    }
}
