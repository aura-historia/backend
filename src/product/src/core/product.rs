use crate::core::authenticity::Authenticity;
use crate::core::condition::Condition;
use crate::core::description::Description;
use crate::core::origin_year::OriginYear;
use crate::core::product_event::{
    LocalizedProductEventPayloadView, ProductCreatedEventPayload, ProductEvent,
    ProductEventPayload, ProductPriceChangeEventPayload, ProductPriceDiscoveryEventPayload,
    ProductPriceRemovedEventPayload, ProductStateChangeEventPayload,
};
use crate::core::product_image::ProductImage;
use crate::core::provenance::Provenance;
use crate::core::restoration::Restoration;
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
use shop::core::shop_type::ShopType;
use std::collections::HashMap;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct Product {
    pub product_id: ProductId,
    pub event_id: EventId,
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub shop_name: ShopName,
    pub shop_type: ShopType,
    pub native_title: Localized<Language, Title>,
    pub other_title: HashMap<Language, Title>,
    pub native_description: Option<Localized<Language, Description>>,
    pub other_description: HashMap<Language, Description>,
    pub native_price: Option<Price>,
    pub other_price: HashMap<Currency, MonetaryAmount>,
    pub state: ProductState,
    pub url: Url,
    pub images: Vec<ProductImage>,
    pub origin_year: Option<OriginYear>,
    pub authenticity: Option<Authenticity>,
    pub condition: Option<Condition>,
    pub provenance: Option<Provenance>,
    pub restoration: Option<Restoration>,
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
        state: ProductState,
        url: Url,
        images: Vec<ProductImage>,
    ) -> ProductEvent {
        let payload = ProductCreatedEventPayload {
            shop_id,
            shops_product_id,
            shop_name,
            shop_type,
            native_title,
            native_description,
            native_price,
            other_price,
            state,
            url,
            images,
        };
        ProductEvent {
            aggregate_id: ProductId::new(),
            event_id: EventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ProductEventPayload::Created(payload),
        }
    }

    pub fn change_state(&mut self, new_state: ProductState) -> Option<ProductEvent> {
        if self.state == new_state {
            None
        } else {
            let old_state = self.state;
            self.state = new_state;
            let event_payload_constructor = match new_state {
                ProductState::Listed => ProductEventPayload::StateListed,
                ProductState::Available => ProductEventPayload::StateAvailable,
                ProductState::Reserved => ProductEventPayload::StateReserved,
                ProductState::Sold => ProductEventPayload::StateSold,
                ProductState::Removed => ProductEventPayload::StateRemoved,
                ProductState::Unknown => ProductEventPayload::StateUnknown,
            };
            let event = Event {
                aggregate_id: self.product_id,
                event_id: EventId::new(),
                timestamp: OffsetDateTime::now_utc(),
                payload: event_payload_constructor(ProductStateChangeEventPayload {
                    shop_id: self.shop_id,
                    shops_product_id: self.shops_product_id.clone(),
                    old_state,
                }),
            };
            Some(event)
        }
    }

    pub fn change_price(
        &mut self,
        new_native_price: Price,
        fx_rate: &impl FxRate,
    ) -> Option<ProductEvent> {
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
                    payload: ProductEventPayload::PriceDiscovered(
                        ProductPriceDiscoveryEventPayload {
                            shop_id: self.shop_id,
                            shops_product_id: self.shops_product_id.clone(),
                            native_price: new_native_price,
                            other_price: new_other_price,
                        },
                    ),
                };
                Some(event)
            }
            Some(old_native_price) => {
                let old_price_for_new_currency = old_native_price
                    .into_exchanged(fx_rate, new_native_price.currency)
                    .unwrap_or(old_native_price);
                let payload = ProductPriceChangeEventPayload {
                    shop_id: self.shop_id,
                    shops_product_id: self.shops_product_id.clone(),
                    new_native_price,
                    new_other_price,
                    old_native_price,
                    old_other_price,
                };
                if old_price_for_new_currency.monetary_amount < new_native_price.monetary_amount {
                    let event = Event {
                        aggregate_id: self.product_id,
                        event_id: EventId::new(),
                        timestamp: OffsetDateTime::now_utc(),
                        payload: ProductEventPayload::PriceIncreased(payload),
                    };
                    Some(event)
                } else if old_price_for_new_currency.monetary_amount
                    > new_native_price.monetary_amount
                {
                    let event = Event {
                        aggregate_id: self.product_id,
                        event_id: EventId::new(),
                        timestamp: OffsetDateTime::now_utc(),
                        payload: ProductEventPayload::PriceDropped(payload),
                    };
                    Some(event)
                } else {
                    None
                }
            }
        }
    }

    pub fn remove_price(&mut self) -> Option<ProductEvent> {
        match self.native_price {
            Some(old_native_price) => {
                self.native_price = None;
                let old_other_price = self.other_price.drain().collect();
                let payload = ProductPriceRemovedEventPayload {
                    shop_id: self.shop_id,
                    shops_product_id: self.shops_product_id.clone(),
                    old_native_price,
                    old_other_price,
                };
                let event = Event {
                    aggregate_id: self.product_id,
                    event_id: EventId::new(),
                    timestamp: OffsetDateTime::now_utc(),
                    payload: ProductEventPayload::PriceRemoved(payload),
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
    ) -> Option<ProductEvent> {
        match new_price_opt {
            Some(new_price) => self.change_price(new_price, fx_rate),
            None => self.remove_price(),
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
    pub event_id: EventId,
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub shop_name: ShopName,
    pub shop_type: ShopType,
    pub title: Localized<Language, Title>,
    pub description: Option<Localized<Language, Description>>,
    pub price: Option<Price>,
    pub state: ProductState,
    pub url: Url,
    pub images: Vec<ProductImage>,
    pub origin_year: Option<OriginYear>,
    pub authenticity: Option<Authenticity>,
    pub condition: Option<Condition>,
    pub provenance: Option<Provenance>,
    pub restoration: Option<Restoration>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
    pub history: Option<Vec<Event<ProductId, LocalizedProductEventPayloadView>>>,
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use common::price::domain::FixedFxRate;
    use fake::{Dummy, Fake, Faker, Rng};

    impl Dummy<Faker> for Product {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let native_price: Option<Price> = config.fake_with_rng(rng);
            let other_price = match native_price {
                None => HashMap::new(),
                Some(price) => FixedFxRate()
                    .exchange_all(price.currency, price.monetary_amount)
                    .unwrap(),
            };
            let state = config.fake_with_rng(rng);
            Product {
                product_id: config.fake_with_rng(rng),
                event_id: config.fake_with_rng(rng),
                shop_id: config.fake_with_rng(rng),
                shops_product_id: config.fake_with_rng(rng),
                shop_name: config.fake_with_rng(rng),
                shop_type: config.fake_with_rng(rng),
                native_title: config.fake_with_rng(rng),
                other_title: config.fake_with_rng(rng),
                native_description: config.fake_with_rng(rng),
                other_description: config.fake_with_rng(rng),
                native_price,
                other_price,
                state,
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
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        }
    }

    impl Dummy<Faker> for LocalizedProductView {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            LocalizedProductView {
                product_id: config.fake_with_rng(rng),
                event_id: config.fake_with_rng(rng),
                shop_id: config.fake_with_rng(rng),
                shops_product_id: config.fake_with_rng(rng),
                shop_name: config.fake_with_rng(rng),
                shop_type: config.fake_with_rng(rng),
                title: config.fake_with_rng(rng),
                description: config.fake_with_rng(rng),
                price: config.fake_with_rng(rng),
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
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
                history: None,
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
        use fake::Fake;
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
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_product_id: Default::default(),
                shop_name: "Boop".into(),
                shop_type: fake::Faker.fake(),
                native_title: Localized {
                    localization: Language::De,
                    payload: "Boop".into(),
                },
                other_title: Default::default(),
                native_description: None,
                other_description: Default::default(),
                native_price: None,
                other_price: Default::default(),
                state: from_state,
                url: Url::parse("https://example.com").unwrap(),
                images: vec![],
                origin_year: None,
                authenticity: None,
                condition: None,
                provenance: None,
                restoration: None,
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
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_product_id: Default::default(),
                shop_name: "Boop".into(),
                shop_type: fake::Faker.fake(),
                native_title: Localized {
                    localization: Language::De,
                    payload: "Boop".into(),
                },
                other_title: Default::default(),
                native_description: None,
                other_description: Default::default(),
                native_price: None,
                other_price: Default::default(),
                state: from_state,
                url: Url::parse("https://example.com").unwrap(),
                images: vec![],
                origin_year: None,
                authenticity: None,
                condition: None,
                provenance: None,
                restoration: None,
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
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_product_id: Default::default(),
                shop_name: "Boop".into(),
                shop_type: fake::Faker.fake(),
                native_title: Localized {
                    localization: Language::De,
                    payload: "Boop".into(),
                },
                other_title: Default::default(),
                native_description: None,
                other_description: Default::default(),
                native_price: None,
                other_price: Default::default(),
                state: from_state,
                url: Url::parse("https://example.com").unwrap(),
                images: vec![],
                origin_year: None,
                authenticity: None,
                condition: None,
                provenance: None,
                restoration: None,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };

            let _ = product.change_state(to_state).unwrap();
            assert_eq!(to_state, product.state);
        }
    }

    mod price {
        use crate::core::product::Product;
        use crate::core::product_event::ProductEventPayload;
        use common::currency::domain::Currency;
        use common::language::domain::Language;
        use common::localized::Localized;
        use common::price::domain::{FxRate, MonetaryAmount, Price};
        use common::product_state::domain::ProductState;
        use fake::Fake;
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
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_product_id: Default::default(),
                shop_name: "Boop".into(),
                shop_type: fake::Faker.fake(),
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
                state: ProductState::Listed,
                url: Url::parse("https://example.com").unwrap(),
                images: vec![],
                origin_year: None,
                authenticity: None,
                condition: None,
                provenance: None,
                restoration: None,
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
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_product_id: Default::default(),
                shop_name: "Boop".into(),
                shop_type: fake::Faker.fake(),
                native_title: Localized {
                    localization: Language::De,
                    payload: "Boop".into(),
                },
                other_title: Default::default(),
                native_description: None,
                other_description: Default::default(),
                native_price: None,
                other_price: Default::default(),
                state: ProductState::Listed,
                url: Url::parse("https://example.com").unwrap(),
                images: vec![],
                origin_year: None,
                authenticity: None,
                condition: None,
                provenance: None,
                restoration: None,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };
            let actual = product.new_price(Some(to_price), &IdentityFxRate).unwrap();

            match actual.payload {
                ProductEventPayload::PriceDiscovered(payload) => {
                    assert_eq!(to_price, payload.native_price);
                    assert!(
                        payload
                            .other_price
                            .iter()
                            .all(|(_, amount)| &to_price.monetary_amount == amount)
                    );
                    assert_eq!(product.native_price, Some(to_price));
                    assert!(
                        product
                            .other_price
                            .iter()
                            .all(|(_, amount)| &to_price.monetary_amount == amount)
                    );
                }
                _ => panic!("Expected ProductEventPayload::PriceDiscovered"),
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
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_product_id: Default::default(),
                shop_name: "Boop".into(),
                shop_type: fake::Faker.fake(),
                native_title: Localized {
                    localization: Language::De,
                    payload: "Boop".into(),
                },
                other_title: Default::default(),
                native_description: None,
                other_description: Default::default(),
                native_price: Some(Price::new(700u64.into(), Currency::Eur)),
                other_price: Default::default(),
                state: ProductState::Listed,
                url: Url::parse("https://example.com").unwrap(),
                images: vec![],
                origin_year: None,
                authenticity: None,
                condition: None,
                provenance: None,
                restoration: None,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };
            let actual = product.new_price(Some(to_price), &IdentityFxRate).unwrap();

            match actual.payload {
                ProductEventPayload::PriceDropped(payload) => {
                    assert_eq!(to_price, payload.new_native_price);
                    assert!(
                        payload
                            .new_other_price
                            .iter()
                            .all(|(_, amount)| &to_price.monetary_amount == amount)
                    );
                    assert_eq!(
                        Price::new(700u64.into(), Currency::Eur),
                        payload.old_native_price
                    );
                    assert_eq!(product.native_price, Some(to_price));
                    assert!(
                        product
                            .other_price
                            .iter()
                            .all(|(_, amount)| &to_price.monetary_amount == amount)
                    );
                }
                _ => panic!("Expected ProductEventPayload::PriceDropped"),
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
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_product_id: Default::default(),
                shop_name: "Boop".into(),
                shop_type: fake::Faker.fake(),
                native_title: Localized {
                    localization: Language::De,
                    payload: "Boop".into(),
                },
                other_title: Default::default(),
                native_description: None,
                other_description: Default::default(),
                native_price: Some(Price::new(169u64.into(), Currency::Eur)),
                other_price: Default::default(),
                state: ProductState::Listed,
                url: Url::parse("https://example.com").unwrap(),
                images: vec![],
                origin_year: None,
                authenticity: None,
                condition: None,
                provenance: None,
                restoration: None,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };
            let actual = product.new_price(Some(to_price), &IdentityFxRate).unwrap();

            match actual.payload {
                ProductEventPayload::PriceIncreased(payload) => {
                    assert_eq!(to_price, payload.new_native_price);
                    assert!(
                        payload
                            .new_other_price
                            .iter()
                            .all(|(_, amount)| &to_price.monetary_amount == amount)
                    );
                    assert_eq!(
                        Price::new(169u64.into(), Currency::Eur),
                        payload.old_native_price
                    );
                    assert_eq!(product.native_price, Some(to_price));
                }
                _ => panic!("Expected ProductEventPayload::PriceIncreased"),
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
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_product_id: Default::default(),
                shop_type: fake::Faker.fake(),
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
                state: ProductState::Listed,
                url: Url::parse("https://example.com").unwrap(),
                images: vec![],
                origin_year: None,
                authenticity: None,
                condition: None,
                provenance: None,
                restoration: None,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };
            let actual = product.new_price(None, &IdentityFxRate).unwrap();

            match actual.payload {
                ProductEventPayload::PriceRemoved(payload) => {
                    assert!(product.native_price.is_none());
                    assert!(product.other_price.is_empty());
                    assert_eq!(price, payload.old_native_price);
                }
                _ => panic!("Expected ProductEventPayload::PriceRemoved"),
            }
        }
    }
}
