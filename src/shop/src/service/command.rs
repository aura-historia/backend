use common::shop_name::ShopName;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct CreateShopCommand {
    pub name: ShopName,
    pub urls: Vec<Url>,
    pub image: Option<Url>,
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, Rng};

    impl Dummy<Faker> for CreateShopCommand {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            CreateShopCommand {
                name: config.fake_with_rng(rng),
                urls: vec![
                    Url::parse(&format!(
                        "https://www.{}.com/",
                        config.fake_with_rng::<String, R>(rng)
                    ))
                    .unwrap(),
                ],
                image: config.fake_with_rng(rng),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use fake::{Fake, Faker};

        use crate::service::command::CreateShopCommand;

        #[test]
        fn should_fake_create_shop_command() {
            let _ = Faker.fake::<CreateShopCommand>();
        }
    }
}
