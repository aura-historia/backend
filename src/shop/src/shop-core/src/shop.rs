use common::{shop_id::ShopId, shop_name::ShopName};
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct Shop {
    pub shop_id: ShopId,
    pub name: ShopName,
    pub urls: Vec<Url>,
    pub image: Option<Url>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, Rng};

    impl Dummy<Faker> for Shop {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            Shop {
                shop_id: config.fake_with_rng(rng),
                name: config.fake_with_rng(rng),
                urls: vec![Url::parse(&format!("https://www.{}.com/", config.fake_with_rng::<String, R>(rng))).unwrap()],
                image: Some(Url::parse("https://upload.wikimedia.org/wikipedia/commons/thumb/c/c1/Google_%22G%22_logo.svg/1200px-Google_%22G%22_logo.svg.png").unwrap()),
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::shop::Shop;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_shop() {
            let _ = Faker.fake::<Shop>();
        }
    }
}
