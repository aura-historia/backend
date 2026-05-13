use crate::core::{
    address::{GeoAddress, StructuredAddress},
    partner_shop::PartnerShop,
    partner_status::ShopPartnerStatus,
    shop_type::ShopType,
    woocommerce_webhook_secret::WoocommerceWebhookSecret,
};
use common::currency::domain::Currency;
use common::{domain::Domain, shop_id::ShopId, shop_name::ShopName, slug_id::SlugId};
use serde_email::Email;
use std::collections::HashSet;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct Shop {
    pub shop_id: ShopId,
    pub shop_slug_id: SlugId<0>,
    pub name: ShopName,
    pub shop_type: ShopType,
    pub domains: HashSet<Domain>,
    pub shopify_domain: Option<Domain>,
    pub shopify_currency: Option<Currency>,
    pub woocommerce_webhook_secret: Option<WoocommerceWebhookSecret>,
    pub url: Option<Url>,
    pub image: Option<Url>,
    pub structured_address: Option<StructuredAddress>,
    pub geo_address: Option<GeoAddress>,
    pub phone: Option<String>,
    pub email: Option<Email>,
    pub partner_status: ShopPartnerStatus,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

impl From<PartnerShop> for Shop {
    fn from(partner_shop: PartnerShop) -> Self {
        Shop {
            shop_id: partner_shop.shop_id,
            shop_slug_id: partner_shop.shop_slug_id,
            name: partner_shop.name,
            shop_type: partner_shop.shop_type,
            domains: partner_shop.domains,
            shopify_domain: partner_shop.shopify_domain,
            shopify_currency: partner_shop.shopify_currency,
            woocommerce_webhook_secret: partner_shop.woocommerce_webhook_secret,
            url: partner_shop.url,
            image: partner_shop.image,
            structured_address: partner_shop.structured_address,
            geo_address: partner_shop.geo_address,
            phone: partner_shop.phone,
            email: partner_shop.email,
            partner_status: ShopPartnerStatus::Partnered,
            created: partner_shop.created,
            updated: partner_shop.updated,
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for Shop {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let name: ShopName = config.fake_with_rng(rng);
            Shop {
                shop_id: config.fake_with_rng(rng),
                shop_slug_id: SlugId::from(name.as_ref()),
                name,
                shop_type: config.fake_with_rng(rng),
                domains: [Faker.fake()].into(),
                shopify_domain: config.fake_with_rng(rng),
                shopify_currency: config.fake_with_rng(rng),
                woocommerce_webhook_secret: config.fake_with_rng(rng),
                url: config.fake_with_rng(rng),
                image: config.fake_with_rng(rng),
                structured_address: None,
                geo_address: None,
                phone: None,
                email: None,
                partner_status: config.fake_with_rng(rng),
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::core::shop::Shop;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_shop() {
            let _ = Faker.fake::<Shop>();
        }
    }
}
