use crate::title::Title;
use localization::Language;
use localization::Localized;

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
    pub source_language: Language,
    pub target_language: Language,
    pub target: T,
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddedProductEnrichmentEventPayload {
    pub embedding: Vec<f32>,
    pub title: Option<Localized<Language, Title>>,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn translation_payload() -> TranslationProductEnrichmentEventPayload<Title> {
        TranslationProductEnrichmentEventPayload {
            source_language: Language::De,
            target_language: Language::En,
            target: Title::from("Vase"),
        }
    }

    fn embedded_payload() -> EmbeddedProductEnrichmentEventPayload {
        EmbeddedProductEnrichmentEventPayload {
            embedding: vec![1.0, 2.0],
            title: Some(Localized::new(Language::De, Title::from("Vase"))),
        }
    }

    #[test]
    fn should_return_translation_event_type_and_accessor() {
        let payload = translation_payload();
        let event = ProductEnrichmentEventPayload::TranslatedTitle(payload.clone());

        assert_eq!("ENRICHMENT_TRANSLATED_TITLE", event.event_type());
        assert_eq!(Some(&payload), event.as_translated_title());
        assert!(event.as_embedded().is_none());
    }

    #[test]
    fn should_return_embedded_event_type_and_accessor() {
        let payload = embedded_payload();
        let event = ProductEnrichmentEventPayload::Embedded(payload.clone());

        assert_eq!("ENRICHMENT_EMBEDDED", event.event_type());
        assert_eq!(Some(&payload), event.as_embedded());
        assert!(event.as_translated_title().is_none());
    }
}
