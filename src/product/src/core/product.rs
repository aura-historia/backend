use crate::core::description::Description;
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
use crate::core::product_event::policy::{
    ProductPolicyEventPayload, ProhibitedContentProductPolicyEventPayload,
};
use crate::core::product_event::{
    ProductDomainEvent, ProductEnrichmentEvent, ProductEvent, ProductEventPayload,
    ProductPolicyEvent,
};
use crate::core::product_image::ProductImage;
use crate::core::prohibited_content::{ProhibitedContent, ProhibitedContentReason};
use crate::core::title::Title;
use common::currency::domain::Currency;
use common::event::Event;
use common::event_id::EventId;
use common::has_key::HasKey;
use common::language::domain::Language;
use common::localized::Localized;
use common::price::domain::{FxRate, MonetaryAmount, Price};
use common::product_id::{ProductId, ProductKey};
use common::product_state::domain::ProductState;
use common::shop_id::ShopId;
use common::shop_name::ShopName;
use common::shops_product_id::ShopsProductId;
use common::slug_id::SlugId;
use geo::core::address::{GeoAddress, StructuredAddress};
use shop::core::shop_type::ShopType;
use std::collections::HashMap;
use time::OffsetDateTime;
use tracing::{error, warn};
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct Product {
    pub product_id: ProductId,
    pub product_slug_id: SlugId<6>,
    pub shop_slug_id: SlugId<0>,
    pub seller_slug_id: SlugId<0>,
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
    pub url: Url,
    pub images: Vec<ProductImage>,
    pub embedding: Option<Vec<f32>>,
    pub auction_start: Option<OffsetDateTime>,
    pub auction_end: Option<OffsetDateTime>,
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
        images: Vec<ProductImage>,
        auction_start: Option<OffsetDateTime>,
        auction_end: Option<OffsetDateTime>,
    ) -> ProductDomainEvent {
        let payload = ProductCreatedDomainEventPayload {
            product_slug_id: SlugId::from(native_title.payload.as_ref()),
            shop_slug_id: SlugId::from(shop_name.as_ref()),
            seller_slug_id: SlugId::from(seller_name.as_ref()),
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

    pub fn remove_price(&mut self) -> Option<ProductDomainEvent> {
        match self.native_price {
            Some(old_native_price) => {
                self.native_price = None;
                let old_other_price = self.other_price.drain().collect();
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
                seller_id: self.seller_id,
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
                    seller_id: self.seller_id,
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
                        native_title: Some(self.native_title.payload.clone()),
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
                for image in &mut self.images {
                    image.prohibited_content = prohibited_content;
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
            url: self.url,
            images: self.images,
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
        let mut descriptions: HashMap<Language, Description> = HashMap::new();
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
    pub seller_slug_id: SlugId<0>,
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
    pub url: Url,
    pub images: Vec<ProductImage>,
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
            let native_title: Localized<Language, Title> = config.fake_with_rng(rng);
            let shop_name: ShopName = config.fake_with_rng(rng);
            let seller_name: ShopName = config.fake_with_rng(rng);
            Product {
                product_id: config.fake_with_rng(rng),
                product_slug_id: SlugId::from(native_title.payload.as_ref()),
                shop_slug_id: SlugId::from(shop_name.as_ref()),
                seller_slug_id: SlugId::from(seller_name.as_ref()),
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
                url: Url::parse("https://www.example.com/product").unwrap(),
                images: config.fake_with_rng(rng),
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
