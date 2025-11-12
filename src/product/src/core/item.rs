use crate::core::description::Description;
use crate::core::product_event::{
    ItemCreatedEventPayload, ProductEvent, ItemEventPayload, ItemPriceChangeEventPayload,
    ItemPriceDiscoveryEventPayload, ItemPriceRemovedEventPayload, ItemStateChangeEventPayload,
    LocalizedItemEventPayloadView,
};
use crate::core::title::Title;
use common::currency::domain::Currency;
use common::event::Event;
use common::event_id::EventId;
use common::has_key::HasKey;
use common::product_id::{ProductId, ProductKey};
use common::product_state::domain::ProductState;
use common::language::domain::Language;
use common::localized::Localized;
use common::price::domain::{FxRate, MonetaryAmount, Price};
use common::shop_id::ShopId;
use common::shop_name::ShopName;
use common::shops_product_id::ShopsProductId;
use std::collections::HashMap;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub product_id: ProductId,
    pub event_id: EventId,
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub shop_name: ShopName,
    pub native_title: Localized<Language, Title>,
    pub other_title: HashMap<Language, Title>,
    pub native_description: Option<Localized<Language, Description>>,
    pub other_description: HashMap<Language, Description>,
    pub native_price: Option<Price>,
    pub other_price: HashMap<Currency, MonetaryAmount>,
    pub state: ProductState,
    pub url: Url,
    pub images: Vec<Url>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

impl Item {
    #[allow(clippy::too_many_arguments)]
    pub fn create(
        shop_id: ShopId,
        shops_product_id: ShopsProductId,
        shop_name: ShopName,
        native_title: Localized<Language, Title>,
        other_title: HashMap<Language, Title>,
        native_description: Option<Localized<Language, Description>>,
        other_description: HashMap<Language, Description>,
        native_price: Option<Price>,
        other_price: HashMap<Currency, MonetaryAmount>,
        state: ProductState,
        url: Url,
        images: Vec<Url>,
    ) -> ProductEvent {
        let payload = ItemCreatedEventPayload {
            shop_id,
            shops_product_id,
            shop_name,
            native_title,
            other_title,
            native_description,
            native_price,
            other_price,
            state,
            url,
            images,
            other_description,
        };
        ProductEvent {
            aggregate_id: ProductId::new(),
            event_id: EventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ItemEventPayload::Created(payload),
        }
    }

    pub fn change_state(&mut self, new_state: ProductState) -> Option<ProductEvent> {
        if self.state == new_state {
            None
        } else {
            let old_state = self.state;
            self.state = new_state;
            let event_payload_constructor = match new_state {
                ProductState::Listed => ItemEventPayload::StateListed,
                ProductState::Available => ItemEventPayload::StateAvailable,
                ProductState::Reserved => ItemEventPayload::StateReserved,
                ProductState::Sold => ItemEventPayload::StateSold,
                ProductState::Removed => ItemEventPayload::StateRemoved,
                ProductState::Unknown => ItemEventPayload::StateUnknown,
            };
            let event = Event {
                aggregate_id: self.product_id,
                event_id: EventId::new(),
                timestamp: OffsetDateTime::now_utc(),
                payload: event_payload_constructor(ItemStateChangeEventPayload {
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
                    payload: ItemEventPayload::PriceDiscovered(ItemPriceDiscoveryEventPayload {
                        shop_id: self.shop_id,
                        shops_product_id: self.shops_product_id.clone(),
                        native_price: new_native_price,
                        other_price: new_other_price,
                    }),
                };
                Some(event)
            }
            Some(old_native_price) => {
                let old_price_for_new_currency = old_native_price
                    .into_exchanged(fx_rate, new_native_price.currency)
                    .unwrap_or(old_native_price);
                let payload = ItemPriceChangeEventPayload {
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
                        payload: ItemEventPayload::PriceIncreased(payload),
                    };
                    Some(event)
                } else if old_price_for_new_currency.monetary_amount
                    > new_native_price.monetary_amount
                {
                    let event = Event {
                        aggregate_id: self.product_id,
                        event_id: EventId::new(),
                        timestamp: OffsetDateTime::now_utc(),
                        payload: ItemEventPayload::PriceDropped(payload),
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
                let payload = ItemPriceRemovedEventPayload {
                    shop_id: self.shop_id,
                    shops_product_id: self.shops_product_id.clone(),
                    old_native_price,
                    old_other_price,
                };
                let event = Event {
                    aggregate_id: self.product_id,
                    event_id: EventId::new(),
                    timestamp: OffsetDateTime::now_utc(),
                    payload: ItemEventPayload::PriceRemoved(payload),
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

impl HasKey for Item {
    type Key = ProductKey;

    fn key(&self) -> Self::Key {
        ProductKey {
            shop_id: self.shop_id,
            shops_product_id: self.shops_product_id.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalizedItemView {
    pub product_id: ProductId,
    pub event_id: EventId,
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub shop_name: ShopName,
    pub title: Localized<Language, Title>,
    pub description: Option<Localized<Language, Description>>,
    pub price: Option<Price>,
    pub state: ProductState,
    pub url: Url,
    pub images: Vec<Url>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
    pub history: Option<Vec<Event<ProductId, LocalizedItemEventPayloadView>>>,
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use common::price::domain::FixedFxRate;
    use fake::{Dummy, Fake, Faker, Rng};

    impl Dummy<Faker> for Item {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let native_price: Option<Price> = config.fake_with_rng(rng);
            let other_price = match native_price {
                None => HashMap::new(),
                Some(price) => FixedFxRate()
                    .exchange_all(price.currency, price.monetary_amount)
                    .unwrap(),
            };
            let state = config.fake_with_rng(rng);
            Item {
                product_id: config.fake_with_rng(rng),
                event_id: config.fake_with_rng(rng),
                shop_id: config.fake_with_rng(rng),
                shops_product_id: config.fake_with_rng(rng),
                shop_name: config.fake_with_rng(rng),
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
                images: vec![
                    Url::parse(&format!(
                        "https://foo.bar/images/{}",
                        config.fake_with_rng::<u16, _>(rng)
                    ))
                    .unwrap(),
                    Url::parse(&format!(
                        "https://foo.bar/images/{}",
                        config.fake_with_rng::<u16, _>(rng)
                    ))
                    .unwrap(),
                    Url::parse(&format!(
                        "https://foo.bar/images/{}",
                        config.fake_with_rng::<u16, _>(rng)
                    ))
                    .unwrap(),
                ],
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        }
    }

    impl Dummy<Faker> for LocalizedItemView {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            LocalizedItemView {
                product_id: config.fake_with_rng(rng),
                event_id: config.fake_with_rng(rng),
                shop_id: config.fake_with_rng(rng),
                shops_product_id: config.fake_with_rng(rng),
                shop_name: config.fake_with_rng(rng),
                title: config.fake_with_rng(rng),
                description: config.fake_with_rng(rng),
                price: config.fake_with_rng(rng),
                state: config.fake_with_rng(rng),
                url: Url::parse(&format!(
                    "https://foo.bar/item/{}",
                    config.fake_with_rng::<u16, _>(rng)
                ))
                .unwrap(),
                images: vec![
                    Url::parse(&format!(
                        "https://foo.bar/images/{}",
                        config.fake_with_rng::<u16, _>(rng)
                    ))
                    .unwrap(),
                    Url::parse(&format!(
                        "https://foo.bar/images/{}",
                        config.fake_with_rng::<u16, _>(rng)
                    ))
                    .unwrap(),
                    Url::parse(&format!(
                        "https://foo.bar/images/{}",
                        config.fake_with_rng::<u16, _>(rng)
                    ))
                    .unwrap(),
                ],
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
                history: None,
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::core::item::{Item, LocalizedItemView};
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_item() {
            let _ = Faker.fake::<Item>();
        }

        #[test]
        fn should_fake_localized_item_view() {
            let _ = Faker.fake::<LocalizedItemView>();
        }
    }
}

#[cfg(test)]
mod tests {
    mod state {
        use crate::core::item::Item;
        use common::product_state::domain::ProductState;
        use common::language::domain::Language;
        use common::localized::Localized;
        use time::OffsetDateTime;
        use url::Url;

        #[rstest::rstest]
        #[case::listed(ProductState::Listed, ProductState::Listed)]
        #[case::available(ProductState::Available, ProductState::Available)]
        #[case::reserved(ProductState::Reserved, ProductState::Reserved)]
        #[case::sold(ProductState::Sold, ProductState::Sold)]
        #[case::removed(ProductState::Removed, ProductState::Removed)]
        #[case::unknown(ProductState::Unknown, ProductState::Unknown)]
        fn should_return_none_when_state_did_not_change_for_change_state(
            #[case] from_state: ProductState,
            #[case] to_state: ProductState,
        ) {
            let mut item = Item {
                product_id: Default::default(),
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_product_id: Default::default(),
                shop_name: "Boop".into(),
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
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };

            let actual = item.change_state(to_state);

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
        fn should_return_state_change_when_state_changed_for_change_state(
            #[case] from_state: ProductState,
            #[case] to_state: ProductState,
        ) {
            let mut item = Item {
                product_id: Default::default(),
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_product_id: Default::default(),
                shop_name: "Boop".into(),
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
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };

            let actual = item.change_state(to_state).unwrap();
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
        fn should_change_state_when_state_changed_for_change_state(
            #[case] from_state: ProductState,
            #[case] to_state: ProductState,
        ) {
            let mut item = Item {
                product_id: Default::default(),
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_product_id: Default::default(),
                shop_name: "Boop".into(),
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
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };

            let _ = item.change_state(to_state).unwrap();
            assert_eq!(to_state, item.state);
        }
    }

    mod price {
        use crate::core::item::Item;
        use crate::core::product_event::ItemEventPayload;
        use common::currency::domain::Currency;
        use common::product_state::domain::ProductState;
        use common::language::domain::Language;
        use common::localized::Localized;
        use common::price::domain::{FxRate, MonetaryAmount, Price};
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
        fn should_return_none_when_price_and_currency_did_not_change_for_new_price(
            #[case] currency: Currency,
            #[case] monetary_amount: MonetaryAmount,
        ) {
            let price = Price {
                monetary_amount,
                currency,
            };
            let mut item = Item {
                product_id: Default::default(),
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_product_id: Default::default(),
                shop_name: "Boop".into(),
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
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };

            let actual = item.new_price(Some(price), &IdentityFxRate);

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
        fn should_discover_price_when_price_changed_from_none_for_new_price(
            #[case] to_price: Price,
        ) {
            let mut item = Item {
                product_id: Default::default(),
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_product_id: Default::default(),
                shop_name: "Boop".into(),
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
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };
            let actual = item.new_price(Some(to_price), &IdentityFxRate).unwrap();

            match actual.payload {
                ItemEventPayload::PriceDiscovered(payload) => {
                    assert_eq!(to_price, payload.native_price);
                    assert!(
                        payload
                            .other_price
                            .iter()
                            .all(|(_, amount)| &to_price.monetary_amount == amount)
                    );
                    assert_eq!(item.native_price, Some(to_price));
                    assert!(
                        item.other_price
                            .iter()
                            .all(|(_, amount)| &to_price.monetary_amount == amount)
                    );
                }
                _ => panic!("Expected ItemEventPayload::PriceDiscovered"),
            }
        }

        #[rstest::rstest]
        #[case::eur_non_zero(Price::new(420u64.into(), Currency::Eur))]
        #[case::gbp_non_zero(Price::new(430u64.into(), Currency::Gbp))]
        #[case::usd_non_zero(Price::new(440u64.into(), Currency::Usd))]
        #[case::aud_non_zero(Price::new(450u64.into(), Currency::Aud))]
        #[case::cad_non_zero(Price::new(460u64.into(), Currency::Cad))]
        #[case::nzd_non_zero(Price::new(477u64.into(), Currency::Nzd))]
        fn should_find_dropped_price_when_price_dropped_for_new_price(#[case] to_price: Price) {
            let mut item = Item {
                product_id: Default::default(),
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_product_id: Default::default(),
                shop_name: "Boop".into(),
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
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };
            let actual = item.new_price(Some(to_price), &IdentityFxRate).unwrap();

            match actual.payload {
                ItemEventPayload::PriceDropped(payload) => {
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
                    assert_eq!(item.native_price, Some(to_price));
                    assert!(
                        item.other_price
                            .iter()
                            .all(|(_, amount)| &to_price.monetary_amount == amount)
                    );
                }
                _ => panic!("Expected ItemEventPayload::PriceDropped"),
            }
        }

        #[rstest::rstest]
        #[case::eur_non_zero(Price::new(420u64.into(), Currency::Eur))]
        #[case::gbp_non_zero(Price::new(430u64.into(), Currency::Gbp))]
        #[case::usd_non_zero(Price::new(440u64.into(), Currency::Usd))]
        #[case::aud_non_zero(Price::new(450u64.into(), Currency::Aud))]
        #[case::cad_non_zero(Price::new(460u64.into(), Currency::Cad))]
        #[case::nzd_non_zero(Price::new(477u64.into(), Currency::Nzd))]
        fn should_find_increased_price_when_price_increased_for_new_price(#[case] to_price: Price) {
            let mut item = Item {
                product_id: Default::default(),
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_product_id: Default::default(),
                shop_name: "Boop".into(),
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
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };
            let actual = item.new_price(Some(to_price), &IdentityFxRate).unwrap();

            match actual.payload {
                ItemEventPayload::PriceIncreased(payload) => {
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
                    assert_eq!(item.native_price, Some(to_price));
                }
                _ => panic!("Expected ItemEventPayload::PriceIncreased"),
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
        fn should_remove_price_when_price_changed_from_some_to_none_for_new_price(
            #[case] price: Price,
        ) {
            let mut item = Item {
                product_id: Default::default(),
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_product_id: Default::default(),
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
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };
            let actual = item.new_price(None, &IdentityFxRate).unwrap();

            match actual.payload {
                ItemEventPayload::PriceRemoved(payload) => {
                    assert!(item.native_price.is_none());
                    assert!(item.other_price.is_empty());
                    assert_eq!(price, payload.old_native_price);
                }
                _ => panic!("Expected ItemEventPayload::PriceRemoved"),
            }
        }
    }
}
