use crate::core::product_event::enrichment::ProductEnrichmentEventPayload;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProductEnrichmentEventTypeRecord {
    EnrichmentTranslatedTitle,
    EnrichmentTranslatedDescription,
    EnrichmentEmbedded,
    EnrichmentExtractedAttributes,
    EnrichmentClassifyCategory,
    EnrichmentClassifyPeriod,
}

impl From<&ProductEnrichmentEventPayload> for ProductEnrichmentEventTypeRecord {
    fn from(domain: &ProductEnrichmentEventPayload) -> Self {
        match domain {
            ProductEnrichmentEventPayload::TranslatedTitle(_) => {
                ProductEnrichmentEventTypeRecord::EnrichmentTranslatedTitle
            }
            ProductEnrichmentEventPayload::TranslatedDescription(_) => {
                ProductEnrichmentEventTypeRecord::EnrichmentTranslatedDescription
            }
            ProductEnrichmentEventPayload::Embedded(_) => {
                ProductEnrichmentEventTypeRecord::EnrichmentEmbedded
            }
            ProductEnrichmentEventPayload::ExtractedAttributes(_) => {
                ProductEnrichmentEventTypeRecord::EnrichmentExtractedAttributes
            }
            ProductEnrichmentEventPayload::ClassifiedCategory(_) => {
                ProductEnrichmentEventTypeRecord::EnrichmentClassifyCategory
            }
            ProductEnrichmentEventPayload::ClassifiedPeriod(_) => {
                ProductEnrichmentEventTypeRecord::EnrichmentClassifyPeriod
            }
        }
    }
}
