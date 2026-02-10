use crate::category::core::Category;
use common::{
    category_key::{CategoryId, CategoryKey},
    error::missing_field::MissingRequiredField,
    language::domain::Language,
};
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use std::collections::HashMap;
use strum::EnumCount;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
#[serde(rename_all = "camelCase")]
pub struct CategoryDocument {
    pub category_id: CategoryId,
    pub category_key: CategoryKey,
    pub meta_name: String,
    pub meta_description: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub meta_keywords: Vec<String>,
    pub embedding: Vec<f32>,

    pub display_name_de: String,
    pub display_name_en: String,
    pub display_name_fr: String,
    pub display_name_es: String,
    pub display_description_de: String,
    pub display_description_en: String,
    pub display_description_fr: String,
    pub display_description_es: String,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl CategoryDocument {
    pub fn _id(&self) -> &CategoryKey {
        &self.category_key
    }
}

impl From<CategoryDocument> for Category {
    fn from(document: CategoryDocument) -> Self {
        let mut display_name = HashMap::with_capacity(Language::COUNT);
        display_name.insert(Language::De, document.display_name_de.into());
        display_name.insert(Language::En, document.display_name_en.into());
        display_name.insert(Language::Fr, document.display_name_fr.into());
        display_name.insert(Language::Es, document.display_name_es.into());
        let mut display_description = HashMap::with_capacity(Language::COUNT);
        display_description.insert(Language::De, document.display_description_de.into());
        display_description.insert(Language::En, document.display_description_en.into());
        display_description.insert(Language::Fr, document.display_description_fr.into());
        display_description.insert(Language::Es, document.display_description_es.into());

        Self {
            category_id: document.category_id,
            category_key: document.category_key,
            meta_name: document.meta_name.into(),
            meta_description: document.meta_description.into(),
            meta_keywords: document.meta_keywords.into_iter().map(Into::into).collect(),
            embedding: document.embedding,
            display_name,
            display_description,
            created: document.created,
            updated: document.updated,
        }
    }
}

impl TryFrom<Category> for CategoryDocument {
    type Error = MissingRequiredField;

    fn try_from(category: Category) -> Result<Self, Self::Error> {
        let mut category = category;
        Ok(Self {
            category_id: category.category_id,
            category_key: category.category_key,
            meta_name: category.meta_name.into(),
            meta_description: category.meta_description.into(),
            meta_keywords: category.meta_keywords.into_iter().map(Into::into).collect(),
            embedding: category.embedding,
            display_name_de: category
                .display_name
                .remove(&Language::De)
                .ok_or(MissingRequiredField::new("display_name_de"))?
                .into(),
            display_name_en: category
                .display_name
                .remove(&Language::En)
                .ok_or(MissingRequiredField::new("display_name_en"))?
                .into(),
            display_name_fr: category
                .display_name
                .remove(&Language::Fr)
                .ok_or(MissingRequiredField::new("display_name_fr"))?
                .into(),
            display_name_es: category
                .display_name
                .remove(&Language::Es)
                .ok_or(MissingRequiredField::new("display_name_es"))?
                .into(),
            display_description_de: category
                .display_description
                .remove(&Language::De)
                .ok_or(MissingRequiredField::new("display_description_de"))?
                .into(),
            display_description_en: category
                .display_description
                .remove(&Language::En)
                .ok_or(MissingRequiredField::new("display_description_en"))?
                .into(),
            display_description_fr: category
                .display_description
                .remove(&Language::Fr)
                .ok_or(MissingRequiredField::new("display_description_fr"))?
                .into(),
            display_description_es: category
                .display_description
                .remove(&Language::Es)
                .ok_or(MissingRequiredField::new("display_description_es"))?
                .into(),
            created: category.created,
            updated: category.updated,
        })
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, Rng};

    impl Dummy<Faker> for CategoryDocument {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            config.fake_with_rng::<Category, R>(rng).try_into().unwrap()
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::category::document::CategoryDocument;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_category_document() {
            Faker.fake::<CategoryDocument>();
        }
    }
}
