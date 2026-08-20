use crate::prohibited_content::ProhibitedContent;
use url::Url;

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct ProductImage {
    pub url: Url,
    pub prohibited_content: ProhibitedContent,
}

#[cfg(feature = "test-data")]
mod faker {
    use crate::product_image::ProductImage;
    use fake::{Dummy, Fake, Faker, RngExt};
    use url::Url;

    impl Dummy<Faker> for ProductImage {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let url = match Url::parse("https://example.com/image.jpg") {
                Ok(url) => url,
                Err(error) => panic!("invalid product image test URL: {error}"),
            };

            ProductImage {
                url,
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
