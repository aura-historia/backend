use crate::core::{
    authenticity::Authenticity, condition::Condition, provenance::Provenance,
    restoration::Restoration, title::Title,
};
use common::{
    category_key::CategoryId, has_key::HasKey, language::domain::Language, period_key::PeriodId,
    product_id::ProductKey, shop_id::ShopId, shops_product_id::ShopsProductId, year::Year,
};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum ProductEnrichmentEventPayload {
    TranslatedTitle(TranslationProductEnrichmentEventPayload<Title>),
    Embedded(EmbeddedProductEnrichmentEventPayload),
    ExtractedAttributes(ExtractedAttributesProductEnrichmentEventPayload),
    ClassifiedCategory(ClassifiedCategoryProductEnrichmentEventPayload),
    ClassifiedPeriod(ClassifiedPeriodProductEnrichmentEventPayload),
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
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq)]
pub struct ExtractedAttributesProductEnrichmentEventPayload {
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub origin_year_min: Option<Year>,
    pub origin_year: Option<Year>,
    pub origin_year_max: Option<Year>,
    pub authenticity: Option<Authenticity>,
    pub condition: Option<Condition>,
    pub provenance: Option<Provenance>,
    pub restoration: Option<Restoration>,
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq)]
pub struct ClassifiedCategoryProductEnrichmentEventPayload {
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub category_id: CategoryId,
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq)]
pub struct ClassifiedPeriodProductEnrichmentEventPayload {
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub period_id: PeriodId,
}

impl HasKey for ProductEnrichmentEventPayload {
    type Key = ProductKey;

    fn key(&self) -> Self::Key {
        match self {
            ProductEnrichmentEventPayload::TranslatedTitle(payload) => payload.key(),
            ProductEnrichmentEventPayload::Embedded(payload) => payload.key(),
            ProductEnrichmentEventPayload::ExtractedAttributes(payload) => payload.key(),
            ProductEnrichmentEventPayload::ClassifiedCategory(payload) => payload.key(),
            ProductEnrichmentEventPayload::ClassifiedPeriod(payload) => payload.key(),
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

impl HasKey for ExtractedAttributesProductEnrichmentEventPayload {
    type Key = ProductKey;

    fn key(&self) -> Self::Key {
        ProductKey::new(self.shop_id, self.shops_product_id.clone())
    }
}

impl HasKey for ClassifiedCategoryProductEnrichmentEventPayload {
    type Key = ProductKey;

    fn key(&self) -> Self::Key {
        ProductKey::new(self.shop_id, self.shops_product_id.clone())
    }
}

impl HasKey for ClassifiedPeriodProductEnrichmentEventPayload {
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
            ProductEnrichmentEventPayload::ExtractedAttributes(_) => {
                "ENRICHMENT_EXTRACTED_ATTRIBUTES"
            }
            ProductEnrichmentEventPayload::ClassifiedCategory(_) => {
                "ENRICHMENT_CLASSIFIED_CATEGORY"
            }
            ProductEnrichmentEventPayload::ClassifiedPeriod(_) => "ENRICHMENT_CLASSIFIED_PERIOD",
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

    pub fn as_extracted_attributes(
        &self,
    ) -> Option<&ExtractedAttributesProductEnrichmentEventPayload> {
        match self {
            ProductEnrichmentEventPayload::ExtractedAttributes(payload) => Some(payload),
            _ => None,
        }
    }

    pub fn as_classified_category(
        &self,
    ) -> Option<&ClassifiedCategoryProductEnrichmentEventPayload> {
        match self {
            ProductEnrichmentEventPayload::ClassifiedCategory(payload) => Some(payload),
            _ => None,
        }
    }

    pub fn as_classified_period(&self) -> Option<&ClassifiedPeriodProductEnrichmentEventPayload> {
        match self {
            ProductEnrichmentEventPayload::ClassifiedPeriod(payload) => Some(payload),
            _ => None,
        }
    }
}
