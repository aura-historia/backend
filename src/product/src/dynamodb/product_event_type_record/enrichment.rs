use crate::core::product_event::enrichment::ProductEnrichmentEventPayload;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProductEnrichmentEventTypeRecord {
    EnrichmentTranslatedTitle,
    EnrichmentEmbedded,
}

impl ProductEnrichmentEventTypeRecord {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProductEnrichmentEventTypeRecord::EnrichmentTranslatedTitle => {
                "ENRICHMENT_TRANSLATED_TITLE"
            }
            ProductEnrichmentEventTypeRecord::EnrichmentEmbedded => "ENRICHMENT_EMBEDDED",
        }
    }
}

impl From<&ProductEnrichmentEventPayload> for ProductEnrichmentEventTypeRecord {
    fn from(domain: &ProductEnrichmentEventPayload) -> Self {
        match domain {
            ProductEnrichmentEventPayload::TranslatedTitle(_) => {
                ProductEnrichmentEventTypeRecord::EnrichmentTranslatedTitle
            }
            ProductEnrichmentEventPayload::Embedded(_) => {
                ProductEnrichmentEventTypeRecord::EnrichmentEmbedded
            }
        }
    }
}
