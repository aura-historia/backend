use crate::core::authenticity::Authenticity;
use crate::core::condition::Condition;
use crate::core::description::Description;
use crate::core::origin_year::OriginYear;
use crate::core::product_event::domain::{
    ProductAuctionTimeChangeDomainEventPayload, ProductAuthenticityChangeDomainEventPayload,
    ProductConditionChangeDomainEventPayload, ProductCreatedDomainEventPayload,
    ProductDomainEventPayload, ProductEstimatePriceChangeDomainEventPayload,
    ProductImagesChangeDomainEventPayload, ProductOriginYearChangeDomainEventPayload,
    ProductPriceChangeDomainEventPayload, ProductProvenanceChangeDomainEventPayload,
    ProductRestorationChangeDomainEventPayload, ProductStateChangeDomainEventPayload,
    ProductUrlChangeDomainEventPayload,
};
use crate::core::product_event::enrichment::{
    EmbeddedProductEnrichmentEventPayload, ExtractedAttributesProductEnrichmentEventPayload,
    ProductEnrichmentEventPayload, TranslationProductEnrichmentEventPayload,
};
use crate::core::product_event::policy::{
    ProductPolicyEventPayload, ProhibitedContentProductPolicyEventPayload,
};
use crate::core::product_event::{
    ProductDomainEvent, ProductEnrichmentEvent, ProductEvent, ProductEventPayload,
    ProductPolicyEvent,
};
use crate::core::product_image::ProductImage;
use crate::core::prohibited_content::{ProhibitedContent, ProhibitedContentReason};
use crate::core::provenance::Provenance;
use crate::core::restoration::Restoration;
use crate::core::title::Title;
use common::category_key::CategoryId;
use common::currency::domain::Currency;
use common::event::Event;
use common::event_id::EventId;
use common::has_key::HasKey;
use common::language::domain::Language;
use common::localized::Localized;
use common::period_key::PeriodId;
use common::price::domain::{FxRate, MonetaryAmount, Price};
use common::product_id::{ProductId, ProductKey};
use common::product_state::domain::ProductState;
use common::shop_id::ShopId;
use common::shop_name::ShopName;
use common::shops_product_id::ShopsProductId;
use common::slug_id::SlugId;
use common::string_newtype;
use common::year::YearRange;
use shop::core::shop_type::ShopType;
use std::collections::HashMap;
use time::OffsetDateTime;
use tracing::{error, warn};
use url::Url;

string_newtype!(ProductCategory);
string_newtype!(ProductPeriod);

#[derive(Debug, Clone, PartialEq)]
pub struct Product {
    pub product_id: ProductId,
    pub product_slug_id: SlugId<6>,
    pub shop_slug_id: SlugId<0>,
    pub event_id: EventId,
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub shop_name: ShopName,
    pub shop_type: ShopType,
    pub category_id: Option<CategoryId>,
    pub category_name: HashMap<Language, ProductCategory>,
    pub period_id: Option<PeriodId>,
    pub period_name: HashMap<Language, ProductPeriod>,
    pub native_title: Localized<Language, Title>,
    pub other_title: HashMap<Language, Title>,
    pub native_description: Option<Localized<Language, Description>>,
    pub other_description: HashMap<Language, Description>,
    pub native_price: Option<Price>,
    pub other_price: HashMap<Currency, MonetaryAmount>,
    pub native_price_estimate_min: Option<Price>,
    pub other_price_estimate_min: HashMap<Currency, MonetaryAmount>,
    pub native_price_estimate_max: Option<Price>,
    pub other_price_estimate_max: HashMap<Currency, MonetaryAmount>,
    pub state: ProductState,
    pub url: Url,
    pub images: Vec<ProductImage>,
    pub embedding: Option<Vec<f32>>,
    pub origin_year: Option<OriginYear>,
    pub authenticity: Authenticity,
    pub condition: Condition,
    pub provenance: Provenance,
    pub restoration: Restoration,
    pub auction_start: Option<OffsetDateTime>,
    pub auction_end: Option<OffsetDateTime>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

impl Product {
    #[allow(clippy::too_many_arguments)]
    pub fn create(
        shop_id: ShopId,
        shops_product_id: ShopsProductId,
        shop_name: ShopName,
        shop_type: ShopType,
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
        images: Vec<ProductImage>,
        auction_start: Option<OffsetDateTime>,
        auction_end: Option<OffsetDateTime>,
    ) -> ProductDomainEvent {
        let payload = ProductCreatedDomainEventPayload {
            product_slug_id: SlugId::from(native_title.payload.as_ref()),
            shop_slug_id: SlugId::from(shop_name.as_ref()),
            shop_id,
            shops_product_id,
            shop_name,
            shop_type,
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

    pub fn change_state(&mut self, new_state: ProductState) -> Option<ProductDomainEvent> {
        if self.state == new_state {
            None
        } else {
            let old_state = self.state;
            self.state = new_state;
            let event = Event {
                aggregate_id: self.product_id,
                event_id: EventId::new(),
                timestamp: OffsetDateTime::now_utc(),
                payload: ProductDomainEventPayload::StateChanged(
                    ProductStateChangeDomainEventPayload {
                        shop_id: self.shop_id,
                        shops_product_id: self.shops_product_id.clone(),
                        old_state,
                        new_state,
                    },
                ),
            };
            Some(event)
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
            None => {
                let event = Event {
                    aggregate_id: self.product_id,
                    event_id: EventId::new(),
                    timestamp: OffsetDateTime::now_utc(),
                    payload: ProductDomainEventPayload::PriceChanged(
                        ProductPriceChangeDomainEventPayload {
                            shop_id: self.shop_id,
                            shops_product_id: self.shops_product_id.clone(),
                            old_native_price: None,
                            old_other_price: HashMap::new(),
                            new_native_price: Some(new_native_price),
                            new_other_price,
                        },
                    ),
                };
                Some(event)
            }
            Some(old_native_price) => {
                let old_price_for_new_currency = old_native_price
                    .into_exchanged(fx_rate, new_native_price.currency)
                    .unwrap_or(old_native_price);
                if old_price_for_new_currency.monetary_amount == new_native_price.monetary_amount {
                    return None;
                }
                let event = Event {
                    aggregate_id: self.product_id,
                    event_id: EventId::new(),
                    timestamp: OffsetDateTime::now_utc(),
                    payload: ProductDomainEventPayload::PriceChanged(
                        ProductPriceChangeDomainEventPayload {
                            shop_id: self.shop_id,
                            shops_product_id: self.shops_product_id.clone(),
                            old_native_price: Some(old_native_price),
                            old_other_price,
                            new_native_price: Some(new_native_price),
                            new_other_price,
                        },
                    ),
                };
                Some(event)
            }
        }
    }

    pub fn remove_price(&mut self) -> Option<ProductDomainEvent> {
        match self.native_price {
            Some(old_native_price) => {
                self.native_price = None;
                let old_other_price = self.other_price.drain().collect();
                let event = Event {
                    aggregate_id: self.product_id,
                    event_id: EventId::new(),
                    timestamp: OffsetDateTime::now_utc(),
                    payload: ProductDomainEventPayload::PriceChanged(
                        ProductPriceChangeDomainEventPayload {
                            shop_id: self.shop_id,
                            shops_product_id: self.shops_product_id.clone(),
                            old_native_price: Some(old_native_price),
                            old_other_price,
                            new_native_price: None,
                            new_other_price: HashMap::new(),
                        },
                    ),
                };
                Some(event)
            }
            None => None,
        }
    }

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

    pub fn change_url(&mut self, url: Url) -> Option<ProductDomainEvent> {
        if self.url == url {
            return None;
        }
        self.url = url.clone();
        Some(Event {
            aggregate_id: self.product_id,
            event_id: EventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ProductDomainEventPayload::UrlChanged(ProductUrlChangeDomainEventPayload {
                shop_id: self.shop_id,
                shops_product_id: self.shops_product_id.clone(),
                url,
            }),
        })
    }

    pub fn change_images(&mut self, images: Vec<ProductImage>) -> Option<ProductDomainEvent> {
        if self.images == images {
            return None;
        }
        self.images = images.clone();
        Some(Event {
            aggregate_id: self.product_id,
            event_id: EventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ProductDomainEventPayload::ImagesChanged(
                ProductImagesChangeDomainEventPayload {
                    shop_id: self.shop_id,
                    shops_product_id: self.shops_product_id.clone(),
                    images,
                },
            ),
        })
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
                    shops_product_id: self.shops_product_id.clone(),
                    auction_start: self.auction_start,
                    auction_end: self.auction_end,
                },
            ),
        })
    }

    pub fn change_origin_year(&mut self, origin_year: OriginYear) -> Option<ProductDomainEvent> {
        if self.origin_year == Some(origin_year) {
            return None;
        }
        self.origin_year = Some(origin_year);
        Some(Event {
            aggregate_id: self.product_id,
            event_id: EventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ProductDomainEventPayload::OriginYearChanged(
                ProductOriginYearChangeDomainEventPayload {
                    shop_id: self.shop_id,
                    shops_product_id: self.shops_product_id.clone(),
                    origin_year,
                },
            ),
        })
    }

    pub fn change_authenticity(
        &mut self,
        authenticity: Authenticity,
    ) -> Option<ProductDomainEvent> {
        if self.authenticity == authenticity {
            return None;
        }
        self.authenticity = authenticity;
        Some(Event {
            aggregate_id: self.product_id,
            event_id: EventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ProductDomainEventPayload::AuthenticityChanged(
                ProductAuthenticityChangeDomainEventPayload {
                    shop_id: self.shop_id,
                    shops_product_id: self.shops_product_id.clone(),
                    authenticity,
                },
            ),
        })
    }

    pub fn change_condition(&mut self, condition: Condition) -> Option<ProductDomainEvent> {
        if self.condition == condition {
            return None;
        }
        self.condition = condition;
        Some(Event {
            aggregate_id: self.product_id,
            event_id: EventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ProductDomainEventPayload::ConditionChanged(
                ProductConditionChangeDomainEventPayload {
                    shop_id: self.shop_id,
                    shops_product_id: self.shops_product_id.clone(),
                    condition,
                },
            ),
        })
    }

    pub fn change_provenance(&mut self, provenance: Provenance) -> Option<ProductDomainEvent> {
        if self.provenance == provenance {
            return None;
        }
        self.provenance = provenance;
        Some(Event {
            aggregate_id: self.product_id,
            event_id: EventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ProductDomainEventPayload::ProvenanceChanged(
                ProductProvenanceChangeDomainEventPayload {
                    shop_id: self.shop_id,
                    shops_product_id: self.shops_product_id.clone(),
                    provenance,
                },
            ),
        })
    }

    pub fn change_restoration(&mut self, restoration: Restoration) -> Option<ProductDomainEvent> {
        if self.restoration == restoration {
            return None;
        }
        self.restoration = restoration;
        Some(Event {
            aggregate_id: self.product_id,
            event_id: EventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ProductDomainEventPayload::RestorationChanged(
                ProductRestorationChangeDomainEventPayload {
                    shop_id: self.shop_id,
                    shops_product_id: self.shops_product_id.clone(),
                    restoration,
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
            let event_payload = ProductEnrichmentEventPayload::TranslatedTitle(
                TranslationProductEnrichmentEventPayload {
                    shop_id: self.shop_id,
                    shops_product_id: self.shops_product_id.clone(),
                    source_language,
                    target_language,
                    target: title,
                },
            );
            let event = Event {
                aggregate_id: self.product_id,
                event_id: EventId::new(),
                timestamp: OffsetDateTime::now_utc(),
                payload: event_payload,
            };

            Some(event)
        }
    }

    pub fn translate_description(
        &mut self,
        source_language: Language,
        target_language: Language,
        description: Description,
    ) -> Option<ProductEnrichmentEvent> {
        if self
            .other_description
            .get(&target_language)
            .is_some_and(|existing| existing == &description)
        {
            None
        } else {
            self.other_description
                .insert(target_language, description.clone());
            let event_payload = ProductEnrichmentEventPayload::TranslatedDescription(
                TranslationProductEnrichmentEventPayload {
                    shop_id: self.shop_id,
                    shops_product_id: self.shops_product_id.clone(),
                    source_language,
                    target_language,
                    target: description,
                },
            );
            let event = Event {
                aggregate_id: self.product_id,
                event_id: EventId::new(),
                timestamp: OffsetDateTime::now_utc(),
                payload: event_payload,
            };

            Some(event)
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
            let event_payload =
                ProductEnrichmentEventPayload::Embedded(EmbeddedProductEnrichmentEventPayload {
                    shop_id: self.shop_id,
                    shops_product_id: self.shops_product_id.clone(),
                    embedding,
                });
            let event = Event {
                aggregate_id: self.product_id,
                event_id: EventId::new(),
                timestamp: OffsetDateTime::now_utc(),
                payload: event_payload,
            };

            Some(event)
        }
    }

    pub fn extract_attributes(
        &mut self,
        origin_year: Option<OriginYear>,
        authenticity: Option<Authenticity>,
        condition: Option<Condition>,
        provenance: Option<Provenance>,
        restoration: Option<Restoration>,
    ) -> Option<ProductEnrichmentEvent> {
        let mut event_payload = ExtractedAttributesProductEnrichmentEventPayload {
            shop_id: self.shop_id,
            shops_product_id: self.shops_product_id.clone(),
            origin_year_min: None,
            origin_year: None,
            origin_year_max: None,
            authenticity: None,
            condition: None,
            provenance: None,
            restoration: None,
        };
        if self.origin_year != origin_year && origin_year.is_some() {
            event_payload.origin_year_min = origin_year.and_then(|oy| oy.min());
            event_payload.origin_year = origin_year.and_then(|oy| oy.exact());
            event_payload.origin_year_max = origin_year.and_then(|oy| oy.max());
            self.origin_year = origin_year;
        }
        if authenticity.is_some_and(|new| new != self.authenticity) {
            event_payload.authenticity = authenticity;
            self.authenticity = authenticity.expect("shouldn't fail because is_some");
        }
        if condition.is_some_and(|new| new != self.condition) {
            event_payload.condition = condition;
            self.condition = condition.expect("shouldn't fail because is_some");
        }
        if provenance.is_some_and(|new| new != self.provenance) {
            event_payload.provenance = provenance;
            self.provenance = provenance.expect("shouldn't fail because is_some");
        }
        if restoration.is_some_and(|new| new != self.restoration) {
            event_payload.restoration = restoration;
            self.restoration = restoration.expect("shouldn't fail because is_some");
        }

        let event = Event {
            aggregate_id: self.product_id,
            event_id: EventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ProductEnrichmentEventPayload::ExtractedAttributes(event_payload),
        };

        Some(event)
    }

    pub fn prohibit_content(
        &mut self,
        decision: ProhibitedContent,
        reason: ProhibitedContentReason,
    ) -> Option<ProductPolicyEvent> {
        let event_payload = ProductPolicyEventPayload::ProhibitedContentDecision(
            ProhibitedContentProductPolicyEventPayload {
                shop_id: self.shop_id,
                shops_product_id: self.shops_product_id.clone(),
                decision,
                reason,
            },
        );
        let event = Event {
            aggregate_id: self.product_id,
            event_id: EventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            payload: event_payload,
        };

        match decision {
            ProhibitedContent::Unknown => None,
            prohibted_content => {
                for image in &mut self.images {
                    image.prohibited_content = prohibted_content;
                }
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
                ProductDomainEventPayload::OriginYearChanged(p) => {
                    self.origin_year = Some(p.origin_year);
                }
                ProductDomainEventPayload::AuthenticityChanged(p) => {
                    self.authenticity = p.authenticity;
                }
                ProductDomainEventPayload::ConditionChanged(p) => {
                    self.condition = p.condition;
                }
                ProductDomainEventPayload::ProvenanceChanged(p) => {
                    self.provenance = p.provenance;
                }
                ProductDomainEventPayload::RestorationChanged(p) => {
                    self.restoration = p.restoration;
                }
            },
            ProductEventPayload::ProductEnrichmentEvent(payload) => match payload {
                ProductEnrichmentEventPayload::TranslatedTitle(p) => {
                    self.other_title.insert(p.target_language, p.target);
                }
                ProductEnrichmentEventPayload::TranslatedDescription(p) => {
                    self.other_description.insert(p.target_language, p.target);
                }
                ProductEnrichmentEventPayload::Embedded(p) => {
                    self.embedding = Some(p.embedding);
                }
                ProductEnrichmentEventPayload::ExtractedAttributes(p) => {
                    if let Some(exact) = p.origin_year {
                        self.origin_year = Some(OriginYear::ExactYear(exact));
                    } else if p.origin_year_min.is_some() || p.origin_year_max.is_some() {
                        self.origin_year = Some(OriginYear::EstimatedRange(YearRange {
                            min: p.origin_year_min,
                            max: p.origin_year_max,
                        }));
                    }
                    if let Some(authenticity) = p.authenticity {
                        self.authenticity = authenticity;
                    }
                    if let Some(condition) = p.condition {
                        self.condition = condition;
                    }
                    if let Some(provenance) = p.provenance {
                        self.provenance = provenance;
                    }
                    if let Some(restoration) = p.restoration {
                        self.restoration = restoration;
                    }
                }
                ProductEnrichmentEventPayload::ClassifiedCategory(p) => {
                    self.category_id = Some(p.category_id);
                }
                ProductEnrichmentEventPayload::ClassifiedPeriod(p) => {
                    self.period_id = Some(p.period_id);
                }
            },
            ProductEventPayload::ProductPolicyEvent(payload) => match payload {
                ProductPolicyEventPayload::ProhibitedContentDecision(p) => {
                    if p.decision != ProhibitedContent::Unknown {
                        for image in &mut self.images {
                            image.prohibited_content = p.decision;
                        }
                    }
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

        let mut available_descriptions: HashMap<Language, Description> = self.other_description;
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
            event_id: self.event_id,
            shop_id: self.shop_id,
            shops_product_id: self.shops_product_id,
            shop_name: self.shop_name,
            shop_type: self.shop_type,
            category_id: self.category_id,
            category_name: Language::resolve(preferred_languages, self.category_name),
            period_id: self.period_id,
            period_name: Language::resolve(preferred_languages, self.period_name),
            title,
            description,
            price,
            price_estimate_min,
            price_estimate_max,
            state: self.state,
            url: self.url,
            images: self.images,
            origin_year: self.origin_year,
            authenticity: self.authenticity,
            condition: self.condition,
            provenance: self.provenance,
            restoration: self.restoration,
            auction_start: self.auction_start,
            auction_end: self.auction_end,
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
        let mut descriptions: HashMap<Language, Description> = self.other_description.clone();
        if let Some(description_native) = &self.native_description {
            descriptions
                .entry(description_native.localization)
                .or_insert(description_native.payload.clone());
        }
        descriptions
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
    pub product_slug_id: SlugId<6>,
    pub shop_slug_id: SlugId<0>,
    pub event_id: EventId,
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub shop_name: ShopName,
    pub shop_type: ShopType,
    pub category_id: Option<CategoryId>,
    pub category_name: Option<Localized<Language, ProductCategory>>,
    pub period_id: Option<PeriodId>,
    pub period_name: Option<Localized<Language, ProductPeriod>>,
    pub title: Localized<Language, Title>,
    pub description: Option<Localized<Language, Description>>,
    pub price: Option<Price>,
    pub price_estimate_min: Option<Price>,
    pub price_estimate_max: Option<Price>,
    pub state: ProductState,
    pub url: Url,
    pub images: Vec<ProductImage>,
    pub origin_year: Option<OriginYear>,
    pub authenticity: Authenticity,
    pub condition: Condition,
    pub provenance: Provenance,
    pub restoration: Restoration,
    pub auction_start: Option<OffsetDateTime>,
    pub auction_end: Option<OffsetDateTime>,
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
            let state = config.fake_with_rng(rng);
            let native_title: Localized<Language, Title> = config.fake_with_rng(rng);
            let shop_name: ShopName = config.fake_with_rng(rng);
            Product {
                product_slug_id: SlugId::from(native_title.payload.as_ref()),
                shop_slug_id: SlugId::from(shop_name.as_ref()),
                product_id: config.fake_with_rng(rng),
                event_id: config.fake_with_rng(rng),
                shop_id: config.fake_with_rng(rng),
                shops_product_id: config.fake_with_rng(rng),
                shop_name,
                shop_type: config.fake_with_rng(rng),
                category_id: config.fake_with_rng(rng),
                category_name: config.fake_with_rng(rng),
                period_id: config.fake_with_rng(rng),
                period_name: config.fake_with_rng(rng),
                native_title,
                other_title: config.fake_with_rng(rng),
                native_description: config.fake_with_rng(rng),
                other_description: config.fake_with_rng(rng),
                native_price,
                other_price,
                native_price_estimate_min,
                other_price_estimate_min,
                native_price_estimate_max,
                other_price_estimate_max,
                state,
                url: Url::parse(&format!(
                    "https://foo.bar/item/{}",
                    config.fake_with_rng::<u16, _>(rng)
                ))
                .unwrap(),
                images: config.fake_with_rng(rng),
                embedding: Some(fake::vec![f32; 768]),
                origin_year: config.fake_with_rng(rng),
                authenticity: config.fake_with_rng(rng),
                condition: config.fake_with_rng(rng),
                provenance: config.fake_with_rng(rng),
                restoration: config.fake_with_rng(rng),
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
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        }
    }

    impl Dummy<Faker> for LocalizedProductView {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let title: Localized<Language, Title> = config.fake_with_rng(rng);
            let shop_name: ShopName = config.fake_with_rng(rng);
            LocalizedProductView {
                product_slug_id: SlugId::from(title.payload.as_ref()),
                shop_slug_id: SlugId::from(shop_name.as_ref()),
                product_id: config.fake_with_rng(rng),
                event_id: config.fake_with_rng(rng),
                shop_id: config.fake_with_rng(rng),
                shops_product_id: config.fake_with_rng(rng),
                shop_name,
                shop_type: config.fake_with_rng(rng),
                category_id: config.fake_with_rng(rng),
                category_name: config.fake_with_rng(rng),
                period_id: config.fake_with_rng(rng),
                period_name: config.fake_with_rng(rng),
                title,
                description: config.fake_with_rng(rng),
                price: config.fake_with_rng(rng),
                price_estimate_min: config.fake_with_rng(rng),
                price_estimate_max: config.fake_with_rng(rng),
                state: config.fake_with_rng(rng),
                url: Url::parse(&format!(
                    "https://foo.bar/item/{}",
                    config.fake_with_rng::<u16, _>(rng)
                ))
                .unwrap(),
                images: config.fake_with_rng(rng),
                origin_year: config.fake_with_rng(rng),
                authenticity: config.fake_with_rng(rng),
                condition: config.fake_with_rng(rng),
                provenance: config.fake_with_rng(rng),
                restoration: config.fake_with_rng(rng),
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
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::core::product::{LocalizedProductView, Product};
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_product() {
            let _ = Faker.fake::<Product>();
        }

        #[test]
        fn should_fake_localized_product_view() {
            let _ = Faker.fake::<LocalizedProductView>();
        }
    }
}

#[cfg(test)]
mod tests {
    mod state {
        use crate::core::product::Product;
        use common::language::domain::Language;
        use common::localized::Localized;
        use common::product_state::domain::ProductState;
        use fake::{Fake, Faker};
        use rstest;
        use time::OffsetDateTime;
        use url::Url;

        #[rstest::rstest]
        #[case::listed(ProductState::Listed, ProductState::Listed)]
        #[case::available(ProductState::Available, ProductState::Available)]
        #[case::reserved(ProductState::Reserved, ProductState::Reserved)]
        #[case::sold(ProductState::Sold, ProductState::Sold)]
        #[case::removed(ProductState::Removed, ProductState::Removed)]
        #[case::unknown(ProductState::Unknown, ProductState::Unknown)]
        #[trace]
        fn should_return_none_when_state_did_not_change_for_change_state(
            #[case] from_state: ProductState,
            #[case] to_state: ProductState,
        ) {
            let mut product = Product {
                product_id: Default::default(),
                product_slug_id: Faker.fake(),
                shop_slug_id: Faker.fake(),
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_product_id: Default::default(),
                shop_name: "Boop".into(),
                shop_type: fake::Faker.fake(),
                category_id: Faker.fake(),
                category_name: Faker.fake(),
                period_id: Faker.fake(),
                period_name: Faker.fake(),
                native_title: Localized {
                    localization: Language::De,
                    payload: "Boop".into(),
                },
                other_title: Default::default(),
                native_description: None,
                other_description: Default::default(),
                native_price: None,
                other_price: Default::default(),
                native_price_estimate_min: None,
                other_price_estimate_min: Default::default(),
                native_price_estimate_max: None,
                other_price_estimate_max: Default::default(),
                state: from_state,
                url: Url::parse("https://example.com").unwrap(),
                images: vec![],
                embedding: Some(fake::vec![f32; 768]),
                origin_year: None,
                authenticity: Default::default(),
                condition: Default::default(),
                provenance: Default::default(),
                restoration: Default::default(),
                auction_start: None,
                auction_end: None,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };

            let actual = product.change_state(to_state);

            assert!(actual.is_none());
        }

        #[rstest::rstest]
        #[case::listed(ProductState::Listed, ProductState::Available)]
        #[case::listed(ProductState::Listed, ProductState::Removed)]
        #[case::available(ProductState::Available, ProductState::Reserved)]
        #[case::available(ProductState::Available, ProductState::Sold)]
        #[case::available(ProductState::Available, ProductState::Removed)]
        #[case::reserved(ProductState::Reserved, ProductState::Available)]
        #[case::reserved(ProductState::Reserved, ProductState::Sold)]
        #[case::sold(ProductState::Sold, ProductState::Removed)]
        #[case::sold(ProductState::Sold, ProductState::Unknown)]
        #[case::sold(ProductState::Unknown, ProductState::Available)]
        #[trace]
        fn should_return_state_change_when_state_changed_for_change_state(
            #[case] from_state: ProductState,
            #[case] to_state: ProductState,
        ) {
            let mut product = Product {
                product_id: Default::default(),
                product_slug_id: Faker.fake(),
                shop_slug_id: Faker.fake(),
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_product_id: Default::default(),
                shop_name: "Boop".into(),
                shop_type: fake::Faker.fake(),
                category_id: Faker.fake(),
                category_name: Faker.fake(),
                period_id: Faker.fake(),
                period_name: Faker.fake(),
                native_title: Localized {
                    localization: Language::De,
                    payload: "Boop".into(),
                },
                other_title: Default::default(),
                native_description: None,
                other_description: Default::default(),
                native_price: None,
                other_price: Default::default(),
                native_price_estimate_min: None,
                other_price_estimate_min: Default::default(),
                native_price_estimate_max: None,
                other_price_estimate_max: Default::default(),
                state: from_state,
                url: Url::parse("https://example.com").unwrap(),
                images: vec![],
                embedding: Some(fake::vec![f32; 768]),
                origin_year: None,
                authenticity: Default::default(),
                condition: Default::default(),
                provenance: Default::default(),
                restoration: Default::default(),
                auction_start: None,
                auction_end: None,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };

            let actual = product.change_state(to_state).unwrap();
            let payload = actual.payload.as_state_changed().unwrap();
            assert_eq!(from_state, payload.old_state);
        }

        #[rstest::rstest]
        #[case::listed(ProductState::Listed, ProductState::Available)]
        #[case::listed(ProductState::Listed, ProductState::Removed)]
        #[case::available(ProductState::Available, ProductState::Reserved)]
        #[case::available(ProductState::Available, ProductState::Sold)]
        #[case::available(ProductState::Available, ProductState::Removed)]
        #[case::reserved(ProductState::Reserved, ProductState::Available)]
        #[case::reserved(ProductState::Reserved, ProductState::Sold)]
        #[case::sold(ProductState::Sold, ProductState::Removed)]
        #[case::sold(ProductState::Sold, ProductState::Unknown)]
        #[case::sold(ProductState::Unknown, ProductState::Available)]
        #[trace]
        fn should_change_state_when_state_changed_for_change_state(
            #[case] from_state: ProductState,
            #[case] to_state: ProductState,
        ) {
            let mut product = Product {
                product_id: Default::default(),
                product_slug_id: Faker.fake(),
                shop_slug_id: Faker.fake(),
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_product_id: Default::default(),
                shop_name: "Boop".into(),
                shop_type: fake::Faker.fake(),
                category_id: Faker.fake(),
                category_name: Faker.fake(),
                period_id: Faker.fake(),
                period_name: Faker.fake(),
                native_title: Localized {
                    localization: Language::De,
                    payload: "Boop".into(),
                },
                other_title: Default::default(),
                native_description: None,
                other_description: Default::default(),
                native_price: None,
                other_price: Default::default(),
                native_price_estimate_min: None,
                other_price_estimate_min: Default::default(),
                native_price_estimate_max: None,
                other_price_estimate_max: Default::default(),
                state: from_state,
                url: Url::parse("https://example.com").unwrap(),
                images: vec![],
                embedding: Some(fake::vec![f32; 768]),
                origin_year: None,
                authenticity: Default::default(),
                condition: Default::default(),
                provenance: Default::default(),
                restoration: Default::default(),
                auction_start: None,
                auction_end: None,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };

            let _ = product.change_state(to_state).unwrap();
            assert_eq!(to_state, product.state);
        }
    }

    mod price {
        use crate::core::product::Product;
        use crate::core::product_event::domain::ProductDomainEventPayload;
        use common::currency::domain::Currency;
        use common::language::domain::Language;
        use common::localized::Localized;
        use common::price::domain::{FxRate, MonetaryAmount, Price};
        use common::product_state::domain::ProductState;
        use fake::Fake;
        use fake::Faker;
        use time::OffsetDateTime;
        use url::Url;

        struct IdentityFxRate;

        impl FxRate for IdentityFxRate {
            fn exchange(
                &self,
                _from_currency: Currency,
                _to_currency: Currency,
                from_amount: MonetaryAmount,
            ) -> Result<MonetaryAmount, common::price::domain::MonetaryAmountOverflowError>
            {
                Ok(from_amount)
            }
        }

        #[rstest::rstest]
        #[case::eur_zero(Currency::Eur, 0u64.into())]
        #[case::gbp_zero(Currency::Gbp, 0u64.into())]
        #[case::usd_zero(Currency::Usd, 0u64.into())]
        #[case::aud_zero(Currency::Aud, 0u64.into())]
        #[case::cad_zero(Currency::Cad, 0u64.into())]
        #[case::nzd_zero(Currency::Nzd, 0u64.into())]
        #[case::eur_non_zero(Currency::Eur, 42u64.into())]
        #[case::gbp_non_zero(Currency::Gbp, 42u64.into())]
        #[case::usd_non_zero(Currency::Usd, 42u64.into())]
        #[case::aud_non_zero(Currency::Aud, 42u64.into())]
        #[case::cad_non_zero(Currency::Cad, 42u64.into())]
        #[case::nzd_non_zero(Currency::Nzd, 42u64.into())]
        #[trace]
        fn should_return_none_when_price_and_currency_did_not_change_for_new_price(
            #[case] currency: Currency,
            #[case] monetary_amount: MonetaryAmount,
        ) {
            let price = Price {
                monetary_amount,
                currency,
            };
            let mut product = Product {
                product_id: Default::default(),
                product_slug_id: Faker.fake(),
                shop_slug_id: Faker.fake(),
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_product_id: Default::default(),
                shop_name: "Boop".into(),
                shop_type: fake::Faker.fake(),
                category_id: Faker.fake(),
                category_name: Faker.fake(),
                period_id: Faker.fake(),
                period_name: Faker.fake(),
                native_title: Localized {
                    localization: Language::De,
                    payload: "Boop".into(),
                },
                other_title: Default::default(),
                native_description: None,
                other_description: Default::default(),
                native_price: Some(price),
                other_price: IdentityFxRate
                    .exchange_all(price.currency, price.monetary_amount)
                    .unwrap(),
                native_price_estimate_min: None,
                other_price_estimate_min: Default::default(),
                native_price_estimate_max: None,
                other_price_estimate_max: Default::default(),
                state: ProductState::Listed,
                url: Url::parse("https://example.com").unwrap(),
                images: vec![],
                embedding: Some(fake::vec![f32; 768]),
                origin_year: None,
                authenticity: Default::default(),
                condition: Default::default(),
                provenance: Default::default(),
                restoration: Default::default(),
                auction_start: None,
                auction_end: None,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };

            let actual = product.new_price(Some(price), &IdentityFxRate);

            assert!(actual.is_none());
        }

        #[rstest::rstest]
        #[case::eur_zero(Price::new(0u64.into(), Currency::Eur))]
        #[case::gbp_zero(Price::new(0u64.into(), Currency::Gbp))]
        #[case::usd_zero(Price::new(0u64.into(), Currency::Usd))]
        #[case::aud_zero(Price::new(0u64.into(), Currency::Aud))]
        #[case::cad_zero(Price::new(0u64.into(), Currency::Cad))]
        #[case::nzd_zero(Price::new(0u64.into(), Currency::Nzd))]
        #[case::eur_non_zero(Price::new(42u64.into(), Currency::Eur))]
        #[case::gbp_non_zero(Price::new(42u64.into(), Currency::Gbp))]
        #[case::usd_non_zero(Price::new(42u64.into(), Currency::Usd))]
        #[case::aud_non_zero(Price::new(42u64.into(), Currency::Aud))]
        #[case::cad_non_zero(Price::new(42u64.into(), Currency::Cad))]
        #[case::nzd_non_zero(Price::new(42u64.into(), Currency::Nzd))]
        #[trace]
        fn should_discover_price_when_price_changed_from_none_for_new_price(
            #[case] to_price: Price,
        ) {
            let mut product = Product {
                product_id: Default::default(),
                product_slug_id: Faker.fake(),
                shop_slug_id: Faker.fake(),
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_product_id: Default::default(),
                shop_name: "Boop".into(),
                shop_type: fake::Faker.fake(),
                category_id: Faker.fake(),
                category_name: Faker.fake(),
                period_id: Faker.fake(),
                period_name: Faker.fake(),
                native_title: Localized {
                    localization: Language::De,
                    payload: "Boop".into(),
                },
                other_title: Default::default(),
                native_description: None,
                other_description: Default::default(),
                native_price: None,
                other_price: Default::default(),
                native_price_estimate_min: None,
                other_price_estimate_min: Default::default(),
                native_price_estimate_max: None,
                other_price_estimate_max: Default::default(),
                state: ProductState::Listed,
                url: Url::parse("https://example.com").unwrap(),
                images: vec![],
                embedding: Some(fake::vec![f32; 768]),
                origin_year: None,
                authenticity: Default::default(),
                condition: Default::default(),
                provenance: Default::default(),
                restoration: Default::default(),
                auction_start: None,
                auction_end: None,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };
            let actual = product.new_price(Some(to_price), &IdentityFxRate).unwrap();

            match actual.payload {
                ProductDomainEventPayload::PriceChanged(payload) => {
                    assert_eq!(to_price, payload.new_native_price.unwrap());
                    assert!(
                        payload
                            .new_other_price
                            .iter()
                            .all(|(_, amount)| &to_price.monetary_amount == amount)
                    );
                    assert!(payload.old_native_price.is_none());
                    assert!(payload.old_other_price.is_empty());
                    assert_eq!(product.native_price, Some(to_price));
                    assert!(
                        product
                            .other_price
                            .iter()
                            .all(|(_, amount)| &to_price.monetary_amount == amount)
                    );
                }
                _ => panic!("Expected ProductEventPayload::PriceChanged"),
            }
        }

        #[rstest::rstest]
        #[case::eur_non_zero(Price::new(420u64.into(), Currency::Eur))]
        #[case::gbp_non_zero(Price::new(430u64.into(), Currency::Gbp))]
        #[case::usd_non_zero(Price::new(440u64.into(), Currency::Usd))]
        #[case::aud_non_zero(Price::new(450u64.into(), Currency::Aud))]
        #[case::cad_non_zero(Price::new(460u64.into(), Currency::Cad))]
        #[case::nzd_non_zero(Price::new(477u64.into(), Currency::Nzd))]
        #[trace]
        fn should_find_dropped_price_when_price_dropped_for_new_price(#[case] to_price: Price) {
            let mut product = Product {
                product_id: Default::default(),
                product_slug_id: Faker.fake(),
                shop_slug_id: Faker.fake(),
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_product_id: Default::default(),
                shop_name: "Boop".into(),
                shop_type: fake::Faker.fake(),
                category_id: Faker.fake(),
                category_name: Faker.fake(),
                period_id: Faker.fake(),
                period_name: Faker.fake(),
                native_title: Localized {
                    localization: Language::De,
                    payload: "Boop".into(),
                },
                other_title: Default::default(),
                native_description: None,
                other_description: Default::default(),
                native_price: Some(Price::new(700u64.into(), Currency::Eur)),
                other_price: Default::default(),
                native_price_estimate_min: None,
                other_price_estimate_min: Default::default(),
                native_price_estimate_max: None,
                other_price_estimate_max: Default::default(),
                state: ProductState::Listed,
                url: Url::parse("https://example.com").unwrap(),
                images: vec![],
                embedding: Some(fake::vec![f32; 768]),
                origin_year: None,
                authenticity: Default::default(),
                condition: Default::default(),
                provenance: Default::default(),
                restoration: Default::default(),
                auction_start: None,
                auction_end: None,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };
            let actual = product.new_price(Some(to_price), &IdentityFxRate).unwrap();

            match actual.payload {
                ProductDomainEventPayload::PriceChanged(payload) => {
                    assert_eq!(to_price, payload.new_native_price.unwrap());
                    assert!(
                        payload
                            .new_other_price
                            .iter()
                            .all(|(_, amount)| &to_price.monetary_amount == amount)
                    );
                    assert_eq!(
                        Price::new(700u64.into(), Currency::Eur),
                        payload.old_native_price.unwrap()
                    );
                    assert_eq!(product.native_price, Some(to_price));
                    assert!(
                        product
                            .other_price
                            .iter()
                            .all(|(_, amount)| &to_price.monetary_amount == amount)
                    );
                }
                _ => panic!("Expected ProductEventPayload::PriceChanged"),
            }
        }

        #[rstest::rstest]
        #[case::eur_non_zero(Price::new(420u64.into(), Currency::Eur))]
        #[case::gbp_non_zero(Price::new(430u64.into(), Currency::Gbp))]
        #[case::usd_non_zero(Price::new(440u64.into(), Currency::Usd))]
        #[case::aud_non_zero(Price::new(450u64.into(), Currency::Aud))]
        #[case::cad_non_zero(Price::new(460u64.into(), Currency::Cad))]
        #[case::nzd_non_zero(Price::new(477u64.into(), Currency::Nzd))]
        #[trace]
        fn should_find_increased_price_when_price_increased_for_new_price(#[case] to_price: Price) {
            let mut product = Product {
                product_id: Default::default(),
                product_slug_id: Faker.fake(),
                shop_slug_id: Faker.fake(),
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_product_id: Default::default(),
                shop_name: "Boop".into(),
                shop_type: fake::Faker.fake(),
                category_id: Faker.fake(),
                category_name: Faker.fake(),
                period_id: Faker.fake(),
                period_name: Faker.fake(),
                native_title: Localized {
                    localization: Language::De,
                    payload: "Boop".into(),
                },
                other_title: Default::default(),
                native_description: None,
                other_description: Default::default(),
                native_price: Some(Price::new(169u64.into(), Currency::Eur)),
                other_price: Default::default(),
                native_price_estimate_min: None,
                other_price_estimate_min: Default::default(),
                native_price_estimate_max: None,
                other_price_estimate_max: Default::default(),
                state: ProductState::Listed,
                url: Url::parse("https://example.com").unwrap(),
                images: vec![],
                embedding: Some(fake::vec![f32; 768]),
                origin_year: None,
                authenticity: Default::default(),
                condition: Default::default(),
                provenance: Default::default(),
                restoration: Default::default(),
                auction_start: None,
                auction_end: None,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };
            let actual = product.new_price(Some(to_price), &IdentityFxRate).unwrap();

            match actual.payload {
                ProductDomainEventPayload::PriceChanged(payload) => {
                    assert_eq!(to_price, payload.new_native_price.unwrap());
                    assert!(
                        payload
                            .new_other_price
                            .iter()
                            .all(|(_, amount)| &to_price.monetary_amount == amount)
                    );
                    assert_eq!(
                        Price::new(169u64.into(), Currency::Eur),
                        payload.old_native_price.unwrap()
                    );
                    assert_eq!(product.native_price, Some(to_price));
                }
                _ => panic!("Expected ProductEventPayload::PriceChanged"),
            }
        }

        #[rstest::rstest]
        #[case::eur_zero(Price::new(0u64.into(), Currency::Eur))]
        #[case::gbp_zero(Price::new(0u64.into(), Currency::Gbp))]
        #[case::usd_zero(Price::new(0u64.into(), Currency::Usd))]
        #[case::aud_zero(Price::new(0u64.into(), Currency::Aud))]
        #[case::cad_zero(Price::new(0u64.into(), Currency::Cad))]
        #[case::nzd_zero(Price::new(0u64.into(), Currency::Nzd))]
        #[case::eur_non_zero(Price::new(42u64.into(), Currency::Eur))]
        #[case::gbp_non_zero(Price::new(42u64.into(), Currency::Gbp))]
        #[case::usd_non_zero(Price::new(42u64.into(), Currency::Usd))]
        #[case::aud_non_zero(Price::new(42u64.into(), Currency::Aud))]
        #[case::cad_non_zero(Price::new(42u64.into(), Currency::Cad))]
        #[case::nzd_non_zero(Price::new(42u64.into(), Currency::Nzd))]
        #[trace]
        fn should_remove_price_when_price_changed_from_some_to_none_for_new_price(
            #[case] price: Price,
        ) {
            let mut product = Product {
                product_id: Default::default(),
                product_slug_id: Faker.fake(),
                shop_slug_id: Faker.fake(),
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_product_id: Default::default(),
                shop_type: fake::Faker.fake(),
                category_id: Faker.fake(),
                category_name: Faker.fake(),
                period_id: Faker.fake(),
                period_name: Faker.fake(),
                shop_name: "Boop".into(),
                native_title: Localized {
                    localization: Language::De,
                    payload: "Boop".into(),
                },
                other_title: Default::default(),
                native_description: None,
                other_description: Default::default(),
                native_price: Some(price),
                other_price: Default::default(),
                native_price_estimate_min: None,
                other_price_estimate_min: Default::default(),
                native_price_estimate_max: None,
                other_price_estimate_max: Default::default(),
                state: ProductState::Listed,
                url: Url::parse("https://example.com").unwrap(),
                images: vec![],
                embedding: Some(fake::vec![f32; 768]),
                origin_year: None,
                authenticity: Default::default(),
                condition: Default::default(),
                provenance: Default::default(),
                restoration: Default::default(),
                auction_start: None,
                auction_end: None,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };
            let actual = product.new_price(None, &IdentityFxRate).unwrap();

            match actual.payload {
                ProductDomainEventPayload::PriceChanged(payload) => {
                    assert!(product.native_price.is_none());
                    assert!(product.other_price.is_empty());
                    assert_eq!(price, payload.old_native_price.unwrap());
                    assert!(payload.new_native_price.is_none());
                    assert!(payload.new_other_price.is_empty());
                }
                _ => panic!("Expected ProductEventPayload::PriceChanged"),
            }
        }
    }

    mod change_price {
        use crate::core::product::Product;
        use common::currency::domain::Currency;
        use common::language::domain::Language;
        use common::localized::Localized;
        use common::price::domain::{FxRate, MonetaryAmount, Price};
        use common::product_state::domain::ProductState;
        use fake::{Fake, Faker};
        use rstest;
        use time::OffsetDateTime;
        use url::Url;

        struct IdentityFxRate;

        impl FxRate for IdentityFxRate {
            fn exchange(
                &self,
                _from_currency: Currency,
                _to_currency: Currency,
                from_amount: MonetaryAmount,
            ) -> Result<MonetaryAmount, common::price::domain::MonetaryAmountOverflowError>
            {
                Ok(from_amount)
            }
        }

        #[rstest::rstest]
        #[case::eur(Price::new(42u64.into(), Currency::Eur))]
        #[case::gbp(Price::new(42u64.into(), Currency::Gbp))]
        #[case::usd(Price::new(42u64.into(), Currency::Usd))]
        #[trace]
        fn should_return_none_when_price_did_not_change_for_change_price(#[case] price: Price) {
            let mut product = Product {
                product_id: Default::default(),
                product_slug_id: Faker.fake(),
                shop_slug_id: Faker.fake(),
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_product_id: Default::default(),
                shop_name: "Boop".into(),
                shop_type: fake::Faker.fake(),
                category_id: Faker.fake(),
                category_name: Faker.fake(),
                period_id: Faker.fake(),
                period_name: Faker.fake(),
                native_title: Localized::new(Language::De, "Titel".into()),
                other_title: Default::default(),
                native_description: None,
                other_description: Default::default(),
                native_price: Some(price),
                other_price: IdentityFxRate
                    .exchange_all(price.currency, price.monetary_amount)
                    .unwrap(),
                native_price_estimate_min: None,
                other_price_estimate_min: Default::default(),
                native_price_estimate_max: None,
                other_price_estimate_max: Default::default(),
                state: ProductState::Listed,
                url: Url::parse("https://example.com").unwrap(),
                images: vec![],
                embedding: Some(fake::vec![f32; 768]),
                origin_year: None,
                authenticity: Default::default(),
                condition: Default::default(),
                provenance: Default::default(),
                restoration: Default::default(),
                auction_start: None,
                auction_end: None,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };

            let actual = product.change_price(price, &IdentityFxRate);
            assert!(actual.is_none());
        }
    }

    mod translation {
        use crate::core::product::Product;
        use crate::core::product_event::enrichment::ProductEnrichmentEventPayload;
        use common::language::domain::Language;
        use common::localized::Localized;
        use fake::{Fake, Faker};
        use time::OffsetDateTime;
        use url::Url;

        #[test]
        fn should_return_none_when_translated_title_is_same_for_translate_title() {
            let mut product = Product {
                product_id: Default::default(),
                product_slug_id: Faker.fake(),
                shop_slug_id: Faker.fake(),
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_product_id: Default::default(),
                shop_name: "Boop".into(),
                shop_type: fake::Faker.fake(),
                category_id: Faker.fake(),
                category_name: Faker.fake(),
                period_id: Faker.fake(),
                period_name: Faker.fake(),
                native_title: Localized::new(Language::De, "Titel".into()),
                other_title: {
                    let mut m = std::collections::HashMap::new();
                    m.insert(Language::En, "Title".into());
                    m
                },
                native_description: None,
                other_description: Default::default(),
                native_price: None,
                other_price: Default::default(),
                native_price_estimate_min: None,
                other_price_estimate_min: Default::default(),
                native_price_estimate_max: None,
                other_price_estimate_max: Default::default(),
                state: common::product_state::domain::ProductState::Listed,
                url: Url::parse("https://example.com").unwrap(),
                images: vec![],
                embedding: Some(fake::vec![f32; 768]),
                origin_year: None,
                authenticity: Default::default(),
                condition: Default::default(),
                provenance: Default::default(),
                restoration: Default::default(),
                auction_start: None,
                auction_end: None,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };

            let actual = product.translate_title(Language::De, Language::En, "Title".into());
            assert!(actual.is_none());
        }

        #[test]
        fn should_emit_event_and_store_translated_title_when_new_for_translate_title() {
            let mut product = Product {
                product_id: Default::default(),
                product_slug_id: Faker.fake(),
                shop_slug_id: Faker.fake(),
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_product_id: Default::default(),
                shop_name: "Boop".into(),
                shop_type: fake::Faker.fake(),
                category_id: Faker.fake(),
                category_name: Faker.fake(),
                period_id: Faker.fake(),
                period_name: Faker.fake(),
                native_title: Localized::new(Language::De, "Titel".into()),
                other_title: Default::default(),
                native_description: None,
                other_description: Default::default(),
                native_price: None,
                other_price: Default::default(),
                native_price_estimate_min: None,
                other_price_estimate_min: Default::default(),
                native_price_estimate_max: None,
                other_price_estimate_max: Default::default(),
                state: common::product_state::domain::ProductState::Listed,
                url: Url::parse("https://example.com").unwrap(),
                images: vec![],
                embedding: Some(fake::vec![f32; 768]),
                origin_year: None,
                authenticity: Default::default(),
                condition: Default::default(),
                provenance: Default::default(),
                restoration: Default::default(),
                auction_start: None,
                auction_end: None,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };

            let actual = product
                .translate_title(Language::De, Language::En, "Title".into())
                .unwrap();
            match actual.payload {
                ProductEnrichmentEventPayload::TranslatedTitle(payload) => {
                    assert_eq!(payload.target_language, Language::En);
                    assert_eq!(payload.target, "Title".into());
                    assert_eq!(
                        product.other_title.get(&Language::En).cloned(),
                        Some("Title".into())
                    );
                }
                _ => panic!("Expected ProductEnrichmentEventPayload::TranslatedTitle"),
            }
        }

        #[test]
        fn should_return_none_when_translated_description_is_same_for_translate_description() {
            let mut product = Product {
                product_id: Default::default(),
                product_slug_id: Faker.fake(),
                shop_slug_id: Faker.fake(),
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_product_id: Default::default(),
                shop_name: "Boop".into(),
                shop_type: fake::Faker.fake(),
                category_id: Faker.fake(),
                category_name: Faker.fake(),
                period_id: Faker.fake(),
                period_name: Faker.fake(),
                native_title: Localized::new(Language::De, "Titel".into()),
                other_title: Default::default(),
                native_description: None,
                other_description: {
                    let mut m = std::collections::HashMap::new();
                    m.insert(Language::En, "Desc".into());
                    m
                },
                native_price: None,
                other_price: Default::default(),
                native_price_estimate_min: None,
                other_price_estimate_min: Default::default(),
                native_price_estimate_max: None,
                other_price_estimate_max: Default::default(),
                state: common::product_state::domain::ProductState::Listed,
                url: Url::parse("https://example.com").unwrap(),
                images: vec![],
                embedding: Some(fake::vec![f32; 768]),
                origin_year: None,
                authenticity: Default::default(),
                condition: Default::default(),
                provenance: Default::default(),
                restoration: Default::default(),
                auction_start: None,
                auction_end: None,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };

            let actual = product.translate_description(Language::De, Language::En, "Desc".into());
            assert!(actual.is_none());
        }

        #[test]
        fn should_emit_event_and_store_translated_description_when_new_for_translate_description() {
            let mut product = Product {
                product_id: Default::default(),
                product_slug_id: Faker.fake(),
                shop_slug_id: Faker.fake(),
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_product_id: Default::default(),
                shop_name: "Boop".into(),
                shop_type: fake::Faker.fake(),
                category_id: Faker.fake(),
                category_name: Faker.fake(),
                period_id: Faker.fake(),
                period_name: Faker.fake(),
                native_title: Localized::new(Language::De, "Titel".into()),
                other_title: Default::default(),
                native_description: None,
                other_description: Default::default(),
                native_price: None,
                other_price: Default::default(),
                native_price_estimate_min: None,
                other_price_estimate_min: Default::default(),
                native_price_estimate_max: None,
                other_price_estimate_max: Default::default(),
                state: common::product_state::domain::ProductState::Listed,
                url: Url::parse("https://example.com").unwrap(),
                images: vec![],
                embedding: Some(fake::vec![f32; 768]),
                origin_year: None,
                authenticity: Default::default(),
                condition: Default::default(),
                provenance: Default::default(),
                restoration: Default::default(),
                auction_start: None,
                auction_end: None,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };

            let actual = product
                .translate_description(Language::De, Language::En, "Desc".into())
                .unwrap();
            match actual.payload {
                ProductEnrichmentEventPayload::TranslatedDescription(payload) => {
                    assert_eq!(payload.target_language, Language::En);
                    assert_eq!(payload.target, "Desc".into());
                    assert_eq!(
                        product.other_description.get(&Language::En).cloned(),
                        Some("Desc".into())
                    );
                }
                _ => panic!("Expected ProductEnrichmentEventPayload::TranslatedDescription"),
            }
        }
    }

    mod embedding {
        use crate::core::product::Product;
        use crate::core::product_event::enrichment::ProductEnrichmentEventPayload;
        use common::language::domain::Language;
        use common::localized::Localized;
        use common::product_state::domain::ProductState;
        use fake::{Fake, Faker};
        use time::OffsetDateTime;
        use url::Url;

        #[test]
        fn should_return_none_when_embedding_is_same_for_embed() {
            let embedding = vec![0.1f32, 0.2f32, 0.3f32];
            let mut product = Product {
                product_id: Default::default(),
                product_slug_id: Faker.fake(),
                shop_slug_id: Faker.fake(),
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_product_id: Default::default(),
                shop_name: "Boop".into(),
                shop_type: fake::Faker.fake(),
                category_id: Faker.fake(),
                category_name: Faker.fake(),
                period_id: Faker.fake(),
                period_name: Faker.fake(),
                native_title: Localized::new(Language::De, "Titel".into()),
                other_title: Default::default(),
                native_description: None,
                other_description: Default::default(),
                native_price: None,
                other_price: Default::default(),
                native_price_estimate_min: None,
                other_price_estimate_min: Default::default(),
                native_price_estimate_max: None,
                other_price_estimate_max: Default::default(),
                state: ProductState::Listed,
                url: Url::parse("https://example.com").unwrap(),
                images: vec![],
                embedding: Some(embedding.clone()),
                origin_year: None,
                authenticity: Default::default(),
                condition: Default::default(),
                provenance: Default::default(),
                restoration: Default::default(),
                auction_start: None,
                auction_end: None,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };

            let actual = product.embed(embedding);
            assert!(actual.is_none());
        }

        #[test]
        fn should_emit_event_and_store_embedding_when_new_for_embed() {
            let embedding = vec![0.1f32, 0.2f32, 0.3f32];
            let mut product = Product {
                product_id: Default::default(),
                product_slug_id: Faker.fake(),
                shop_slug_id: Faker.fake(),
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_product_id: Default::default(),
                shop_name: "Boop".into(),
                shop_type: fake::Faker.fake(),
                category_id: Faker.fake(),
                category_name: Faker.fake(),
                period_id: Faker.fake(),
                period_name: Faker.fake(),
                native_title: Localized::new(Language::De, "Titel".into()),
                other_title: Default::default(),
                native_description: None,
                other_description: Default::default(),
                native_price: None,
                other_price: Default::default(),
                native_price_estimate_min: None,
                other_price_estimate_min: Default::default(),
                native_price_estimate_max: None,
                other_price_estimate_max: Default::default(),
                state: ProductState::Listed,
                url: Url::parse("https://example.com").unwrap(),
                images: vec![],
                embedding: None,
                origin_year: None,
                authenticity: Default::default(),
                condition: Default::default(),
                provenance: Default::default(),
                restoration: Default::default(),
                auction_start: None,
                auction_end: None,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };

            let actual = product.embed(embedding.clone()).unwrap();
            match actual.payload {
                ProductEnrichmentEventPayload::Embedded(payload) => {
                    assert_eq!(payload.embedding, embedding);
                    assert_eq!(product.embedding, Some(embedding));
                }
                _ => panic!("Expected ProductEnrichmentEventPayload::Embedded"),
            }
        }
    }

    mod attributes {
        use crate::core::product::Product;
        use crate::core::product_event::enrichment::ProductEnrichmentEventPayload;
        use common::language::domain::Language;
        use common::localized::Localized;
        use common::product_state::domain::ProductState;
        use fake::{Fake, Faker};
        use time::OffsetDateTime;
        use url::Url;

        #[test]
        fn should_emit_event_and_update_fields_for_extract_attributes() {
            let mut product = Product {
                product_id: Default::default(),
                product_slug_id: Faker.fake(),
                shop_slug_id: Faker.fake(),
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_product_id: Default::default(),
                shop_name: "Boop".into(),
                shop_type: fake::Faker.fake(),
                category_id: Faker.fake(),
                category_name: Faker.fake(),
                period_id: Faker.fake(),
                period_name: Faker.fake(),
                native_title: Localized::new(Language::De, "Titel".into()),
                other_title: Default::default(),
                native_description: None,
                other_description: Default::default(),
                native_price: None,
                other_price: Default::default(),
                native_price_estimate_min: None,
                other_price_estimate_min: Default::default(),
                native_price_estimate_max: None,
                other_price_estimate_max: Default::default(),
                state: ProductState::Listed,
                url: Url::parse("https://example.com").unwrap(),
                images: vec![],
                embedding: None,
                origin_year: None,
                authenticity: Default::default(),
                condition: Default::default(),
                provenance: Default::default(),
                restoration: Default::default(),
                auction_start: None,
                auction_end: None,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };

            let origin_year = Faker.fake();
            let authenticity = crate::core::authenticity::Authenticity::LaterCopy;
            let condition = crate::core::condition::Condition::Excellent;
            let provenance = crate::core::provenance::Provenance::Claimed;
            let restoration = crate::core::restoration::Restoration::Major;

            let actual = product
                .extract_attributes(
                    origin_year,
                    Some(authenticity),
                    Some(condition),
                    Some(provenance),
                    Some(restoration),
                )
                .unwrap();

            match actual.payload {
                ProductEnrichmentEventPayload::ExtractedAttributes(payload) => {
                    assert_eq!(payload.authenticity.unwrap(), authenticity);
                    assert_eq!(payload.condition.unwrap(), condition);
                    assert_eq!(payload.provenance.unwrap(), provenance);
                    assert_eq!(payload.restoration.unwrap(), restoration);

                    assert_eq!(product.origin_year, origin_year);
                    assert_eq!(product.authenticity, authenticity);
                    assert_eq!(product.condition, condition);
                    assert_eq!(product.provenance, provenance);
                    assert_eq!(product.restoration, restoration);
                }
                _ => panic!("Expected ProductEnrichmentEventPayload::ExtractedAttributes"),
            }
        }
    }

    mod policy {
        use crate::core::product_event::policy::ProductPolicyEventPayload;
        use crate::core::product_image::ProductImage;
        use crate::core::prohibited_content::ProhibitedContent;
        use crate::core::{product::Product, prohibited_content::ProhibitedContentReason};
        use common::language::domain::Language;
        use common::localized::Localized;
        use common::product_state::domain::ProductState;
        use fake::{Fake, Faker};
        use time::OffsetDateTime;
        use url::Url;

        #[test]
        fn should_return_none_and_not_mutate_when_unknown_for_prohibit_content() {
            let mut product = Product {
                product_id: Default::default(),
                product_slug_id: Faker.fake(),
                shop_slug_id: Faker.fake(),
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_product_id: Default::default(),
                shop_name: "Boop".into(),
                shop_type: fake::Faker.fake(),
                category_id: Faker.fake(),
                category_name: Faker.fake(),
                period_id: Faker.fake(),
                period_name: Faker.fake(),
                native_title: Localized::new(Language::De, "Titel".into()),
                other_title: Default::default(),
                native_description: None,
                other_description: Default::default(),
                native_price: None,
                other_price: Default::default(),
                native_price_estimate_min: None,
                other_price_estimate_min: Default::default(),
                native_price_estimate_max: None,
                other_price_estimate_max: Default::default(),
                state: ProductState::Listed,
                url: Url::parse("https://example.com").unwrap(),
                images: vec![ProductImage {
                    url: Url::parse("https://example.com/image.jpg").unwrap(),
                    prohibited_content: ProhibitedContent::Unknown,
                }],
                embedding: None,
                origin_year: None,
                authenticity: Default::default(),
                condition: Default::default(),
                provenance: Default::default(),
                restoration: Default::default(),
                auction_start: None,
                auction_end: None,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };

            let actual = product.prohibit_content(
                ProhibitedContent::Unknown,
                ProhibitedContentReason::ProductText,
            );
            assert!(actual.is_none());
            assert!(
                product
                    .images
                    .iter()
                    .all(|i| i.prohibited_content == ProhibitedContent::Unknown)
            );
        }

        #[test]
        fn should_emit_event_and_update_images_when_prohibited_for_prohibit_content() {
            let mut product = Product {
                product_id: Default::default(),
                product_slug_id: Faker.fake(),
                shop_slug_id: Faker.fake(),
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_product_id: Default::default(),
                shop_name: "Boop".into(),
                shop_type: fake::Faker.fake(),
                category_id: Faker.fake(),
                category_name: Faker.fake(),
                period_id: Faker.fake(),
                period_name: Faker.fake(),
                native_title: Localized::new(Language::De, "Titel".into()),
                other_title: Default::default(),
                native_description: None,
                other_description: Default::default(),
                native_price: None,
                other_price: Default::default(),
                native_price_estimate_min: None,
                other_price_estimate_min: Default::default(),
                native_price_estimate_max: None,
                other_price_estimate_max: Default::default(),
                state: ProductState::Listed,
                url: Url::parse("https://example.com").unwrap(),
                images: vec![
                    ProductImage {
                        url: Url::parse("https://example.com/image1.jpg").unwrap(),
                        prohibited_content: ProhibitedContent::Unknown,
                    },
                    ProductImage {
                        url: Url::parse("https://example.com/image2.jpg").unwrap(),
                        prohibited_content: ProhibitedContent::Unknown,
                    },
                ],
                embedding: None,
                origin_year: None,
                authenticity: Default::default(),
                condition: Default::default(),
                provenance: Default::default(),
                restoration: Default::default(),
                auction_start: None,
                auction_end: None,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };

            let actual = product
                .prohibit_content(
                    ProhibitedContent::NaziGermany,
                    ProhibitedContentReason::ProductText,
                )
                .unwrap();

            match actual.payload {
                ProductPolicyEventPayload::ProhibitedContentDecision(payload) => {
                    assert_eq!(payload.decision, ProhibitedContent::NaziGermany);
                    assert!(
                        product
                            .images
                            .iter()
                            .all(|i| i.prohibited_content == ProhibitedContent::NaziGermany)
                    );
                }
            }
        }
    }

    mod localization {
        use crate::core::product::Product;
        use common::currency::domain::Currency;
        use common::language::domain::Language;
        use common::localized::Localized;
        use common::price::domain::Price;
        use common::product_state::domain::ProductState;
        use fake::{Fake, Faker};
        use time::OffsetDateTime;
        use url::Url;

        #[test]
        fn should_localize_fields_when_available_for_localized() {
            let product = Product {
                product_id: Default::default(),
                product_slug_id: Faker.fake(),
                shop_slug_id: Faker.fake(),
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_product_id: Default::default(),
                shop_name: "Boop".into(),
                shop_type: fake::Faker.fake(),
                category_id: Faker.fake(),
                category_name: Faker.fake(),
                period_id: Faker.fake(),
                period_name: Faker.fake(),
                native_title: Localized::new(Language::De, "Titel".into()),
                other_title: {
                    let mut m = std::collections::HashMap::new();
                    m.insert(Language::En, "Title".into());
                    m
                },
                native_description: Some(Localized::new(Language::De, "Beschreibung".into())),
                other_description: {
                    let mut m = std::collections::HashMap::new();
                    m.insert(Language::En, "Description".into());
                    m
                },
                native_price: Some(Price::new(42u64.into(), Currency::Eur)),
                other_price: {
                    let mut m = std::collections::HashMap::new();
                    m.insert(Currency::Usd, 42u64.into());
                    m
                },
                native_price_estimate_min: Some(Price::new(10u64.into(), Currency::Eur)),
                other_price_estimate_min: {
                    let mut m = std::collections::HashMap::new();
                    m.insert(Currency::Usd, 10u64.into());
                    m
                },
                native_price_estimate_max: Some(Price::new(100u64.into(), Currency::Eur)),
                other_price_estimate_max: {
                    let mut m = std::collections::HashMap::new();
                    m.insert(Currency::Usd, 100u64.into());
                    m
                },
                state: ProductState::Listed,
                url: Url::parse("https://example.com").unwrap(),
                images: vec![],
                embedding: None,
                origin_year: None,
                authenticity: Default::default(),
                condition: Default::default(),
                provenance: Default::default(),
                restoration: Default::default(),
                auction_start: None,
                auction_end: None,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };

            let view = product.localized(&Currency::Usd, &[Language::En, Language::De]);

            assert_eq!(view.title.localization, Language::En);
            assert_eq!(view.title.payload, "Title".into());
            assert_eq!(
                view.description.as_ref().unwrap().localization,
                Language::En
            );
            assert_eq!(
                view.description.as_ref().unwrap().payload,
                "Description".into()
            );
            assert_eq!(view.price.unwrap().currency, Currency::Usd);
            assert_eq!(view.price.unwrap().monetary_amount, 42u64.into());
            assert_eq!(view.price_estimate_min.unwrap().currency, Currency::Usd);
            assert_eq!(
                view.price_estimate_min.unwrap().monetary_amount,
                10u64.into()
            );
            assert_eq!(view.price_estimate_max.unwrap().currency, Currency::Usd);
            assert_eq!(
                view.price_estimate_max.unwrap().monetary_amount,
                100u64.into()
            );
        }

        #[test]
        fn should_fallback_to_native_when_preferred_missing_for_localized() {
            let product = Product {
                product_id: Default::default(),
                product_slug_id: Faker.fake(),
                shop_slug_id: Faker.fake(),
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_product_id: Default::default(),
                shop_name: "Boop".into(),
                shop_type: fake::Faker.fake(),
                category_id: Faker.fake(),
                category_name: Faker.fake(),
                period_id: Faker.fake(),
                period_name: Faker.fake(),
                native_title: Localized::new(Language::De, "Titel".into()),
                other_title: Default::default(),
                native_description: Some(Localized::new(Language::De, "Beschreibung".into())),
                other_description: Default::default(),
                native_price: Some(Price::new(42u64.into(), Currency::Eur)),
                other_price: Default::default(),
                native_price_estimate_min: Some(Price::new(10u64.into(), Currency::Eur)),
                other_price_estimate_min: Default::default(),
                native_price_estimate_max: Some(Price::new(100u64.into(), Currency::Eur)),
                other_price_estimate_max: Default::default(),
                state: ProductState::Listed,
                url: Url::parse("https://example.com").unwrap(),
                images: vec![],
                embedding: None,
                origin_year: None,
                authenticity: Default::default(),
                condition: Default::default(),
                provenance: Default::default(),
                restoration: Default::default(),
                auction_start: None,
                auction_end: None,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };

            let view = product.localized(&Currency::Eur, &[Language::En, Language::De]);

            assert_eq!(view.title.localization, Language::De);
            assert_eq!(view.title.payload, "Titel".into());
            assert_eq!(
                view.description.as_ref().unwrap().localization,
                Language::De
            );
            assert_eq!(
                view.description.as_ref().unwrap().payload,
                "Beschreibung".into()
            );
            assert_eq!(view.price.unwrap().currency, Currency::Eur);
            assert_eq!(view.price.unwrap().monetary_amount, 42u64.into());
            assert_eq!(view.price_estimate_min.unwrap().currency, Currency::Eur);
            assert_eq!(
                view.price_estimate_min.unwrap().monetary_amount,
                10u64.into()
            );
            assert_eq!(view.price_estimate_max.unwrap().currency, Currency::Eur);
            assert_eq!(
                view.price_estimate_max.unwrap().monetary_amount,
                100u64.into()
            );
        }
    }

    mod apply {
        use crate::core::authenticity::Authenticity;
        use crate::core::condition::Condition;
        use crate::core::origin_year::OriginYear;
        use crate::core::product::Product;
        use crate::core::product_event::ProductEventPayload;
        use crate::core::product_event::domain::{
            ProductDomainEventPayload, ProductPriceChangeDomainEventPayload,
            ProductStateChangeDomainEventPayload,
        };
        use crate::core::product_event::enrichment::{
            ClassifiedCategoryProductEnrichmentEventPayload,
            ClassifiedPeriodProductEnrichmentEventPayload, EmbeddedProductEnrichmentEventPayload,
            ExtractedAttributesProductEnrichmentEventPayload, ProductEnrichmentEventPayload,
            TranslationProductEnrichmentEventPayload,
        };
        use crate::core::product_event::policy::{
            ProductPolicyEventPayload, ProhibitedContentProductPolicyEventPayload,
        };
        use crate::core::product_image::ProductImage;
        use crate::core::prohibited_content::{ProhibitedContent, ProhibitedContentReason};
        use crate::core::provenance::Provenance;
        use crate::core::restoration::Restoration;
        use common::category_key::CategoryId;
        use common::currency::domain::Currency;
        use common::event::Event;
        use common::event_id::EventId;
        use common::language::domain::Language;
        use common::period_key::PeriodId;
        use common::price::domain::Price;
        use common::product_state::domain::ProductState;
        use common::year::{Year, YearRange};
        use fake::{Fake, Faker};
        use std::collections::HashMap;
        use time::OffsetDateTime;
        use url::Url;

        fn make_event(
            product: &Product,
            payload: ProductEventPayload,
        ) -> Event<common::product_id::ProductId, ProductEventPayload> {
            Event {
                aggregate_id: product.product_id,
                event_id: EventId::new(),
                timestamp: OffsetDateTime::now_utc(),
                payload,
            }
        }

        #[test]
        fn should_update_event_id_and_timestamp_when_any_event_for_apply() {
            let mut product: Product = Faker.fake();
            let original_event_id = product.event_id;

            let event_id = EventId::new();
            let timestamp = OffsetDateTime::now_utc();
            let event = Event {
                aggregate_id: product.product_id,
                event_id,
                timestamp,
                payload: ProductEventPayload::ProductDomainEvent(
                    ProductDomainEventPayload::StateChanged(ProductStateChangeDomainEventPayload {
                        shop_id: product.shop_id,
                        shops_product_id: product.shops_product_id.clone(),
                        old_state: product.state,
                        new_state: ProductState::Listed,
                    }),
                ),
            };

            product.apply(event);

            assert_eq!(product.event_id, event_id);
            assert_ne!(product.event_id, original_event_id);
            assert_eq!(product.updated, timestamp);
        }

        #[test]
        fn should_update_state_to_listed_when_state_listed_event_for_apply() {
            let mut product: Product = Faker.fake();
            product.state = ProductState::Available;

            let event = make_event(
                &product,
                ProductEventPayload::ProductDomainEvent(ProductDomainEventPayload::StateChanged(
                    ProductStateChangeDomainEventPayload {
                        shop_id: product.shop_id,
                        shops_product_id: product.shops_product_id.clone(),
                        old_state: ProductState::Available,
                        new_state: ProductState::Listed,
                    },
                )),
            );

            product.apply(event);

            assert_eq!(product.state, ProductState::Listed);
        }

        #[test]
        fn should_update_state_to_sold_when_state_sold_event_for_apply() {
            let mut product: Product = Faker.fake();
            product.state = ProductState::Available;

            let event = make_event(
                &product,
                ProductEventPayload::ProductDomainEvent(ProductDomainEventPayload::StateChanged(
                    ProductStateChangeDomainEventPayload {
                        shop_id: product.shop_id,
                        shops_product_id: product.shops_product_id.clone(),
                        old_state: ProductState::Available,
                        new_state: ProductState::Sold,
                    },
                )),
            );

            product.apply(event);

            assert_eq!(product.state, ProductState::Sold);
        }

        #[test]
        fn should_set_native_price_when_price_discovered_event_for_apply() {
            let mut product: Product = Faker.fake();
            product.native_price = None;
            product.other_price.clear();

            let price = Price::new(500u64.into(), Currency::Eur);
            let mut other_price = HashMap::new();
            other_price.insert(Currency::Usd, 550u64.into());

            let event = make_event(
                &product,
                ProductEventPayload::ProductDomainEvent(ProductDomainEventPayload::PriceChanged(
                    ProductPriceChangeDomainEventPayload {
                        shop_id: product.shop_id,
                        shops_product_id: product.shops_product_id.clone(),
                        old_native_price: None,
                        old_other_price: HashMap::new(),
                        new_native_price: Some(price),
                        new_other_price: other_price.clone(),
                    },
                )),
            );

            product.apply(event);

            assert_eq!(product.native_price, Some(price));
            assert_eq!(product.other_price, other_price);
        }

        #[test]
        fn should_update_price_when_price_dropped_event_for_apply() {
            let mut product: Product = Faker.fake();
            let old_price = Price::new(1000u64.into(), Currency::Eur);
            product.native_price = Some(old_price);

            let new_price = Price::new(800u64.into(), Currency::Eur);
            let mut new_other_price = HashMap::new();
            new_other_price.insert(Currency::Usd, 880u64.into());

            let event = make_event(
                &product,
                ProductEventPayload::ProductDomainEvent(ProductDomainEventPayload::PriceChanged(
                    ProductPriceChangeDomainEventPayload {
                        shop_id: product.shop_id,
                        shops_product_id: product.shops_product_id.clone(),
                        old_native_price: Some(old_price),
                        old_other_price: HashMap::new(),
                        new_native_price: Some(new_price),
                        new_other_price: new_other_price.clone(),
                    },
                )),
            );

            product.apply(event);

            assert_eq!(product.native_price, Some(new_price));
            assert_eq!(product.other_price, new_other_price);
        }

        #[test]
        fn should_clear_price_when_price_removed_event_for_apply() {
            let mut product: Product = Faker.fake();
            let old_price = Price::new(500u64.into(), Currency::Eur);
            product.native_price = Some(old_price);
            product.other_price.insert(Currency::Usd, 550u64.into());

            let event = make_event(
                &product,
                ProductEventPayload::ProductDomainEvent(ProductDomainEventPayload::PriceChanged(
                    ProductPriceChangeDomainEventPayload {
                        shop_id: product.shop_id,
                        shops_product_id: product.shops_product_id.clone(),
                        old_native_price: Some(old_price),
                        old_other_price: product.other_price.clone(),
                        new_native_price: None,
                        new_other_price: HashMap::new(),
                    },
                )),
            );

            product.apply(event);

            assert_eq!(product.native_price, None);
            assert!(product.other_price.is_empty());
        }

        #[test]
        fn should_insert_translated_title_when_translated_title_event_for_apply() {
            let mut product: Product = Faker.fake();
            product.other_title.clear();

            let event = make_event(
                &product,
                ProductEventPayload::ProductEnrichmentEvent(
                    ProductEnrichmentEventPayload::TranslatedTitle(
                        TranslationProductEnrichmentEventPayload {
                            shop_id: product.shop_id,
                            shops_product_id: product.shops_product_id.clone(),
                            source_language: Language::De,
                            target_language: Language::En,
                            target: "English Title".into(),
                        },
                    ),
                ),
            );

            product.apply(event);

            assert_eq!(
                product.other_title.get(&Language::En).cloned(),
                Some("English Title".into())
            );
        }

        #[test]
        fn should_set_embedding_when_embedded_event_for_apply() {
            let mut product: Product = Faker.fake();
            product.embedding = None;

            let embedding = vec![0.1f32, 0.2, 0.3, 0.4];

            let event = make_event(
                &product,
                ProductEventPayload::ProductEnrichmentEvent(
                    ProductEnrichmentEventPayload::Embedded(
                        EmbeddedProductEnrichmentEventPayload {
                            shop_id: product.shop_id,
                            shops_product_id: product.shops_product_id.clone(),
                            embedding: embedding.clone(),
                        },
                    ),
                ),
            );

            product.apply(event);

            assert_eq!(product.embedding, Some(embedding));
        }

        #[test]
        fn should_update_attributes_when_extracted_attributes_event_for_apply() {
            let mut product: Product = Faker.fake();
            product.origin_year = None;
            product.authenticity = Authenticity::default();
            product.condition = Condition::default();

            let year: Year = Faker.fake();
            let authenticity = Authenticity::LaterCopy;
            let condition = Condition::Excellent;
            let provenance = Provenance::Claimed;
            let restoration = Restoration::Major;

            let event = make_event(
                &product,
                ProductEventPayload::ProductEnrichmentEvent(
                    ProductEnrichmentEventPayload::ExtractedAttributes(
                        ExtractedAttributesProductEnrichmentEventPayload {
                            shop_id: product.shop_id,
                            shops_product_id: product.shops_product_id.clone(),
                            origin_year_min: None,
                            origin_year: Some(year),
                            origin_year_max: None,
                            authenticity: Some(authenticity),
                            condition: Some(condition),
                            provenance: Some(provenance),
                            restoration: Some(restoration),
                        },
                    ),
                ),
            );

            product.apply(event);

            assert_eq!(product.origin_year, Some(OriginYear::ExactYear(year)));
            assert_eq!(product.authenticity, authenticity);
            assert_eq!(product.condition, condition);
            assert_eq!(product.provenance, provenance);
            assert_eq!(product.restoration, restoration);
        }

        #[test]
        fn should_set_estimated_range_when_extracted_attributes_with_min_max_for_apply() {
            let mut product: Product = Faker.fake();
            product.origin_year = None;

            let min_year: Year = Faker.fake();
            let max_year: Year = Faker.fake();

            let event = make_event(
                &product,
                ProductEventPayload::ProductEnrichmentEvent(
                    ProductEnrichmentEventPayload::ExtractedAttributes(
                        ExtractedAttributesProductEnrichmentEventPayload {
                            shop_id: product.shop_id,
                            shops_product_id: product.shops_product_id.clone(),
                            origin_year_min: Some(min_year),
                            origin_year: None,
                            origin_year_max: Some(max_year),
                            authenticity: None,
                            condition: None,
                            provenance: None,
                            restoration: None,
                        },
                    ),
                ),
            );

            product.apply(event);

            assert_eq!(
                product.origin_year,
                Some(OriginYear::EstimatedRange(YearRange {
                    min: Some(min_year),
                    max: Some(max_year),
                }))
            );
        }

        #[test]
        fn should_set_category_when_classified_category_event_for_apply() {
            let mut product: Product = Faker.fake();
            product.category_id = None;

            let category_id: CategoryId = Faker.fake();

            let event = make_event(
                &product,
                ProductEventPayload::ProductEnrichmentEvent(
                    ProductEnrichmentEventPayload::ClassifiedCategory(
                        ClassifiedCategoryProductEnrichmentEventPayload {
                            shop_id: product.shop_id,
                            shops_product_id: product.shops_product_id.clone(),
                            category_id: category_id.clone(),
                        },
                    ),
                ),
            );

            product.apply(event);

            assert_eq!(product.category_id, Some(category_id));
        }

        #[test]
        fn should_set_period_when_classified_period_event_for_apply() {
            let mut product: Product = Faker.fake();
            product.period_id = None;

            let period_id: PeriodId = Faker.fake();

            let event = make_event(
                &product,
                ProductEventPayload::ProductEnrichmentEvent(
                    ProductEnrichmentEventPayload::ClassifiedPeriod(
                        ClassifiedPeriodProductEnrichmentEventPayload {
                            shop_id: product.shop_id,
                            shops_product_id: product.shops_product_id.clone(),
                            period_id: period_id.clone(),
                        },
                    ),
                ),
            );

            product.apply(event);

            assert_eq!(product.period_id, Some(period_id));
        }

        #[test]
        fn should_update_images_when_prohibited_content_event_for_apply() {
            let mut product: Product = Faker.fake();
            product.images = vec![
                ProductImage {
                    url: Url::parse("https://example.com/img1.jpg").unwrap(),
                    prohibited_content: ProhibitedContent::Unknown,
                },
                ProductImage {
                    url: Url::parse("https://example.com/img2.jpg").unwrap(),
                    prohibited_content: ProhibitedContent::Unknown,
                },
            ];

            let event = make_event(
                &product,
                ProductEventPayload::ProductPolicyEvent(
                    ProductPolicyEventPayload::ProhibitedContentDecision(
                        ProhibitedContentProductPolicyEventPayload {
                            shop_id: product.shop_id,
                            shops_product_id: product.shops_product_id.clone(),
                            decision: ProhibitedContent::NaziGermany,
                            reason: ProhibitedContentReason::ProductText,
                        },
                    ),
                ),
            );

            product.apply(event);

            assert!(
                product
                    .images
                    .iter()
                    .all(|i| i.prohibited_content == ProhibitedContent::NaziGermany)
            );
        }

        #[test]
        fn should_not_update_images_when_prohibited_content_unknown_for_apply() {
            let mut product: Product = Faker.fake();
            product.images = vec![ProductImage {
                url: Url::parse("https://example.com/img1.jpg").unwrap(),
                prohibited_content: ProhibitedContent::None,
            }];

            let event = make_event(
                &product,
                ProductEventPayload::ProductPolicyEvent(
                    ProductPolicyEventPayload::ProhibitedContentDecision(
                        ProhibitedContentProductPolicyEventPayload {
                            shop_id: product.shop_id,
                            shops_product_id: product.shops_product_id.clone(),
                            decision: ProhibitedContent::Unknown,
                            reason: ProhibitedContentReason::ProductText,
                        },
                    ),
                ),
            );

            product.apply(event);

            assert!(
                product
                    .images
                    .iter()
                    .all(|i| i.prohibited_content == ProhibitedContent::None)
            );
        }
    }

    mod estimate_price {
        use crate::core::product::Product;
        use crate::core::product_event::domain::ProductDomainEventPayload;
        use common::currency::domain::Currency;
        use common::language::domain::Language;
        use common::localized::Localized;
        use common::price::domain::{FixedFxRate, Price};
        use common::product_state::domain::ProductState;
        use fake::{Fake, Faker};
        use time::OffsetDateTime;
        use url::Url;

        fn make_product() -> Product {
            Product {
                product_id: Default::default(),
                product_slug_id: Faker.fake(),
                shop_slug_id: Faker.fake(),
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_product_id: Default::default(),
                shop_name: "Boop".into(),
                shop_type: fake::Faker.fake(),
                category_id: Faker.fake(),
                category_name: Faker.fake(),
                period_id: Faker.fake(),
                period_name: Faker.fake(),
                native_title: Localized {
                    localization: Language::De,
                    payload: "Boop".into(),
                },
                other_title: Default::default(),
                native_description: None,
                other_description: Default::default(),
                native_price: None,
                other_price: Default::default(),
                native_price_estimate_min: None,
                other_price_estimate_min: Default::default(),
                native_price_estimate_max: None,
                other_price_estimate_max: Default::default(),
                state: ProductState::Listed,
                url: Url::parse("https://example.com").unwrap(),
                images: vec![],
                embedding: Some(fake::vec![f32; 768]),
                origin_year: None,
                authenticity: Default::default(),
                condition: Default::default(),
                provenance: Default::default(),
                restoration: Default::default(),
                auction_start: None,
                auction_end: None,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        }

        #[test]
        fn should_return_none_when_estimate_prices_not_provided() {
            let mut product = make_product();
            let result = product.change_estimate_price(None, None, &FixedFxRate());
            assert!(result.is_none());
        }

        #[test]
        fn should_return_none_when_estimate_min_unchanged() {
            let mut product = make_product();
            let price = Price::new(100u64.into(), Currency::Eur);
            product.native_price_estimate_min = Some(price);
            let result = product.change_estimate_price(Some(price), None, &FixedFxRate());
            assert!(result.is_none());
        }

        #[test]
        fn should_return_event_when_estimate_min_changes() {
            let mut product = make_product();
            let new_min = Some(Price::new(200u64.into(), Currency::Eur));
            let result = product.change_estimate_price(new_min, None, &FixedFxRate());
            assert!(result.is_some());
            let event = result.unwrap();
            match event.payload {
                ProductDomainEventPayload::EstimatePriceChanged(p) => {
                    assert_eq!(new_min, p.native_price_estimate_min);
                }
                other => panic!("Expected EstimatePriceChanged but got {:?}", other),
            }
        }

        #[test]
        fn should_return_event_when_estimate_max_changes() {
            let mut product = make_product();
            let new_max = Some(Price::new(500u64.into(), Currency::Eur));
            let result = product.change_estimate_price(None, new_max, &FixedFxRate());
            assert!(result.is_some());
            let event = result.unwrap();
            match event.payload {
                ProductDomainEventPayload::EstimatePriceChanged(p) => {
                    assert_eq!(new_max, p.native_price_estimate_max);
                }
                other => panic!("Expected EstimatePriceChanged but got {:?}", other),
            }
        }

        #[test]
        fn should_return_event_when_both_estimates_change() {
            let mut product = make_product();
            let new_min = Some(Price::new(100u64.into(), Currency::Eur));
            let new_max = Some(Price::new(500u64.into(), Currency::Eur));
            let result = product.change_estimate_price(new_min, new_max, &FixedFxRate());
            assert!(result.is_some());
            let event = result.unwrap();
            match event.payload {
                ProductDomainEventPayload::EstimatePriceChanged(p) => {
                    assert_eq!(new_min, p.native_price_estimate_min);
                    assert_eq!(new_max, p.native_price_estimate_max);
                }
                other => panic!("Expected EstimatePriceChanged but got {:?}", other),
            }
        }

        #[test]
        fn should_update_product_fields_when_estimate_price_changes() {
            let mut product = make_product();
            let new_min = Price::new(100u64.into(), Currency::Eur);
            let new_max = Price::new(500u64.into(), Currency::Eur);
            product.change_estimate_price(Some(new_min), Some(new_max), &FixedFxRate());
            assert_eq!(Some(new_min), product.native_price_estimate_min);
            assert_eq!(Some(new_max), product.native_price_estimate_max);
        }
    }

    mod url {
        use crate::core::product::Product;
        use crate::core::product_event::domain::ProductDomainEventPayload;
        use common::language::domain::Language;
        use common::localized::Localized;
        use common::product_state::domain::ProductState;
        use fake::{Fake, Faker};
        use time::OffsetDateTime;
        use url::Url;

        fn make_product() -> Product {
            Product {
                product_id: Default::default(),
                product_slug_id: Faker.fake(),
                shop_slug_id: Faker.fake(),
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_product_id: Default::default(),
                shop_name: "Boop".into(),
                shop_type: fake::Faker.fake(),
                category_id: Faker.fake(),
                category_name: Faker.fake(),
                period_id: Faker.fake(),
                period_name: Faker.fake(),
                native_title: Localized {
                    localization: Language::De,
                    payload: "Boop".into(),
                },
                other_title: Default::default(),
                native_description: None,
                other_description: Default::default(),
                native_price: None,
                other_price: Default::default(),
                native_price_estimate_min: None,
                other_price_estimate_min: Default::default(),
                native_price_estimate_max: None,
                other_price_estimate_max: Default::default(),
                state: ProductState::Listed,
                url: Url::parse("https://example.com").unwrap(),
                images: vec![],
                embedding: Some(fake::vec![f32; 768]),
                origin_year: None,
                authenticity: Default::default(),
                condition: Default::default(),
                provenance: Default::default(),
                restoration: Default::default(),
                auction_start: None,
                auction_end: None,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        }

        #[test]
        fn should_return_none_when_url_unchanged() {
            let mut product = make_product();
            let same_url = product.url.clone();
            let result = product.change_url(same_url);
            assert!(result.is_none());
        }

        #[test]
        fn should_return_event_when_url_changes() {
            let mut product = make_product();
            let new_url = Url::parse("https://different.example.com").unwrap();
            let result = product.change_url(new_url.clone());
            assert!(result.is_some());
            let event = result.unwrap();
            match event.payload {
                ProductDomainEventPayload::UrlChanged(p) => {
                    assert_eq!(new_url, p.url);
                }
                other => panic!("Expected UrlChanged but got {:?}", other),
            }
        }

        #[test]
        fn should_update_product_url_when_url_changes() {
            let mut product = make_product();
            let new_url = Url::parse("https://different.example.com").unwrap();
            product.change_url(new_url.clone());
            assert_eq!(new_url, product.url);
        }
    }

    mod images {
        use crate::core::product::Product;
        use crate::core::product_event::domain::ProductDomainEventPayload;
        use crate::core::product_image::ProductImage;
        use crate::core::prohibited_content::ProhibitedContent;
        use common::language::domain::Language;
        use common::localized::Localized;
        use common::product_state::domain::ProductState;
        use fake::{Fake, Faker};
        use time::OffsetDateTime;
        use url::Url;

        fn make_product() -> Product {
            Product {
                product_id: Default::default(),
                product_slug_id: Faker.fake(),
                shop_slug_id: Faker.fake(),
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_product_id: Default::default(),
                shop_name: "Boop".into(),
                shop_type: fake::Faker.fake(),
                category_id: Faker.fake(),
                category_name: Faker.fake(),
                period_id: Faker.fake(),
                period_name: Faker.fake(),
                native_title: Localized {
                    localization: Language::De,
                    payload: "Boop".into(),
                },
                other_title: Default::default(),
                native_description: None,
                other_description: Default::default(),
                native_price: None,
                other_price: Default::default(),
                native_price_estimate_min: None,
                other_price_estimate_min: Default::default(),
                native_price_estimate_max: None,
                other_price_estimate_max: Default::default(),
                state: ProductState::Listed,
                url: Url::parse("https://example.com").unwrap(),
                images: vec![],
                embedding: Some(fake::vec![f32; 768]),
                origin_year: None,
                authenticity: Default::default(),
                condition: Default::default(),
                provenance: Default::default(),
                restoration: Default::default(),
                auction_start: None,
                auction_end: None,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        }

        #[test]
        fn should_return_none_when_images_unchanged() {
            let mut product = make_product();
            let same_images = product.images.clone();
            let result = product.change_images(same_images);
            assert!(result.is_none());
        }

        #[test]
        fn should_return_event_when_images_change() {
            let mut product = make_product();
            let new_images = vec![ProductImage {
                url: Url::parse("https://img.example.com/1.jpg").unwrap(),
                prohibited_content: ProhibitedContent::None,
            }];
            let result = product.change_images(new_images.clone());
            assert!(result.is_some());
            let event = result.unwrap();
            match event.payload {
                ProductDomainEventPayload::ImagesChanged(p) => {
                    assert_eq!(new_images, p.images);
                }
                other => panic!("Expected ImagesChanged but got {:?}", other),
            }
        }

        #[test]
        fn should_update_product_images_when_images_change() {
            let mut product = make_product();
            let new_images = vec![ProductImage {
                url: Url::parse("https://img.example.com/1.jpg").unwrap(),
                prohibited_content: ProhibitedContent::None,
            }];
            product.change_images(new_images.clone());
            assert_eq!(new_images, product.images);
        }
    }

    mod auction_time {
        use crate::core::product::Product;
        use crate::core::product_event::domain::ProductDomainEventPayload;
        use common::language::domain::Language;
        use common::localized::Localized;
        use common::product_state::domain::ProductState;
        use fake::{Fake, Faker};
        use time::OffsetDateTime;
        use url::Url;

        fn make_product() -> Product {
            Product {
                product_id: Default::default(),
                product_slug_id: Faker.fake(),
                shop_slug_id: Faker.fake(),
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_product_id: Default::default(),
                shop_name: "Boop".into(),
                shop_type: fake::Faker.fake(),
                category_id: Faker.fake(),
                category_name: Faker.fake(),
                period_id: Faker.fake(),
                period_name: Faker.fake(),
                native_title: Localized {
                    localization: Language::De,
                    payload: "Boop".into(),
                },
                other_title: Default::default(),
                native_description: None,
                other_description: Default::default(),
                native_price: None,
                other_price: Default::default(),
                native_price_estimate_min: None,
                other_price_estimate_min: Default::default(),
                native_price_estimate_max: None,
                other_price_estimate_max: Default::default(),
                state: ProductState::Listed,
                url: Url::parse("https://example.com").unwrap(),
                images: vec![],
                embedding: Some(fake::vec![f32; 768]),
                origin_year: None,
                authenticity: Default::default(),
                condition: Default::default(),
                provenance: Default::default(),
                restoration: Default::default(),
                auction_start: None,
                auction_end: None,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        }

        #[test]
        fn should_return_none_when_no_auction_times_provided() {
            let mut product = make_product();
            let result = product.change_auction_time(None, None);
            assert!(result.is_none());
        }

        #[test]
        fn should_return_none_when_auction_times_unchanged() {
            let mut product = make_product();
            let start = OffsetDateTime::now_utc();
            let end = OffsetDateTime::now_utc() + time::Duration::days(7);
            product.auction_start = Some(start);
            product.auction_end = Some(end);
            let result = product.change_auction_time(Some(start), Some(end));
            assert!(result.is_none());
        }

        #[test]
        fn should_return_event_when_auction_start_changes() {
            let mut product = make_product();
            let new_start = Some(OffsetDateTime::now_utc() + time::Duration::days(1));
            let result = product.change_auction_time(new_start, None);
            assert!(result.is_some());
            let event = result.unwrap();
            match event.payload {
                ProductDomainEventPayload::AuctionTimeChanged(p) => {
                    assert_eq!(new_start, p.auction_start);
                }
                other => panic!("Expected AuctionTimeChanged but got {:?}", other),
            }
        }

        #[test]
        fn should_return_event_when_auction_end_changes() {
            let mut product = make_product();
            let new_end = Some(OffsetDateTime::now_utc() + time::Duration::days(14));
            let result = product.change_auction_time(None, new_end);
            assert!(result.is_some());
            let event = result.unwrap();
            match event.payload {
                ProductDomainEventPayload::AuctionTimeChanged(p) => {
                    assert_eq!(new_end, p.auction_end);
                }
                other => panic!("Expected AuctionTimeChanged but got {:?}", other),
            }
        }

        #[test]
        fn should_return_event_when_both_auction_times_change() {
            let mut product = make_product();
            let new_start = Some(OffsetDateTime::now_utc() + time::Duration::days(1));
            let new_end = Some(OffsetDateTime::now_utc() + time::Duration::days(14));
            let result = product.change_auction_time(new_start, new_end);
            assert!(result.is_some());
            let event = result.unwrap();
            match event.payload {
                ProductDomainEventPayload::AuctionTimeChanged(p) => {
                    assert_eq!(new_start, p.auction_start);
                    assert_eq!(new_end, p.auction_end);
                }
                other => panic!("Expected AuctionTimeChanged but got {:?}", other),
            }
        }
    }

    mod origin_year {
        use crate::core::origin_year::OriginYear;
        use crate::core::product::Product;
        use crate::core::product_event::domain::ProductDomainEventPayload;
        use common::language::domain::Language;
        use common::localized::Localized;
        use common::product_state::domain::ProductState;
        use common::year::Year;
        use fake::{Fake, Faker};
        use time::OffsetDateTime;
        use url::Url;

        fn make_product() -> Product {
            Product {
                product_id: Default::default(),
                product_slug_id: Faker.fake(),
                shop_slug_id: Faker.fake(),
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_product_id: Default::default(),
                shop_name: "Boop".into(),
                shop_type: fake::Faker.fake(),
                category_id: Faker.fake(),
                category_name: Faker.fake(),
                period_id: Faker.fake(),
                period_name: Faker.fake(),
                native_title: Localized {
                    localization: Language::De,
                    payload: "Boop".into(),
                },
                other_title: Default::default(),
                native_description: None,
                other_description: Default::default(),
                native_price: None,
                other_price: Default::default(),
                native_price_estimate_min: None,
                other_price_estimate_min: Default::default(),
                native_price_estimate_max: None,
                other_price_estimate_max: Default::default(),
                state: ProductState::Listed,
                url: Url::parse("https://example.com").unwrap(),
                images: vec![],
                embedding: Some(fake::vec![f32; 768]),
                origin_year: None,
                authenticity: Default::default(),
                condition: Default::default(),
                provenance: Default::default(),
                restoration: Default::default(),
                auction_start: None,
                auction_end: None,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        }

        #[test]
        fn should_return_none_when_origin_year_unchanged() {
            let mut product = make_product();
            let oy = OriginYear::ExactYear(Year::from(1900i32));
            product.origin_year = Some(oy);
            let result = product.change_origin_year(oy);
            assert!(result.is_none());
        }

        #[test]
        fn should_return_event_when_origin_year_changes() {
            let mut product = make_product();
            let old_oy = OriginYear::ExactYear(Year::from(1900i32));
            product.origin_year = Some(old_oy);
            let new_oy = OriginYear::ExactYear(Year::from(1800i32));
            let result = product.change_origin_year(new_oy);
            assert!(result.is_some());
            let event = result.unwrap();
            match event.payload {
                ProductDomainEventPayload::OriginYearChanged(p) => {
                    assert_eq!(new_oy, p.origin_year);
                }
                other => panic!("Expected OriginYearChanged but got {:?}", other),
            }
        }

        #[test]
        fn should_return_event_when_origin_year_set_from_none() {
            let mut product = make_product();
            assert!(product.origin_year.is_none());
            let new_oy = OriginYear::ExactYear(Year::from(1900i32));
            let result = product.change_origin_year(new_oy);
            assert!(result.is_some());
            let event = result.unwrap();
            match event.payload {
                ProductDomainEventPayload::OriginYearChanged(p) => {
                    assert_eq!(new_oy, p.origin_year);
                }
                other => panic!("Expected OriginYearChanged but got {:?}", other),
            }
        }
    }

    mod authenticity {
        use crate::core::authenticity::Authenticity;
        use crate::core::product::Product;
        use crate::core::product_event::domain::ProductDomainEventPayload;
        use common::language::domain::Language;
        use common::localized::Localized;
        use common::product_state::domain::ProductState;
        use fake::{Fake, Faker};
        use time::OffsetDateTime;
        use url::Url;

        fn make_product() -> Product {
            Product {
                product_id: Default::default(),
                product_slug_id: Faker.fake(),
                shop_slug_id: Faker.fake(),
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_product_id: Default::default(),
                shop_name: "Boop".into(),
                shop_type: fake::Faker.fake(),
                category_id: Faker.fake(),
                category_name: Faker.fake(),
                period_id: Faker.fake(),
                period_name: Faker.fake(),
                native_title: Localized {
                    localization: Language::De,
                    payload: "Boop".into(),
                },
                other_title: Default::default(),
                native_description: None,
                other_description: Default::default(),
                native_price: None,
                other_price: Default::default(),
                native_price_estimate_min: None,
                other_price_estimate_min: Default::default(),
                native_price_estimate_max: None,
                other_price_estimate_max: Default::default(),
                state: ProductState::Listed,
                url: Url::parse("https://example.com").unwrap(),
                images: vec![],
                embedding: Some(fake::vec![f32; 768]),
                origin_year: None,
                authenticity: Default::default(),
                condition: Default::default(),
                provenance: Default::default(),
                restoration: Default::default(),
                auction_start: None,
                auction_end: None,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        }

        #[test]
        fn should_return_none_when_authenticity_unchanged() {
            let mut product = make_product();
            let result = product.change_authenticity(product.authenticity);
            assert!(result.is_none());
        }

        #[test]
        fn should_return_event_when_authenticity_changes() {
            let mut product = make_product();
            product.authenticity = Authenticity::Unknown;
            let new_auth = Authenticity::Original;
            let result = product.change_authenticity(new_auth);
            assert!(result.is_some());
            let event = result.unwrap();
            match event.payload {
                ProductDomainEventPayload::AuthenticityChanged(p) => {
                    assert_eq!(new_auth, p.authenticity);
                }
                other => panic!("Expected AuthenticityChanged but got {:?}", other),
            }
        }
    }

    mod condition {
        use crate::core::condition::Condition;
        use crate::core::product::Product;
        use crate::core::product_event::domain::ProductDomainEventPayload;
        use common::language::domain::Language;
        use common::localized::Localized;
        use common::product_state::domain::ProductState;
        use fake::{Fake, Faker};
        use time::OffsetDateTime;
        use url::Url;

        fn make_product() -> Product {
            Product {
                product_id: Default::default(),
                product_slug_id: Faker.fake(),
                shop_slug_id: Faker.fake(),
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_product_id: Default::default(),
                shop_name: "Boop".into(),
                shop_type: fake::Faker.fake(),
                category_id: Faker.fake(),
                category_name: Faker.fake(),
                period_id: Faker.fake(),
                period_name: Faker.fake(),
                native_title: Localized {
                    localization: Language::De,
                    payload: "Boop".into(),
                },
                other_title: Default::default(),
                native_description: None,
                other_description: Default::default(),
                native_price: None,
                other_price: Default::default(),
                native_price_estimate_min: None,
                other_price_estimate_min: Default::default(),
                native_price_estimate_max: None,
                other_price_estimate_max: Default::default(),
                state: ProductState::Listed,
                url: Url::parse("https://example.com").unwrap(),
                images: vec![],
                embedding: Some(fake::vec![f32; 768]),
                origin_year: None,
                authenticity: Default::default(),
                condition: Default::default(),
                provenance: Default::default(),
                restoration: Default::default(),
                auction_start: None,
                auction_end: None,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        }

        #[test]
        fn should_return_none_when_condition_unchanged() {
            let mut product = make_product();
            let result = product.change_condition(product.condition);
            assert!(result.is_none());
        }

        #[test]
        fn should_return_event_when_condition_changes() {
            let mut product = make_product();
            product.condition = Condition::Unknown;
            let new_cond = Condition::Excellent;
            let result = product.change_condition(new_cond);
            assert!(result.is_some());
            let event = result.unwrap();
            match event.payload {
                ProductDomainEventPayload::ConditionChanged(p) => {
                    assert_eq!(new_cond, p.condition);
                }
                other => panic!("Expected ConditionChanged but got {:?}", other),
            }
        }
    }

    mod provenance {
        use crate::core::product::Product;
        use crate::core::product_event::domain::ProductDomainEventPayload;
        use crate::core::provenance::Provenance;
        use common::language::domain::Language;
        use common::localized::Localized;
        use common::product_state::domain::ProductState;
        use fake::{Fake, Faker};
        use time::OffsetDateTime;
        use url::Url;

        fn make_product() -> Product {
            Product {
                product_id: Default::default(),
                product_slug_id: Faker.fake(),
                shop_slug_id: Faker.fake(),
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_product_id: Default::default(),
                shop_name: "Boop".into(),
                shop_type: fake::Faker.fake(),
                category_id: Faker.fake(),
                category_name: Faker.fake(),
                period_id: Faker.fake(),
                period_name: Faker.fake(),
                native_title: Localized {
                    localization: Language::De,
                    payload: "Boop".into(),
                },
                other_title: Default::default(),
                native_description: None,
                other_description: Default::default(),
                native_price: None,
                other_price: Default::default(),
                native_price_estimate_min: None,
                other_price_estimate_min: Default::default(),
                native_price_estimate_max: None,
                other_price_estimate_max: Default::default(),
                state: ProductState::Listed,
                url: Url::parse("https://example.com").unwrap(),
                images: vec![],
                embedding: Some(fake::vec![f32; 768]),
                origin_year: None,
                authenticity: Default::default(),
                condition: Default::default(),
                provenance: Default::default(),
                restoration: Default::default(),
                auction_start: None,
                auction_end: None,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        }

        #[test]
        fn should_return_none_when_provenance_unchanged() {
            let mut product = make_product();
            let result = product.change_provenance(product.provenance);
            assert!(result.is_none());
        }

        #[test]
        fn should_return_event_when_provenance_changes() {
            let mut product = make_product();
            product.provenance = Provenance::Unknown;
            let new_prov = Provenance::Complete;
            let result = product.change_provenance(new_prov);
            assert!(result.is_some());
            let event = result.unwrap();
            match event.payload {
                ProductDomainEventPayload::ProvenanceChanged(p) => {
                    assert_eq!(new_prov, p.provenance);
                }
                other => panic!("Expected ProvenanceChanged but got {:?}", other),
            }
        }
    }

    mod restoration {
        use crate::core::product::Product;
        use crate::core::product_event::domain::ProductDomainEventPayload;
        use crate::core::restoration::Restoration;
        use common::language::domain::Language;
        use common::localized::Localized;
        use common::product_state::domain::ProductState;
        use fake::{Fake, Faker};
        use time::OffsetDateTime;
        use url::Url;

        fn make_product() -> Product {
            Product {
                product_id: Default::default(),
                product_slug_id: Faker.fake(),
                shop_slug_id: Faker.fake(),
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_product_id: Default::default(),
                shop_name: "Boop".into(),
                shop_type: fake::Faker.fake(),
                category_id: Faker.fake(),
                category_name: Faker.fake(),
                period_id: Faker.fake(),
                period_name: Faker.fake(),
                native_title: Localized {
                    localization: Language::De,
                    payload: "Boop".into(),
                },
                other_title: Default::default(),
                native_description: None,
                other_description: Default::default(),
                native_price: None,
                other_price: Default::default(),
                native_price_estimate_min: None,
                other_price_estimate_min: Default::default(),
                native_price_estimate_max: None,
                other_price_estimate_max: Default::default(),
                state: ProductState::Listed,
                url: Url::parse("https://example.com").unwrap(),
                images: vec![],
                embedding: Some(fake::vec![f32; 768]),
                origin_year: None,
                authenticity: Default::default(),
                condition: Default::default(),
                provenance: Default::default(),
                restoration: Default::default(),
                auction_start: None,
                auction_end: None,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        }

        #[test]
        fn should_return_none_when_restoration_unchanged() {
            let mut product = make_product();
            let result = product.change_restoration(product.restoration);
            assert!(result.is_none());
        }

        #[test]
        fn should_return_event_when_restoration_changes() {
            let mut product = make_product();
            product.restoration = Restoration::Unknown;
            let new_rest = Restoration::Minor;
            let result = product.change_restoration(new_rest);
            assert!(result.is_some());
            let event = result.unwrap();
            match event.payload {
                ProductDomainEventPayload::RestorationChanged(p) => {
                    assert_eq!(new_rest, p.restoration);
                }
                other => panic!("Expected RestorationChanged but got {:?}", other),
            }
        }
    }
}
