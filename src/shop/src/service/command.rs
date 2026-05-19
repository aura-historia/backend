use crate::core::{
    address::StructuredAddress, affiliate_configuration::AffiliateConfiguration,
    shop_type::ShopType, woocommerce_webhook_secret::WoocommerceWebhookSecret,
};
use common::currency::domain::Currency;
use common::language::domain::Language;
use common::{domain::Domain, shop_name::ShopName};
use serde_email::Email;
use std::collections::HashSet;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct CreateShopCommand {
    pub name: ShopName,
    pub shop_type: ShopType,
    pub domains: HashSet<Domain>,
    pub shopify_domain: Option<Domain>,
    pub shopify_currency: Option<Currency>,
    pub shopify_language: Option<Language>,
    pub woocommerce_webhook_secret: Option<WoocommerceWebhookSecret>,
    pub woocommerce_currency: Option<Currency>,
    pub woocommerce_language: Option<Language>,
    pub url: Option<Url>,
    pub image: Option<Url>,
    pub structured_address: Option<StructuredAddress>,
    pub phone: Option<String>,
    pub email: Option<Email>,
    pub affiliate_configuration: Option<AffiliateConfiguration>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct UpdateShopCommand {
    pub shop_type: Option<ShopType>,
    pub domains: Option<HashSet<Domain>>,
    pub shopify_domain: Option<Domain>,
    pub shopify_currency: Option<Currency>,
    pub shopify_language: Option<Language>,
    pub woocommerce_webhook_secret: Option<WoocommerceWebhookSecret>,
    pub woocommerce_currency: Option<Currency>,
    pub woocommerce_language: Option<Language>,
    pub url: Option<Url>,
    pub image: Option<Url>,
    pub structured_address: Option<StructuredAddress>,
    pub phone: Option<String>,
    pub email: Option<Email>,
}

impl UpdateShopCommand {
    pub fn is_empty(&self) -> bool {
        self.shop_type.is_none()
            && self.domains.is_none()
            && self.shopify_domain.is_none()
            && self.shopify_currency.is_none()
            && self.shopify_language.is_none()
            && self.woocommerce_webhook_secret.is_none()
            && self.woocommerce_currency.is_none()
            && self.woocommerce_language.is_none()
            && self.url.is_none()
            && self.image.is_none()
            && self.structured_address.is_none()
            && self.phone.is_none()
            && self.email.is_none()
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for CreateShopCommand {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            CreateShopCommand {
                name: config.fake_with_rng(rng),
                shop_type: config.fake_with_rng(rng),
                domains: [Domain::try_from(format!(
                    "https://www.{}.com/",
                    config.fake_with_rng::<String, R>(rng)
                ))
                .unwrap()]
                .into(),
                shopify_domain: config.fake_with_rng(rng),
                shopify_currency: config.fake_with_rng(rng),
                shopify_language: config.fake_with_rng(rng),
                woocommerce_webhook_secret: config.fake_with_rng(rng),
                woocommerce_currency: config.fake_with_rng(rng),
                woocommerce_language: config.fake_with_rng(rng),
                url: config.fake_with_rng(rng),
                image: config.fake_with_rng(rng),
                structured_address: None,
                phone: None,
                email: None,
                affiliate_configuration: None,
            }
        }
    }

    impl Dummy<Faker> for UpdateShopCommand {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            UpdateShopCommand {
                shop_type: config.fake_with_rng(rng),
                domains: config.fake_with_rng(rng),
                shopify_domain: config.fake_with_rng(rng),
                shopify_currency: config.fake_with_rng(rng),
                shopify_language: config.fake_with_rng(rng),
                woocommerce_webhook_secret: config.fake_with_rng(rng),
                woocommerce_currency: config.fake_with_rng(rng),
                woocommerce_language: config.fake_with_rng(rng),
                url: config.fake_with_rng(rng),
                image: config.fake_with_rng(rng),
                structured_address: None,
                phone: None,
                email: None,
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::service::command::{CreateShopCommand, UpdateShopCommand};
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_create_shop_command() {
            let _ = Faker.fake::<CreateShopCommand>();
        }

        #[test]
        fn should_fake_update_shop_command() {
            let _ = Faker.fake::<UpdateShopCommand>();
        }
    }
}
