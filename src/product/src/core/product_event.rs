use crate::core::description::Description;
use crate::core::title::Title;
use common::currency::domain::Currency;
use common::event::Event;
use common::has_key::HasKey;
use common::language::domain::Language;
use common::localized::Localized;
use common::price::domain::{MonetaryAmount, Price};
use common::product_id::{ProductId, ProductKey};
use common::product_state::domain::ProductState;
use common::shop_id::ShopId;
use common::shop_name::ShopName;
use common::shops_product_id::ShopsProductId;
use std::collections::HashMap;
use url::Url;

pub type ProductEvent = Event<ProductId, ProductEventPayload>;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum ProductEventPayload {
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

impl ProductEventPayload {
    pub fn as_created(&self) -> Option<&ItemCreatedEventPayload> {
        match self {
            ProductEventPayload::Created(payload) => Some(payload),
            _ => None,
        }
    }

    pub fn as_state_changed(&self) -> Option<&ItemStateChangeEventPayload> {
        match self {
            ProductEventPayload::StateListed(payload) => Some(payload),
            ProductEventPayload::StateAvailable(payload) => Some(payload),
            ProductEventPayload::StateReserved(payload) => Some(payload),
            ProductEventPayload::StateSold(payload) => Some(payload),
            ProductEventPayload::StateRemoved(payload) => Some(payload),
            ProductEventPayload::StateUnknown(payload) => Some(payload),
            _ => None,
        }
    }

    pub fn as_price_discovered(&self) -> Option<&ItemPriceDiscoveryEventPayload> {
        match self {
            ProductEventPayload::PriceDiscovered(payload) => Some(payload),
            _ => None,
        }
    }

    pub fn as_price_changed(&self) -> Option<&ItemPriceChangeEventPayload> {
        match self {
            ProductEventPayload::PriceDropped(payload) => Some(payload),
            ProductEventPayload::PriceIncreased(payload) => Some(payload),
            _ => None,
        }
    }

    pub fn as_price_removed(&self) -> Option<&ItemPriceRemovedEventPayload> {
        match self {
            ProductEventPayload::PriceRemoved(payload) => Some(payload),
            _ => None,
        }
    }
}

impl HasKey for ProductEventPayload {
    type Key = ProductKey;

    fn key(&self) -> ProductKey {
        ProductKey::new(*self.shop_id(), self.shops_product_id().clone())
    }
}

pub trait ProductCommonEventPayload {
    fn shop_id(&self) -> &ShopId;
    fn shops_product_id(&self) -> &ShopsProductId;
}

impl ProductCommonEventPayload for ProductEventPayload {
    fn shop_id(&self) -> &ShopId {
        match self {
            ProductEventPayload::Created(payload) => payload.shop_id(),
            ProductEventPayload::StateListed(payload) => payload.shop_id(),
            ProductEventPayload::StateAvailable(payload) => payload.shop_id(),
            ProductEventPayload::StateReserved(payload) => payload.shop_id(),
            ProductEventPayload::StateSold(payload) => payload.shop_id(),
            ProductEventPayload::StateRemoved(payload) => payload.shop_id(),
            ProductEventPayload::StateUnknown(payload) => payload.shop_id(),
            ProductEventPayload::PriceDiscovered(payload) => payload.shop_id(),
            ProductEventPayload::PriceDropped(payload) => payload.shop_id(),
            ProductEventPayload::PriceIncreased(payload) => payload.shop_id(),
            ProductEventPayload::PriceRemoved(payload) => payload.shop_id(),
        }
    }

    fn shops_product_id(&self) -> &ShopsProductId {
        match self {
            ProductEventPayload::Created(payload) => payload.shops_product_id(),
            ProductEventPayload::StateListed(payload) => payload.shops_product_id(),
            ProductEventPayload::StateAvailable(payload) => payload.shops_product_id(),
            ProductEventPayload::StateReserved(payload) => payload.shops_product_id(),
            ProductEventPayload::StateSold(payload) => payload.shops_product_id(),
            ProductEventPayload::StateRemoved(payload) => payload.shops_product_id(),
            ProductEventPayload::StateUnknown(payload) => payload.shops_product_id(),
            ProductEventPayload::PriceDiscovered(payload) => payload.shops_product_id(),
            ProductEventPayload::PriceDropped(payload) => payload.shops_product_id(),
            ProductEventPayload::PriceIncreased(payload) => payload.shops_product_id(),
            ProductEventPayload::PriceRemoved(payload) => payload.shops_product_id(),
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

impl ProductCommonEventPayload for ItemCreatedEventPayload {
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

impl ProductCommonEventPayload for ItemStateChangeEventPayload {
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

impl ProductCommonEventPayload for ItemPriceDiscoveryEventPayload {
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

impl ProductCommonEventPayload for ItemPriceChangeEventPayload {
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

impl ProductCommonEventPayload for ItemPriceRemovedEventPayload {
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
            ItemCreatedEventPayload, ItemPriceRemovedEventPayload, ItemStateChangeEventPayload,
            ProductEvent, ProductEventPayload,
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
            let _ = Faker.fake::<ProductEventPayload>();
        }

        #[test]
        fn should_fake_item_event() {
            let _ = Faker.fake::<ProductEvent>();
        }
    }
}
