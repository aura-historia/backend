use crate::prohibited_content::ProhibitedContent;
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProductImage {
    pub url: Url,
    pub prohibited_content: ProhibitedContent,
}

#[cfg(feature = "test-data")]
mod faker {
    use crate::product_image::ProductImage;
    use common::fake::url::ImageUrl;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for ProductImage {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            ProductImage {
                url: config.fake_with_rng::<ImageUrl, R>(rng).into(),
                prohibited_content: config.fake_with_rng(rng),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::product_image::ProductImage;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_product_image() {
            let _ = Faker.fake::<ProductImage>();
        }
    }
}
