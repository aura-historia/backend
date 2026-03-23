use crate::{
    core::product_image::ProductImage, dynamodb::prohibited_content_record::ProhibitedContentRecord,
};
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Clone, Hash, PartialEq, Serialize, Deserialize)]
pub struct ProductImageRecord {
    pub url: Url,
    pub prohibited_content: ProhibitedContentRecord,
}

impl From<ProductImage> for ProductImageRecord {
    fn from(value: ProductImage) -> Self {
        ProductImageRecord {
            url: value.url,
            prohibited_content: value.prohibited_content.into(),
        }
    }
}

impl From<ProductImageRecord> for ProductImage {
    fn from(value: ProductImageRecord) -> Self {
        ProductImage {
            url: value.url,
            prohibited_content: value.prohibited_content.into(),
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use crate::dynamodb::product_image_record::ProductImageRecord;
    use common::fake::url::ImageUrl;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for ProductImageRecord {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            ProductImageRecord {
                url: config.fake_with_rng::<ImageUrl, R>(rng).into(),
                prohibited_content: config.fake_with_rng(rng),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::dynamodb::product_image_record::ProductImageRecord;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_product_image_record() {
            let _ = Faker.fake::<ProductImageRecord>();
        }
    }
}
