use product_core::product_image::ProductImage;
use product_core::prohibited_content::ProhibitedContent;
use serde::{Deserialize, Serialize};

fn serialize_code<T, S>(
    value: &T,
    serializer: S,
    code: fn(T) -> &'static str,
) -> Result<S::Ok, S::Error>
where
    T: Copy,
    S: serde::Serializer,
{
    serializer.serialize_str(code(*value))
}

pub(crate) mod prohibited_content {
    use super::*;

    pub(crate) fn serialize<S>(value: &ProhibitedContent, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serialize_code(value, serializer, ProhibitedContent::as_str)
    }

    pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<ProhibitedContent, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        ProhibitedContent::from_code(&value)
            .ok_or_else(|| serde::de::Error::custom(format!("unsupported code `{value}`")))
    }
}
use url::Url;

#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProductImageDocument {
    pub url: Url,
    #[serde(with = "prohibited_content")]
    pub prohibited_content: ProhibitedContent,
}

impl From<ProductImage> for ProductImageDocument {
    fn from(value: ProductImage) -> Self {
        Self {
            url: value.url,
            prohibited_content: value.prohibited_content,
        }
    }
}

impl From<ProductImageDocument> for ProductImage {
    fn from(value: ProductImageDocument) -> Self {
        Self {
            url: value.url,
            prohibited_content: value.prohibited_content,
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
