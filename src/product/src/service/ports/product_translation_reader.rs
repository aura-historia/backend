#![allow(dead_code)]

use crate::core::description::Description;
use crate::core::title::Title;
use common::language::domain::Language;
use common::localized::Localized;
use common::product_id::ProductId;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub struct ProductTranslationsView {
    pub product_id: ProductId,
    pub titles: HashMap<Language, Title>,
    pub descriptions: HashMap<Language, Description>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProductTranslationReadError {
    #[error("temporary product translation read failure")]
    TemporarilyUnavailable,
    #[error("invalid product translation read model")]
    InvalidReadModel,
    #[error("internal product translation read failure")]
    Internal,
}

#[async_trait::async_trait]
pub(crate) trait ProductTranslationReader: Send + Sync {
    async fn find_for_product(
        &self,
        product_id: ProductId,
    ) -> Result<ProductTranslationsView, ProductTranslationReadError>;
}

impl ProductTranslationsView {
    pub fn resolve_title(
        &self,
        native: Localized<Language, Title>,
        preferred_languages: &[Language],
    ) -> Localized<Language, Title> {
        let mut titles = self.titles.clone();
        titles.entry(native.localization).or_insert(native.payload);
        Language::resolve(preferred_languages, titles)
            .unwrap_or_else(|| Localized::new(Language::En, Title::from("Unknown title")))
    }

    pub fn resolve_description(
        &self,
        native: Option<Localized<Language, Description>>,
        preferred_languages: &[Language],
    ) -> Option<Localized<Language, Description>> {
        let mut descriptions = self.descriptions.clone();
        if let Some(native) = native {
            descriptions
                .entry(native.localization)
                .or_insert(native.payload);
        }
        Language::resolve(preferred_languages, descriptions)
    }
}
