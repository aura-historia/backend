use crate::title::Title;
use common::{
    has_key::HasKey, language::domain::Language, localized::Localized, product_id::ProductKey,
    shop_id::ShopId, shops_product_id::ShopsProductId,
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
    pub native_title: Option<Localized<Language, Title>>,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn translation_payload() -> TranslationProductEnrichmentEventPayload<Title> {
        TranslationProductEnrichmentEventPayload {
            shop_id: ShopId::new(),
            seller_id: ShopId::new(),
            shops_product_id: ShopsProductId::from("sku-1"),
            source_language: Language::De,
            target_language: Language::En,
            target: Title::from("Vase"),
        }
    }

    fn embedded_payload() -> EmbeddedProductEnrichmentEventPayload {
        EmbeddedProductEnrichmentEventPayload {
            shop_id: ShopId::new(),
            seller_id: ShopId::new(),
            shops_product_id: ShopsProductId::from("sku-1"),
            embedding: vec![1.0, 2.0],
            native_title: Some(Localized::new(Language::De, Title::from("Vase"))),
        }
    }

    #[test]
    fn should_return_translation_event_type_key_and_accessors() {
        let payload = translation_payload();
        let key = payload.key();
        let event = ProductEnrichmentEventPayload::TranslatedTitle(payload.clone());

        assert_eq!("ENRICHMENT_TRANSLATED_TITLE", event.event_type());
        assert_eq!(key, payload.key());
        assert_eq!(key, event.key());
        assert_eq!(Some(&payload), event.as_translated_title());
        assert!(event.as_embedded().is_none());
    }

    #[test]
    fn should_return_embedded_event_type_key_and_accessors() {
        let payload = embedded_payload();
        let key = payload.key();
        let event = ProductEnrichmentEventPayload::Embedded(payload.clone());

        assert_eq!("ENRICHMENT_EMBEDDED", event.event_type());
        assert_eq!(key, payload.key());
        assert_eq!(key, event.key());
        assert_eq!(Some(&payload), event.as_embedded());
        assert!(event.as_translated_title().is_none());
    }
}
