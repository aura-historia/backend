use crate::core::{address::StructuredAddress, shop_type::ShopType};
use common::{domain::Domain, shop_name::ShopName};
use serde_email::Email;
use std::collections::HashSet;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct CreateShopCommand {
    pub name: ShopName,
    pub shop_type: ShopType,
    pub domains: HashSet<Domain>,
    pub url: Option<Url>,
    pub image: Option<Url>,
    pub structured_address: Option<StructuredAddress>,
    pub phone: Option<String>,
    pub email: Option<Email>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct UpdateShopCommand {
    pub shop_type: Option<ShopType>,
    pub domains: Option<HashSet<Domain>>,
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
                url: config.fake_with_rng(rng),
                image: config.fake_with_rng(rng),
                structured_address: None,
                phone: None,
                email: None,
            }
        }
    }

    impl Dummy<Faker> for UpdateShopCommand {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            UpdateShopCommand {
                shop_type: config.fake_with_rng(rng),
                domains: config.fake_with_rng(rng),
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
