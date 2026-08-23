use crate::prohibited_content_document::ProhibitedContentDocument;
use product_core::product_image::ProductImage;
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProductImageDocument {
    pub url: Url,
    pub prohibited_content: ProhibitedContentDocument,
}

impl From<ProductImage> for ProductImageDocument {
    fn from(value: ProductImage) -> Self {
        Self {
            url: value.url,
            prohibited_content: value.prohibited_content.into(),
        }
    }
}

impl From<ProductImageDocument> for ProductImage {
    fn from(value: ProductImageDocument) -> Self {
        Self {
            url: value.url,
            prohibited_content: value.prohibited_content.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use product_core::prohibited_content::ProhibitedContent;

    #[test]
    fn should_roundtrip_product_image_document() -> Result<(), url::ParseError> {
        let image = ProductImage {
            url: Url::parse("https://example.com/image.jpg")?,
            prohibited_content: ProhibitedContent::None,
        };

        let document = ProductImageDocument::from(image.clone());

        assert_eq!(image, ProductImage::from(document));
        Ok(())
    }
}
