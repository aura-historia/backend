use crate::{
    core::product_image::ProductImage, data::prohibited_content_data::ProhibitedContentData,
};
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Clone, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProductImageData {
    pub url: Url,
    pub prohibited_content: ProhibitedContentData,
}

impl From<ProductImage> for ProductImageData {
    fn from(value: ProductImage) -> Self {
        ProductImageData {
            url: value.url,
            prohibited_content: value.prohibited_content.into(),
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use crate::data::product_image_data::ProductImageData;
    use common::fake::url::ImageUrl;
    use fake::{Dummy, Fake, Faker, Rng};

    impl Dummy<Faker> for ProductImageData {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            ProductImageData {
                url: config.fake_with_rng::<ImageUrl, R>(rng).into(),
                prohibited_content: config.fake_with_rng(rng),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::data::product_image_data::ProductImageData;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_product_image_data() {
            let _ = Faker.fake::<ProductImageData>();
        }
    }
}
