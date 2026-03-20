use crate::core::shop_type::ShopType;
use common::{domain::Domain, shop_name::ShopName};
use std::collections::HashSet;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct CreateShopCommand {
    pub name: ShopName,
    pub shop_type: ShopType,
    pub domains: HashSet<Domain>,
    pub image: Option<Url>,
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Default)]
pub struct UpdateShopCommand {
    pub shop_type: Option<ShopType>,
    pub domains: Option<HashSet<Domain>>,
    pub image: Option<Url>,
}

impl UpdateShopCommand {
    pub fn is_empty(&self) -> bool {
        self.shop_type.is_none() && self.domains.is_none() && self.image.is_none()
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
                image: config.fake_with_rng(rng),
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
