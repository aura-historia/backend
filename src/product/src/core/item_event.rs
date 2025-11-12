use crate::core::description::Description;
use crate::core::title::Title;
use common::currency::domain::Currency;
use common::event::Event;
use common::has_key::HasKey;
use common::product_id::{ProductId, ProductKey};
use common::product_state::domain::ProductState;
use common::language::domain::Language;
use common::localized::Localized;
use common::price::domain::{MonetaryAmount, Price};
use common::shop_id::ShopId;
use common::shop_name::ShopName;
use common::shops_product_id::ShopsProductId;
use std::collections::HashMap;
use url::Url;

pub type ProductEvent = Event<ProductId, ItemEventPayload>;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum ItemEventPayload {
    Created(ItemCreatedEventPayload),
    StateListed(ItemStateChangeEventPayload),
    StateAvailable(ItemStateChangeEventPayload),
    StateReserved(ItemStateChangeEventPayload),
    StateSold(ItemStateChangeEventPayload),
    StateRemoved(ItemStateChangeEventPayload),
    StateUnknown(ItemStateChangeEventPayload),
    PriceDiscovered(ItemPriceDiscoveryEventPayload),
    PriceDropped(ItemPriceChangeEventPayload),
    PriceIncreased(ItemPriceChangeEventPayload),
    PriceRemoved(ItemPriceRemovedEventPayload),
}

impl ItemEventPayload {
    pub fn as_created(&self) -> Option<&ItemCreatedEventPayload> {
        match self {
            ItemEventPayload::Created(payload) => Some(payload),
            _ => None,
        }
    }

    pub fn as_state_changed(&self) -> Option<&ItemStateChangeEventPayload> {
        match self {
            ItemEventPayload::StateListed(payload) => Some(payload),
            ItemEventPayload::StateAvailable(payload) => Some(payload),
            ItemEventPayload::StateReserved(payload) => Some(payload),
            ItemEventPayload::StateSold(payload) => Some(payload),
            ItemEventPayload::StateRemoved(payload) => Some(payload),
            ItemEventPayload::StateUnknown(payload) => Some(payload),
            _ => None,
        }
    }

    pub fn as_price_discovered(&self) -> Option<&ItemPriceDiscoveryEventPayload> {
        match self {
            ItemEventPayload::PriceDiscovered(payload) => Some(payload),
            _ => None,
        }
    }

    pub fn as_price_changed(&self) -> Option<&ItemPriceChangeEventPayload> {
        match self {
            ItemEventPayload::PriceDropped(payload) => Some(payload),
            ItemEventPayload::PriceIncreased(payload) => Some(payload),
            _ => None,
        }
    }

    pub fn as_price_removed(&self) -> Option<&ItemPriceRemovedEventPayload> {
        match self {
            ItemEventPayload::PriceRemoved(payload) => Some(payload),
            _ => None,
        }
    }
}

impl HasKey for ItemEventPayload {
    type Key = ProductKey;

    fn key(&self) -> ProductKey {
        ProductKey::new(*self.shop_id(), self.shops_product_id().clone())
    }
}

pub trait ItemCommonEventPayload {
    fn shop_id(&self) -> &ShopId;
    fn shops_product_id(&self) -> &ShopsProductId;
}

impl ItemCommonEventPayload for ItemEventPayload {
    fn shop_id(&self) -> &ShopId {
        match self {
            ItemEventPayload::Created(payload) => payload.shop_id(),
            ItemEventPayload::StateListed(payload) => payload.shop_id(),
            ItemEventPayload::StateAvailable(payload) => payload.shop_id(),
            ItemEventPayload::StateReserved(payload) => payload.shop_id(),
            ItemEventPayload::StateSold(payload) => payload.shop_id(),
            ItemEventPayload::StateRemoved(payload) => payload.shop_id(),
            ItemEventPayload::StateUnknown(payload) => payload.shop_id(),
            ItemEventPayload::PriceDiscovered(payload) => payload.shop_id(),
            ItemEventPayload::PriceDropped(payload) => payload.shop_id(),
            ItemEventPayload::PriceIncreased(payload) => payload.shop_id(),
            ItemEventPayload::PriceRemoved(payload) => payload.shop_id(),
        }
    }

    fn shops_product_id(&self) -> &ShopsProductId {
        match self {
            ItemEventPayload::Created(payload) => payload.shops_product_id(),
            ItemEventPayload::StateListed(payload) => payload.shops_product_id(),
            ItemEventPayload::StateAvailable(payload) => payload.shops_product_id(),
            ItemEventPayload::StateReserved(payload) => payload.shops_product_id(),
            ItemEventPayload::StateSold(payload) => payload.shops_product_id(),
            ItemEventPayload::StateRemoved(payload) => payload.shops_product_id(),
            ItemEventPayload::StateUnknown(payload) => payload.shops_product_id(),
            ItemEventPayload::PriceDiscovered(payload) => payload.shops_product_id(),
            ItemEventPayload::PriceDropped(payload) => payload.shops_product_id(),
            ItemEventPayload::PriceIncreased(payload) => payload.shops_product_id(),
            ItemEventPayload::PriceRemoved(payload) => payload.shops_product_id(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ItemCreatedEventPayload {
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
}

impl ItemCommonEventPayload for ItemCreatedEventPayload {
    fn shop_id(&self) -> &ShopId {
        &self.shop_id
    }

    fn shops_product_id(&self) -> &ShopsProductId {
        &self.shops_product_id
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ItemStateChangeEventPayload {
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub old_state: ProductState,
}

impl ItemCommonEventPayload for ItemStateChangeEventPayload {
    fn shop_id(&self) -> &ShopId {
        &self.shop_id
    }

    fn shops_product_id(&self) -> &ShopsProductId {
        &self.shops_product_id
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ItemPriceDiscoveryEventPayload {
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub native_price: Price,
    pub other_price: HashMap<Currency, MonetaryAmount>,
}

impl ItemCommonEventPayload for ItemPriceDiscoveryEventPayload {
    fn shop_id(&self) -> &ShopId {
        &self.shop_id
    }

    fn shops_product_id(&self) -> &ShopsProductId {
        &self.shops_product_id
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ItemPriceChangeEventPayload {
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub new_native_price: Price,
    pub new_other_price: HashMap<Currency, MonetaryAmount>,
    pub old_native_price: Price,
    pub old_other_price: HashMap<Currency, MonetaryAmount>,
}

impl ItemCommonEventPayload for ItemPriceChangeEventPayload {
    fn shop_id(&self) -> &ShopId {
        &self.shop_id
    }

    fn shops_product_id(&self) -> &ShopsProductId {
        &self.shops_product_id
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ItemPriceRemovedEventPayload {
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub old_native_price: Price,
    pub old_other_price: HashMap<Currency, MonetaryAmount>,
}

impl ItemCommonEventPayload for ItemPriceRemovedEventPayload {
    fn shop_id(&self) -> &ShopId {
        &self.shop_id
    }

    fn shops_product_id(&self) -> &ShopsProductId {
        &self.shops_product_id
    }
}

#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum LocalizedItemEventPayloadView {
    Created(LocalizedItemCreatedEventPayloadView),
    StateListed(LocalizedItemStateChangeEventPayloadView),
    StateAvailable(LocalizedItemStateChangeEventPayloadView),
    StateReserved(LocalizedItemStateChangeEventPayloadView),
    StateSold(LocalizedItemStateChangeEventPayloadView),
    StateRemoved(LocalizedItemStateChangeEventPayloadView),
    StateUnknown(LocalizedItemStateChangeEventPayloadView),
    PriceDiscovered(LocalizedItemPriceDiscoveryEventPayloadView),
    PriceDropped(LocalizedItemPriceChangeEventPayloadView),
    PriceIncreased(LocalizedItemPriceChangeEventPayloadView),
    PriceRemoved(LocalizedItemPriceRemovedEventPayloadView),
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalizedItemCreatedEventPayloadView {
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub shop_name: ShopName,
    pub title: Localized<Language, Title>,
    pub description: Option<Localized<Language, Description>>,
    pub price: Option<Price>,
    pub state: ProductState,
    pub url: Url,
    pub images: Vec<Url>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalizedItemStateChangeEventPayloadView {
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub old_state: ProductState,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalizedItemPriceChangeEventPayloadView {
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub new_price: Price,
    pub old_price: Price,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalizedItemPriceDiscoveryEventPayloadView {
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub price: Price,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalizedItemPriceRemovedEventPayloadView {
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub old_price: Price,
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use common::price::domain::{FixedFxRate, FxRate};
    use fake::{Dummy, Fake, Faker, Rng};

    impl Dummy<Faker> for ItemCreatedEventPayload {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let native_price: Option<Price> = config.fake_with_rng(rng);
            let other_price = match native_price {
                None => HashMap::new(),
                Some(price) => FixedFxRate()
                    .exchange_all(price.currency, price.monetary_amount)
                    .unwrap(),
            };
            let state = config.fake_with_rng(rng);
            ItemCreatedEventPayload {
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
            }
        }
    }

    impl Dummy<Faker> for ItemStateChangeEventPayload {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            ItemStateChangeEventPayload {
                shop_id: config.fake_with_rng(rng),
                shops_product_id: config.fake_with_rng(rng),
                old_state: config.fake_with_rng(rng),
            }
        }
    }

    impl Dummy<Faker> for ItemPriceDiscoveryEventPayload {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let native_price: Price = config.fake_with_rng(rng);
            let other_price = FixedFxRate()
                .exchange_all(native_price.currency, native_price.monetary_amount)
                .unwrap();
            ItemPriceDiscoveryEventPayload {
                shop_id: config.fake_with_rng(rng),
                shops_product_id: config.fake_with_rng(rng),
                native_price,
                other_price,
            }
        }
    }

    impl Dummy<Faker> for ItemPriceChangeEventPayload {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let new_native_price: Price = config.fake_with_rng(rng);
            let new_other_price = FixedFxRate()
                .exchange_all(new_native_price.currency, new_native_price.monetary_amount)
                .unwrap();
            let old_native_price: Price = config.fake_with_rng(rng);
            let old_other_price = FixedFxRate()
                .exchange_all(old_native_price.currency, old_native_price.monetary_amount)
                .unwrap();
            ItemPriceChangeEventPayload {
                shop_id: config.fake_with_rng(rng),
                shops_product_id: config.fake_with_rng(rng),
                new_native_price,
                new_other_price,
                old_native_price,
                old_other_price,
            }
        }
    }

    impl Dummy<Faker> for ItemPriceRemovedEventPayload {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let old_native_price: Price = config.fake_with_rng(rng);
            let old_other_price = FixedFxRate()
                .exchange_all(old_native_price.currency, old_native_price.monetary_amount)
                .unwrap();
            ItemPriceRemovedEventPayload {
                shop_id: config.fake_with_rng(rng),
                shops_product_id: config.fake_with_rng(rng),
                old_native_price,
                old_other_price,
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::core::product_event::{
            ItemCreatedEventPayload, ProductEvent, ItemEventPayload, ItemPriceRemovedEventPayload,
            ItemStateChangeEventPayload,
        };
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_item_created_event_payload() {
            let _ = Faker.fake::<ItemCreatedEventPayload>();
        }

        #[test]
        fn should_fake_item_state_change_event_payload() {
            let _ = Faker.fake::<ItemStateChangeEventPayload>();
        }

        #[test]
        fn should_fake_item_price_change_event_payload() {
            let _ = Faker.fake::<ItemStateChangeEventPayload>();
        }

        #[test]
        fn should_fake_item_price_removed_event_payload() {
            let _ = Faker.fake::<ItemPriceRemovedEventPayload>();
        }

        #[test]
        fn should_fake_item_event_payload() {
            let _ = Faker.fake::<ItemEventPayload>();
        }

        #[test]
        fn should_fake_item_event() {
            let _ = Faker.fake::<ProductEvent>();
        }
    }
}
