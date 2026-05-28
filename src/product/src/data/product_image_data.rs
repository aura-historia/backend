use crate::{
    core::product_image::ProductImage, data::prohibited_content_data::ProhibitedContentData,
};
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProductImageData {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub url: Option<Url>,
    pub prohibited_content: ProhibitedContentData,
}

impl ProductImageData {
    pub fn from_with_consent(image: ProductImage, consent: bool) -> ProductImageData {
        ProductImageData {
            url: if image.prohibited_content.is_safe() || consent {
                Some(image.url)
            } else {
                None
            },
            prohibited_content: image.prohibited_content.into(),
        }
    }
}

impl From<ProductImage> for ProductImageData {
    fn from(value: ProductImage) -> Self {
        ProductImageData::from_with_consent(value, false)
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use crate::data::product_image_data::ProductImageData;
    use common::fake::url::ImageUrl;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for ProductImageData {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            ProductImageData {
                url: if config.fake_with_rng(rng) {
                    Some(config.fake_with_rng::<ImageUrl, R>(rng).into())
                } else {
                    None
                },
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

#[cfg(test)]
mod tests {
    use crate::core::product_image::ProductImage;
    use crate::core::prohibited_content::ProhibitedContent;
    use crate::data::product_image_data::ProductImageData;
    use crate::data::prohibited_content_data::ProhibitedContentData;
    use url::Url;

    fn test_url() -> Url {
        Url::parse("https://example.com/image.jpg").unwrap()
    }

    #[test]
    fn from_with_consent_safe_without_consent_returns_some_url() {
        let image = ProductImage {
            url: test_url(),
            prohibited_content: ProhibitedContent::None,
        };
        let result = ProductImageData::from_with_consent(image, false);
        assert_eq!(result.url, Some(test_url()));
        assert_eq!(result.prohibited_content, ProhibitedContentData::None);
    }

    #[test]
    fn from_with_consent_safe_with_consent_returns_some_url() {
        let image = ProductImage {
            url: test_url(),
            prohibited_content: ProhibitedContent::None,
        };
        let result = ProductImageData::from_with_consent(image, true);
        assert_eq!(result.url, Some(test_url()));
        assert_eq!(result.prohibited_content, ProhibitedContentData::None);
    }

    #[test]
    fn from_with_consent_unknown_without_consent_returns_none_url() {
        let image = ProductImage {
            url: test_url(),
            prohibited_content: ProhibitedContent::Unknown,
        };
        let result = ProductImageData::from_with_consent(image, false);
        assert_eq!(result.url, None);
        assert_eq!(result.prohibited_content, ProhibitedContentData::Unknown);
    }

    #[test]
    fn from_with_consent_unknown_with_consent_returns_some_url() {
        let image = ProductImage {
            url: test_url(),
            prohibited_content: ProhibitedContent::Unknown,
        };
        let result = ProductImageData::from_with_consent(image, true);
        assert_eq!(result.url, Some(test_url()));
        assert_eq!(result.prohibited_content, ProhibitedContentData::Unknown);
    }

    #[test]
    fn from_with_consent_nazi_germany_without_consent_returns_none_url() {
        let image = ProductImage {
            url: test_url(),
            prohibited_content: ProhibitedContent::NaziGermany,
        };
        let result = ProductImageData::from_with_consent(image, false);
        assert_eq!(result.url, None);
        assert_eq!(
            result.prohibited_content,
            ProhibitedContentData::NaziGermany
        );
    }

    #[test]
    fn from_with_consent_nazi_germany_with_consent_returns_some_url() {
        let image = ProductImage {
            url: test_url(),
            prohibited_content: ProhibitedContent::NaziGermany,
        };
        let result = ProductImageData::from_with_consent(image, true);
        assert_eq!(result.url, Some(test_url()));
        assert_eq!(
            result.prohibited_content,
            ProhibitedContentData::NaziGermany
        );
    }

    #[test]
    fn from_impl_safe_image_returns_some_url() {
        let image = ProductImage {
            url: test_url(),
            prohibited_content: ProhibitedContent::None,
        };
        let result = ProductImageData::from(image);
        assert_eq!(result.url, Some(test_url()));
        assert_eq!(result.prohibited_content, ProhibitedContentData::None);
    }

    #[test]
    fn from_impl_unknown_image_returns_none_url() {
        let image = ProductImage {
            url: test_url(),
            prohibited_content: ProhibitedContent::Unknown,
        };
        let result = ProductImageData::from(image);
        assert_eq!(result.url, None);
        assert_eq!(result.prohibited_content, ProhibitedContentData::Unknown);
    }

    #[test]
    fn from_impl_nazi_germany_image_returns_none_url() {
        let image = ProductImage {
            url: test_url(),
            prohibited_content: ProhibitedContent::NaziGermany,
        };
        let result = ProductImageData::from(image);
        assert_eq!(result.url, None);
        assert_eq!(
            result.prohibited_content,
            ProhibitedContentData::NaziGermany
        );
    }
}
