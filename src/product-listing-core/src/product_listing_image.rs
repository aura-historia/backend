use crate::prohibited_content::ProhibitedContent;
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProductListingImage {
    pub url: Url,
    pub prohibited_content: ProhibitedContent,
}

#[cfg(feature = "test-data")]
mod faker {
    use crate::product_listing_image::ProductListingImage;
    use fake::{Dummy, Fake, Faker, RngExt};
    use url::Url;

    impl Dummy<Faker> for ProductListingImage {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let url = match Url::parse("https://example.com/image.jpg") {
                Ok(url) => url,
                Err(error) => panic!("invalid product image test URL: {error}"),
            };

            ProductListingImage {
                url,
                prohibited_content: config.fake_with_rng(rng),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::product_listing_image::ProductListingImage;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_product_listing_image() {
            let _ = Faker.fake::<ProductListingImage>();
        }
    }
}
