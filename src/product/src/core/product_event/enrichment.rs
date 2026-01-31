use crate::core::{
    authenticity::Authenticity, condition::Condition, description::Description,
    provenance::Provenance, restoration::Restoration, title::Title,
};
use common::{
    language::domain::Language, shop_id::ShopId, shops_product_id::ShopsProductId, year::Year,
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
pub struct TranslationProductEnrichmentEventPayload<T> {
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
