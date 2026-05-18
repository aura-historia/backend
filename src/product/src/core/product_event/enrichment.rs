use crate::core::title::Title;
use common::{
    has_key::HasKey, language::domain::Language, product_id::ProductKey, shop_id::ShopId,
    shops_product_id::ShopsProductId,
};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum ProductEnrichmentEventPayload {
    TranslatedTitle(TranslationProductEnrichmentEventPayload<Title>),
    Embedded(EmbeddedProductEnrichmentEventPayload),
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq)]
pub struct TranslationProductEnrichmentEventPayload<T: Into<String> + From<String>> {
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub source_language: Language,
    pub target_language: Language,
    pub target: T,
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddedProductEnrichmentEventPayload {
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub embedding: Vec<f32>,
    pub native_title: Option<Title>,
    /// The language of [`Self::native_title`].  Present when `native_title` is `Some`.
    pub native_title_language: Option<Language>,
}

impl HasKey for ProductEnrichmentEventPayload {
    type Key = ProductKey;

    fn key(&self) -> Self::Key {
        match self {
            ProductEnrichmentEventPayload::TranslatedTitle(payload) => payload.key(),
            ProductEnrichmentEventPayload::Embedded(payload) => payload.key(),
        }
    }
}

impl<T> HasKey for TranslationProductEnrichmentEventPayload<T>
where
    T: Into<String> + From<String>,
{
    type Key = ProductKey;

    fn key(&self) -> Self::Key {
        ProductKey::new(self.shop_id, self.shops_product_id.clone())
    }
}

impl HasKey for EmbeddedProductEnrichmentEventPayload {
    type Key = ProductKey;

    fn key(&self) -> Self::Key {
        ProductKey::new(self.shop_id, self.shops_product_id.clone())
    }
}

impl ProductEnrichmentEventPayload {
    pub fn event_type(&self) -> &'static str {
        match self {
            ProductEnrichmentEventPayload::TranslatedTitle(_) => "ENRICHMENT_TRANSLATED_TITLE",
            ProductEnrichmentEventPayload::Embedded(_) => "ENRICHMENT_EMBEDDED",
        }
    }

    pub fn as_translated_title(&self) -> Option<&TranslationProductEnrichmentEventPayload<Title>> {
        match self {
            ProductEnrichmentEventPayload::TranslatedTitle(payload) => Some(payload),
            _ => None,
        }
    }

    pub fn as_embedded(&self) -> Option<&EmbeddedProductEnrichmentEventPayload> {
        match self {
            ProductEnrichmentEventPayload::Embedded(payload) => Some(payload),
            _ => None,
        }
    }
}
