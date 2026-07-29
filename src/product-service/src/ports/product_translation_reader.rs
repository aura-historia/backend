#![allow(dead_code)]

use common::language::domain::Language;
use common::localized::Localized;
use common::product_id::ProductId;
use product_core::description::Description;
use product_core::title::Title;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub struct ProductTranslationsView {
    pub product_id: ProductId,
    pub titles: HashMap<Language, Title>,
    pub descriptions: HashMap<Language, Description>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProductTranslationReadError {
    #[error("product translation lookup failed")]
    ProductTranslationLookupFailed,
    #[error("product translation read model is invalid")]
    ProductTranslationReadModelInvalid,
}

#[async_trait::async_trait]
pub trait ProductTranslationReader: Send {
    async fn find_for_product(
        &mut self,
        product_id: ProductId,
    ) -> Result<ProductTranslationsView, ProductTranslationReadError>;
}

pub trait ProductTranslationReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ProductTranslationReader + 'tx;
}

impl ProductTranslationsView {
    pub fn resolve_title(
        &self,
        title: Option<Localized<Language, Title>>,
        preferred_languages: &[Language],
    ) -> Option<Localized<Language, Title>> {
        let mut titles = self.titles.clone();
        if let Some(title) = title {
            titles.entry(title.localization).or_insert(title.payload);
        }
        Language::resolve(preferred_languages, titles)
    }

    pub fn resolve_description(
        &self,
        description: Option<Localized<Language, Description>>,
        preferred_languages: &[Language],
    ) -> Option<Localized<Language, Description>> {
        let mut descriptions = self.descriptions.clone();
        if let Some(description) = description {
            descriptions
                .entry(description.localization)
                .or_insert(description.payload);
        }
        Language::resolve(preferred_languages, descriptions)
    }
}
