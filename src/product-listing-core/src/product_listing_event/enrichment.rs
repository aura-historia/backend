use crate::title::Title;
use localization::Language;
use localization::Localized;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum ProductListingEnrichmentEventPayload {
    TranslatedTitle(TranslationProductListingEnrichmentEventPayload<Title>),
    Embedded(EmbeddedProductListingEnrichmentEventPayload),
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq)]
pub struct TranslationProductListingEnrichmentEventPayload<T: Into<String> + From<String>> {
    pub source_language: Language,
    pub target_language: Language,
    pub target: T,
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddedProductListingEnrichmentEventPayload {
    pub embedding: Vec<f32>,
    pub title: Option<Localized<Language, Title>>,
}

impl ProductListingEnrichmentEventPayload {
    pub fn event_type(&self) -> &'static str {
        match self {
            ProductListingEnrichmentEventPayload::TranslatedTitle(_) => {
                "ENRICHMENT_TRANSLATED_TITLE"
            }
            ProductListingEnrichmentEventPayload::Embedded(_) => "ENRICHMENT_EMBEDDED",
        }
    }

    pub fn as_translated_title(
        &self,
    ) -> Option<&TranslationProductListingEnrichmentEventPayload<Title>> {
        match self {
            ProductListingEnrichmentEventPayload::TranslatedTitle(payload) => Some(payload),
            _ => None,
        }
    }

    pub fn as_embedded(&self) -> Option<&EmbeddedProductListingEnrichmentEventPayload> {
        match self {
            ProductListingEnrichmentEventPayload::Embedded(payload) => Some(payload),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn translation_payload() -> TranslationProductListingEnrichmentEventPayload<Title> {
        TranslationProductListingEnrichmentEventPayload {
            source_language: Language::De,
            target_language: Language::En,
            target: Title::from("Vase"),
        }
    }

    fn embedded_payload() -> EmbeddedProductListingEnrichmentEventPayload {
        EmbeddedProductListingEnrichmentEventPayload {
            embedding: vec![1.0, 2.0],
            title: Some(Localized::new(Language::De, Title::from("Vase"))),
        }
    }

    #[test]
    fn should_return_translation_event_type_and_accessor() {
        let payload = translation_payload();
        let event = ProductListingEnrichmentEventPayload::TranslatedTitle(payload.clone());

        assert_eq!("ENRICHMENT_TRANSLATED_TITLE", event.event_type());
        assert_eq!(Some(&payload), event.as_translated_title());
        assert!(event.as_embedded().is_none());
    }

    #[test]
    fn should_return_embedded_event_type_and_accessor() {
        let payload = embedded_payload();
        let event = ProductListingEnrichmentEventPayload::Embedded(payload.clone());

        assert_eq!("ENRICHMENT_EMBEDDED", event.event_type());
        assert_eq!(Some(&payload), event.as_embedded());
        assert!(event.as_translated_title().is_none());
    }
}
