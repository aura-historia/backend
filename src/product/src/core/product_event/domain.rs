use crate::core::description::Description;
use crate::core::product_image::ProductImage;
use crate::core::title::Title;
use common::currency::domain::Currency;
use common::has_key::HasKey;
use common::language::domain::Language;
use common::localized::Localized;
use common::price::domain::{MonetaryAmount, Price};
use common::product_id::ProductKey;
use common::product_state::domain::ProductState;
use common::shop_id::ShopId;
use common::shop_name::ShopName;
use common::shops_product_id::ShopsProductId;
use common::slug_id::SlugId;
use shop::core::shop_type::ShopType;
use std::collections::HashMap;
use time::OffsetDateTime;
use url::Url;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum ProductDomainEventPayload {
    Created(ProductCreatedDomainEventPayload),
    StateListed(ProductStateChangeDomainEventPayload),
    StateAvailable(ProductStateChangeDomainEventPayload),
    StateReserved(ProductStateChangeDomainEventPayload),
    StateSold(ProductStateChangeDomainEventPayload),
    StateRemoved(ProductStateChangeDomainEventPayload),
    StateUnknown(ProductStateChangeDomainEventPayload),
    PriceDiscovered(ProductPriceDiscoveryDomainEventPayload),
    PriceDropped(ProductPriceChangeDomainEventPayload),
    PriceIncreased(ProductPriceChangeDomainEventPayload),
    PriceRemoved(ProductPriceRemovedDomainEventPayload),
}

impl ProductDomainEventPayload {
    pub fn is_price_event(&self) -> bool {
        match self {
            ProductDomainEventPayload::Created(_) => false,
            ProductDomainEventPayload::StateListed(_) => false,
            ProductDomainEventPayload::StateAvailable(_) => false,
            ProductDomainEventPayload::StateReserved(_) => false,
            ProductDomainEventPayload::StateSold(_) => false,
            ProductDomainEventPayload::StateRemoved(_) => false,
            ProductDomainEventPayload::StateUnknown(_) => false,
            ProductDomainEventPayload::PriceDiscovered(_) => true,
            ProductDomainEventPayload::PriceDropped(_) => true,
            ProductDomainEventPayload::PriceIncreased(_) => true,
            ProductDomainEventPayload::PriceRemoved(_) => true,
        }
    }

    pub fn is_state_event(&self) -> bool {
        match self {
            ProductDomainEventPayload::Created(_) => false,
            ProductDomainEventPayload::StateListed(_) => true,
            ProductDomainEventPayload::StateAvailable(_) => true,
            ProductDomainEventPayload::StateReserved(_) => true,
            ProductDomainEventPayload::StateSold(_) => true,
            ProductDomainEventPayload::StateRemoved(_) => true,
            ProductDomainEventPayload::StateUnknown(_) => true,
            ProductDomainEventPayload::PriceDiscovered(_) => false,
            ProductDomainEventPayload::PriceDropped(_) => false,
            ProductDomainEventPayload::PriceIncreased(_) => false,
            ProductDomainEventPayload::PriceRemoved(_) => false,
        }
    }

    pub fn as_created(&self) -> Option<&ProductCreatedDomainEventPayload> {
        match self {
            ProductDomainEventPayload::Created(payload) => Some(payload),
            _ => None,
        }
    }

    pub fn as_state_changed(&self) -> Option<&ProductStateChangeDomainEventPayload> {
        match self {
            ProductDomainEventPayload::StateListed(payload) => Some(payload),
            ProductDomainEventPayload::StateAvailable(payload) => Some(payload),
            ProductDomainEventPayload::StateReserved(payload) => Some(payload),
            ProductDomainEventPayload::StateSold(payload) => Some(payload),
            ProductDomainEventPayload::StateRemoved(payload) => Some(payload),
            ProductDomainEventPayload::StateUnknown(payload) => Some(payload),
            _ => None,
        }
    }

    pub fn as_new_state(&self) -> Option<ProductState> {
        match self {
            ProductDomainEventPayload::StateListed(_) => Some(ProductState::Listed),
            ProductDomainEventPayload::StateAvailable(_) => Some(ProductState::Available),
            ProductDomainEventPayload::StateReserved(_) => Some(ProductState::Reserved),
            ProductDomainEventPayload::StateSold(_) => Some(ProductState::Sold),
            ProductDomainEventPayload::StateRemoved(_) => Some(ProductState::Removed),
            ProductDomainEventPayload::StateUnknown(_) => Some(ProductState::Unknown),
            _ => None,
        }
    }

    pub fn as_price_discovered(&self) -> Option<&ProductPriceDiscoveryDomainEventPayload> {
        match self {
            ProductDomainEventPayload::PriceDiscovered(payload) => Some(payload),
            _ => None,
        }
    }

    pub fn as_price_changed(&self) -> Option<&ProductPriceChangeDomainEventPayload> {
        match self {
            ProductDomainEventPayload::PriceDropped(payload) => Some(payload),
            ProductDomainEventPayload::PriceIncreased(payload) => Some(payload),
            _ => None,
        }
    }

    pub fn as_price_removed(&self) -> Option<&ProductPriceRemovedDomainEventPayload> {
        match self {
            ProductDomainEventPayload::PriceRemoved(payload) => Some(payload),
            _ => None,
        }
    }
}

impl HasKey for ProductDomainEventPayload {
    type Key = ProductKey;

    fn key(&self) -> ProductKey {
        ProductKey::new(*self.shop_id(), self.shops_product_id().clone())
    }
}

pub trait ProductCommonEventPayload {
    fn shop_id(&self) -> &ShopId;
    fn shops_product_id(&self) -> &ShopsProductId;
}

impl ProductCommonEventPayload for ProductDomainEventPayload {
    fn shop_id(&self) -> &ShopId {
        match self {
            ProductDomainEventPayload::Created(payload) => payload.shop_id(),
            ProductDomainEventPayload::StateListed(payload) => payload.shop_id(),
            ProductDomainEventPayload::StateAvailable(payload) => payload.shop_id(),
            ProductDomainEventPayload::StateReserved(payload) => payload.shop_id(),
            ProductDomainEventPayload::StateSold(payload) => payload.shop_id(),
            ProductDomainEventPayload::StateRemoved(payload) => payload.shop_id(),
            ProductDomainEventPayload::StateUnknown(payload) => payload.shop_id(),
            ProductDomainEventPayload::PriceDiscovered(payload) => payload.shop_id(),
            ProductDomainEventPayload::PriceDropped(payload) => payload.shop_id(),
            ProductDomainEventPayload::PriceIncreased(payload) => payload.shop_id(),
            ProductDomainEventPayload::PriceRemoved(payload) => payload.shop_id(),
        }
    }

    fn shops_product_id(&self) -> &ShopsProductId {
        match self {
            ProductDomainEventPayload::Created(payload) => payload.shops_product_id(),
            ProductDomainEventPayload::StateListed(payload) => payload.shops_product_id(),
            ProductDomainEventPayload::StateAvailable(payload) => payload.shops_product_id(),
            ProductDomainEventPayload::StateReserved(payload) => payload.shops_product_id(),
            ProductDomainEventPayload::StateSold(payload) => payload.shops_product_id(),
            ProductDomainEventPayload::StateRemoved(payload) => payload.shops_product_id(),
            ProductDomainEventPayload::StateUnknown(payload) => payload.shops_product_id(),
            ProductDomainEventPayload::PriceDiscovered(payload) => payload.shops_product_id(),
            ProductDomainEventPayload::PriceDropped(payload) => payload.shops_product_id(),
            ProductDomainEventPayload::PriceIncreased(payload) => payload.shops_product_id(),
            ProductDomainEventPayload::PriceRemoved(payload) => payload.shops_product_id(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductCreatedDomainEventPayload {
    pub product_slug_id: SlugId<6>,
    pub shop_slug_id: SlugId<0>,
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub shop_name: ShopName,
    pub shop_type: ShopType,
    pub native_title: Localized<Language, Title>,
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
    pub auction_start: Option<OffsetDateTime>,
    pub auction_end: Option<OffsetDateTime>,
}

impl ProductCommonEventPayload for ProductCreatedDomainEventPayload {
    fn shop_id(&self) -> &ShopId {
        &self.shop_id
    }

    fn shops_product_id(&self) -> &ShopsProductId {
        &self.shops_product_id
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductStateChangeDomainEventPayload {
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub old_state: ProductState,
}

impl ProductCommonEventPayload for ProductStateChangeDomainEventPayload {
    fn shop_id(&self) -> &ShopId {
        &self.shop_id
    }

    fn shops_product_id(&self) -> &ShopsProductId {
        &self.shops_product_id
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductPriceDiscoveryDomainEventPayload {
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub native_price: Price,
    pub other_price: HashMap<Currency, MonetaryAmount>,
}

impl ProductCommonEventPayload for ProductPriceDiscoveryDomainEventPayload {
    fn shop_id(&self) -> &ShopId {
        &self.shop_id
    }

    fn shops_product_id(&self) -> &ShopsProductId {
        &self.shops_product_id
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductPriceChangeDomainEventPayload {
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub new_native_price: Price,
    pub new_other_price: HashMap<Currency, MonetaryAmount>,
    pub old_native_price: Price,
    pub old_other_price: HashMap<Currency, MonetaryAmount>,
}

impl ProductCommonEventPayload for ProductPriceChangeDomainEventPayload {
    fn shop_id(&self) -> &ShopId {
        &self.shop_id
    }

    fn shops_product_id(&self) -> &ShopsProductId {
        &self.shops_product_id
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductPriceRemovedDomainEventPayload {
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub old_native_price: Price,
    pub old_other_price: HashMap<Currency, MonetaryAmount>,
}

impl ProductCommonEventPayload for ProductPriceRemovedDomainEventPayload {
    fn shop_id(&self) -> &ShopId {
        &self.shop_id
    }

    fn shops_product_id(&self) -> &ShopsProductId {
        &self.shops_product_id
    }
}

#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum LocalizedProductDomainEventPayloadView {
    Created(LocalizedProductCreatedDomainEventPayloadView),
    StateListed(LocalizedProductStateChangeDomainEventPayloadView),
    StateAvailable(LocalizedProductStateChangeDomainEventPayloadView),
    StateReserved(LocalizedProductStateChangeDomainEventPayloadView),
    StateSold(LocalizedProductStateChangeDomainEventPayloadView),
    StateRemoved(LocalizedProductStateChangeDomainEventPayloadView),
    StateUnknown(LocalizedProductStateChangeDomainEventPayloadView),
    PriceDiscovered(LocalizedProductPriceDiscoveryDomainEventPayloadView),
    PriceDropped(LocalizedProductPriceChangeDomainEventPayloadView),
    PriceIncreased(LocalizedProductPriceChangeDomainEventPayloadView),
    PriceRemoved(LocalizedProductPriceRemovedDomainEventPayloadView),
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalizedProductCreatedDomainEventPayloadView {
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
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalizedProductStateChangeDomainEventPayloadView {
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub old_state: ProductState,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalizedProductPriceChangeDomainEventPayloadView {
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub new_price: Price,
    pub old_price: Price,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalizedProductPriceDiscoveryDomainEventPayloadView {
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub price: Price,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalizedProductPriceRemovedDomainEventPayloadView {
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub old_price: Price,
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use common::price::domain::{FixedFxRate, FxRate};
    use fake::{Dummy, Fake, Faker, Rng};

    impl Dummy<Faker> for ProductCreatedDomainEventPayload {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
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
            ProductCreatedDomainEventPayload {
                product_slug_id: SlugId::from(native_title.payload.as_ref()),
                shop_slug_id: SlugId::from(shop_name.as_ref()),
                shop_id: config.fake_with_rng(rng),
                shops_product_id: config.fake_with_rng(rng),
                shop_name,
                shop_type: config.fake_with_rng(rng),
                native_title,
                native_description: config.fake_with_rng(rng),
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
            }
        }
    }

    impl Dummy<Faker> for ProductStateChangeDomainEventPayload {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            ProductStateChangeDomainEventPayload {
                shop_id: config.fake_with_rng(rng),
                shops_product_id: config.fake_with_rng(rng),
                old_state: config.fake_with_rng(rng),
            }
        }
    }

    impl Dummy<Faker> for ProductPriceDiscoveryDomainEventPayload {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let native_price: Price = config.fake_with_rng(rng);
            let other_price = FixedFxRate()
                .exchange_all(native_price.currency, native_price.monetary_amount)
                .unwrap();
            ProductPriceDiscoveryDomainEventPayload {
                shop_id: config.fake_with_rng(rng),
                shops_product_id: config.fake_with_rng(rng),
                native_price,
                other_price,
            }
        }
    }

    impl Dummy<Faker> for ProductPriceChangeDomainEventPayload {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let new_native_price: Price = config.fake_with_rng(rng);
            let new_other_price = FixedFxRate()
                .exchange_all(new_native_price.currency, new_native_price.monetary_amount)
                .unwrap();
            let old_native_price: Price = config.fake_with_rng(rng);
            let old_other_price = FixedFxRate()
                .exchange_all(old_native_price.currency, old_native_price.monetary_amount)
                .unwrap();
            ProductPriceChangeDomainEventPayload {
                shop_id: config.fake_with_rng(rng),
                shops_product_id: config.fake_with_rng(rng),
                new_native_price,
                new_other_price,
                old_native_price,
                old_other_price,
            }
        }
    }

    impl Dummy<Faker> for ProductPriceRemovedDomainEventPayload {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let old_native_price: Price = config.fake_with_rng(rng);
            let old_other_price = FixedFxRate()
                .exchange_all(old_native_price.currency, old_native_price.monetary_amount)
                .unwrap();
            ProductPriceRemovedDomainEventPayload {
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
            ProductDomainEvent,
            domain::{
                ProductCreatedDomainEventPayload, ProductDomainEventPayload,
                ProductPriceRemovedDomainEventPayload, ProductStateChangeDomainEventPayload,
            },
        };
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_product_created_event_payload() {
            let _ = Faker.fake::<ProductCreatedDomainEventPayload>();
        }

        #[test]
        fn should_fake_product_state_change_event_payload() {
            let _ = Faker.fake::<ProductStateChangeDomainEventPayload>();
        }

        #[test]
        fn should_fake_product_price_change_event_payload() {
            let _ = Faker.fake::<ProductStateChangeDomainEventPayload>();
        }

        #[test]
        fn should_fake_product_price_removed_event_payload() {
            let _ = Faker.fake::<ProductPriceRemovedDomainEventPayload>();
        }

        #[test]
        fn should_fake_product_event_payload() {
            let _ = Faker.fake::<ProductDomainEventPayload>();
        }

        #[test]
        fn should_fake_product_event() {
            let _ = Faker.fake::<ProductDomainEvent>();
        }
    }
}
