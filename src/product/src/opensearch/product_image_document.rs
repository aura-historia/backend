use crate::{
    core::product_image::ProductImage, dynamodb::product_image_record::ProductImageRecord,
    opensearch::prohibited_content_document::ProhibitedContentDocument,
};
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Clone, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProductImageDocument {
    pub url: Url,
    pub prohibited_content: ProhibitedContentDocument,
}

impl From<ProductImage> for ProductImageDocument {
    fn from(value: ProductImage) -> Self {
        ProductImageDocument {
            url: value.url,
            prohibited_content: value.prohibited_content.into(),
        }
    }
}

impl From<ProductImageDocument> for ProductImage {
    fn from(value: ProductImageDocument) -> Self {
        ProductImage {
            url: value.url,
            prohibited_content: value.prohibited_content.into(),
        }
    }
}

impl From<ProductImageRecord> for ProductImageDocument {
    fn from(value: ProductImageRecord) -> Self {
        ProductImageDocument {
            url: value.url,
            prohibited_content: value.prohibited_content.into(),
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use crate::opensearch::product_image_document::ProductImageDocument;
    use common::fake::url::ImageUrl;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for ProductImageDocument {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            ProductImageDocument {
                url: config.fake_with_rng::<ImageUrl, R>(rng).into(),
                prohibited_content: config.fake_with_rng(rng),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::opensearch::product_image_document::ProductImageDocument;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_product_image_document() {
            let _ = Faker.fake::<ProductImageDocument>();
        }
    }
}
