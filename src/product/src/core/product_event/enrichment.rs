use crate::core::{
    authenticity::Authenticity, condition::Condition, description::Description,
    provenance::Provenance, restoration::Restoration, title::Title,
};
use common::{
    has_key::HasKey, language::domain::Language, product_id::ProductKey, shop_id::ShopId,
    shops_product_id::ShopsProductId, year::Year,
};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum ProductEnrichmentEventPayload {
    TranslatedTitle(TranslationProductEnrichmentEventPayload<Title>),
    TranslatedDescription(TranslationProductEnrichmentEventPayload<Description>),
    EmbeddedText(EmbeddedTextProductEnrichmentEventPayload),
    ExtractedAttributes(ExtractedAttributesProductEnrichmentEventPayload),
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq)]
pub struct TranslationProductEnrichmentEventPayload<T: Into<String> + From<String>> {
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub source_language: Language,
    pub target_language: Language,
    pub target: T,
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddedTextProductEnrichmentEventPayload {
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub embedding: Vec<f32>,
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq)]
pub struct ExtractedAttributesProductEnrichmentEventPayload {
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub origin_year_min: Option<Year>,
    pub origin_year: Option<Year>,
    pub origin_year_max: Option<Year>,
    pub authenticity: Option<Authenticity>,
    pub condition: Option<Condition>,
    pub provenance: Option<Provenance>,
    pub restoration: Option<Restoration>,
}

impl HasKey for ProductEnrichmentEventPayload {
    type Key = ProductKey;

    fn key(&self) -> Self::Key {
        match self {
            ProductEnrichmentEventPayload::TranslatedTitle(payload) => payload.key(),
            ProductEnrichmentEventPayload::TranslatedDescription(payload) => payload.key(),
            ProductEnrichmentEventPayload::EmbeddedText(payload) => payload.key(),
            ProductEnrichmentEventPayload::ExtractedAttributes(payload) => payload.key(),
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

impl HasKey for EmbeddedTextProductEnrichmentEventPayload {
    type Key = ProductKey;

    fn key(&self) -> Self::Key {
        ProductKey::new(self.shop_id, self.shops_product_id.clone())
    }
}

impl HasKey for ExtractedAttributesProductEnrichmentEventPayload {
    type Key = ProductKey;

    fn key(&self) -> Self::Key {
        ProductKey::new(self.shop_id, self.shops_product_id.clone())
    }
}

impl ProductEnrichmentEventPayload {
    pub fn as_translated_title(&self) -> Option<&TranslationProductEnrichmentEventPayload<Title>> {
        match self {
            ProductEnrichmentEventPayload::TranslatedTitle(payload) => Some(payload),
            _ => None,
        }
    }

    pub fn as_translated_description(
        &self,
    ) -> Option<&TranslationProductEnrichmentEventPayload<Description>> {
        match self {
            ProductEnrichmentEventPayload::TranslatedDescription(payload) => Some(payload),
            _ => None,
        }
    }

    pub fn as_embedded_text(&self) -> Option<&EmbeddedTextProductEnrichmentEventPayload> {
        match self {
            ProductEnrichmentEventPayload::EmbeddedText(payload) => Some(payload),
            _ => None,
        }
    }

    pub fn as_extracted_attributes(
        &self,
    ) -> Option<&ExtractedAttributesProductEnrichmentEventPayload> {
        match self {
            ProductEnrichmentEventPayload::ExtractedAttributes(payload) => Some(payload),
            _ => None,
        }
    }
}
