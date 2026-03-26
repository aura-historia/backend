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
use tracing::error;
use url::Url;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum ProductDomainEventPayload {
    Created(ProductCreatedDomainEventPayload),
    StateChanged(ProductStateChangeDomainEventPayload),
    PriceChanged(ProductPriceChangeDomainEventPayload),
}

impl ProductDomainEventPayload {
    pub fn is_price_event(&self) -> bool {
        matches!(self, ProductDomainEventPayload::PriceChanged(_))
    }

    pub fn is_state_event(&self) -> bool {
        matches!(self, ProductDomainEventPayload::StateChanged(_))
    }

    pub fn as_created(&self) -> Option<&ProductCreatedDomainEventPayload> {
        match self {
            ProductDomainEventPayload::Created(payload) => Some(payload),
            _ => None,
        }
    }

    pub fn as_state_changed(&self) -> Option<&ProductStateChangeDomainEventPayload> {
        match self {
            ProductDomainEventPayload::StateChanged(payload) => Some(payload),
            _ => None,
        }
    }

    pub fn as_new_state(&self) -> Option<ProductState> {
        match self {
            ProductDomainEventPayload::StateChanged(payload) => Some(payload.new_state),
            _ => None,
        }
    }

    pub fn as_price_changed(&self) -> Option<&ProductPriceChangeDomainEventPayload> {
        match self {
            ProductDomainEventPayload::PriceChanged(payload) => Some(payload),
            _ => None,
        }
    }

    pub fn localized(self, currency: &Currency) -> LocalizedProductDomainEventPayloadView {
        match self {
            ProductDomainEventPayload::Created(payload) => {
                let mut prices = payload.other_price;
                if let Some(native_price) = payload.native_price {
                    prices.insert(native_price.currency, native_price.monetary_amount);
                }
                LocalizedProductDomainEventPayloadView::Created(
                    LocalizedProductCreatedDomainEventPayloadView {
                        shop_id: payload.shop_id,
                        shops_product_id: payload.shops_product_id,
                        shop_name: payload.shop_name,
                        shop_type: payload.shop_type,
                        title: payload.native_title,
                        description: payload.native_description,
                        price: prices
                            .remove(currency)
                            .map(|amount| Price::new(amount, *currency)),
                        state: payload.state,
                        url: payload.url,
                        images: payload.images,
                    },
                )
            }
            ProductDomainEventPayload::StateChanged(payload) => {
                LocalizedProductDomainEventPayloadView::StateChanged(
                    LocalizedProductStateChangeDomainEventPayloadView {
                        shop_id: payload.shop_id,
                        shops_product_id: payload.shops_product_id,
                        old_state: payload.old_state,
                        new_state: payload.new_state,
                    },
                )
            }
            ProductDomainEventPayload::PriceChanged(payload) => {
                let old_price = match payload.old_native_price {
                    Some(old_native_price) => {
                        let mut old_prices = payload.old_other_price;
                        old_prices.insert(old_native_price.currency, old_native_price.monetary_amount);
                        Currency::resolve(&[*currency], old_prices)
                    }
                    None => None,
                };
                let new_price = match payload.new_native_price {
                    Some(new_native_price) => {
                        let mut new_prices = payload.new_other_price;
                        new_prices.insert(new_native_price.currency, new_native_price.monetary_amount);
                        Currency::resolve(&[*currency], new_prices)
                    }
                    None => None,
                };
                LocalizedProductDomainEventPayloadView::PriceChanged(
                    LocalizedProductPriceChangeDomainEventPayloadView {
                        shop_id: payload.shop_id,
                        shops_product_id: payload.shops_product_id,
                        old_price,
                        new_price,
                    },
                )
            }
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
            ProductDomainEventPayload::StateChanged(payload) => payload.shop_id(),
            ProductDomainEventPayload::PriceChanged(payload) => payload.shop_id(),
        }
    }

    fn shops_product_id(&self) -> &ShopsProductId {
        match self {
            ProductDomainEventPayload::Created(payload) => payload.shops_product_id(),
            ProductDomainEventPayload::StateChanged(payload) => payload.shops_product_id(),
            ProductDomainEventPayload::PriceChanged(payload) => payload.shops_product_id(),
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
    pub new_state: ProductState,
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
pub struct ProductPriceChangeDomainEventPayload {
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub old_native_price: Option<Price>,
    pub old_other_price: HashMap<Currency, MonetaryAmount>,
    pub new_native_price: Option<Price>,
    pub new_other_price: HashMap<Currency, MonetaryAmount>,
}

impl ProductPriceChangeDomainEventPayload {
    pub fn new_prices(&self) -> HashMap<Currency, MonetaryAmount> {
        let mut prices = self.new_other_price.clone();
        if let Some(new_native_price) = self.new_native_price {
            prices.insert(new_native_price.currency, new_native_price.monetary_amount);
        }
        prices
    }

    pub fn old_prices(&self) -> HashMap<Currency, MonetaryAmount> {
        let mut prices = self.old_other_price.clone();
        if let Some(old_native_price) = self.old_native_price {
            prices.insert(old_native_price.currency, old_native_price.monetary_amount);
        }
        prices
    }
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
#[allow(clippy::large_enum_variant)]
pub enum LocalizedProductDomainEventPayloadView {
    Created(LocalizedProductCreatedDomainEventPayloadView),
    StateChanged(LocalizedProductStateChangeDomainEventPayloadView),
    PriceChanged(LocalizedProductPriceChangeDomainEventPayloadView),
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
    pub new_state: ProductState,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalizedProductPriceChangeDomainEventPayloadView {
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub old_price: Option<Price>,
    pub new_price: Option<Price>,
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use common::price::domain::{FixedFxRate, FxRate};
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for ProductCreatedDomainEventPayload {
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
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            ProductStateChangeDomainEventPayload {
                shop_id: config.fake_with_rng(rng),
                shops_product_id: config.fake_with_rng(rng),
                old_state: config.fake_with_rng(rng),
                new_state: config.fake_with_rng(rng),
            }
        }
    }

    impl Dummy<Faker> for ProductPriceChangeDomainEventPayload {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let new_native_price: Option<Price> = config.fake_with_rng(rng);
            let new_other_price = match new_native_price {
                None => HashMap::new(),
                Some(price) => FixedFxRate()
                    .exchange_all(price.currency, price.monetary_amount)
                    .unwrap(),
            };
            let old_native_price: Option<Price> = config.fake_with_rng(rng);
            let old_other_price = match old_native_price {
                None => HashMap::new(),
                Some(price) => FixedFxRate()
                    .exchange_all(price.currency, price.monetary_amount)
                    .unwrap(),
            };
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

    #[cfg(test)]
    mod tests {
        use crate::core::product_event::{
            ProductDomainEvent,
            domain::{
                ProductCreatedDomainEventPayload, ProductDomainEventPayload,
                ProductPriceChangeDomainEventPayload, ProductStateChangeDomainEventPayload,
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
            let _ = Faker.fake::<ProductPriceChangeDomainEventPayload>();
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
