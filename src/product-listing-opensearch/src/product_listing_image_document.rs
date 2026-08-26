use product_listing_core::product_listing_image::ProductListingImage;
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ProductListingImageDocument {
    pub url: Url,
}

impl From<ProductListingImage> for ProductListingImageDocument {
    fn from(value: ProductListingImage) -> Self {
        Self {
            url: value.url().clone(),
        }
    }
}

impl From<ProductListingImageDocument> for ProductListingImage {
    fn from(value: ProductListingImageDocument) -> Self {
        Self::new(value.url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_roundtrip_url_only_product_image_document() -> Result<(), url::ParseError> {
        let image = ProductListingImage::new(Url::parse("https://example.com/image.jpg")?);
        assert_eq!(
            image,
            ProductListingImage::from(ProductListingImageDocument::from(image.clone()))
        );
        Ok(())
    }
}
