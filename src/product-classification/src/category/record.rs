use std::collections::HashMap;

use crate::category::core::Category;
use common::{
    category_key::{CategoryId, CategoryKey},
    error::missing_field::MissingRequiredField,
    language::domain::Language,
};
use serde::{Deserialize, Serialize};
use strum::EnumCount;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CategoryRecord {
    pub pk: String,
    pub sk: String,

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
    pub display_name_it: String,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

pub fn mk_pk() -> &'static str {
    "global#categories"
}

pub fn mk_sk(category_id: &CategoryId) -> String {
    format!("category#{category_id}")
}

impl TryFrom<Category> for CategoryRecord {
    type Error = MissingRequiredField;

    fn try_from(category: Category) -> Result<Self, Self::Error> {
        let mut category = category;
        Ok(Self {
            pk: mk_pk().to_string(),
            sk: mk_sk(&category.category_id),
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
            display_name_it: category
                .display_name
                .remove(&Language::It)
                .ok_or(MissingRequiredField::new("display_name_it"))?
                .into(),
            created: category.created,
            updated: category.updated,
        })
    }
}

impl From<CategoryRecord> for Category {
    fn from(record: CategoryRecord) -> Self {
        let mut display_name = HashMap::with_capacity(Language::COUNT);
        display_name.insert(Language::De, record.display_name_de.into());
        display_name.insert(Language::En, record.display_name_en.into());
        display_name.insert(Language::Fr, record.display_name_fr.into());
        display_name.insert(Language::Es, record.display_name_es.into());
        display_name.insert(Language::It, record.display_name_it.into());
        Self {
            category_id: record.category_id,
            category_key: record.category_key,
            meta_name: record.meta_name.into(),
            meta_description: record.meta_description.into(),
            meta_keywords: record.meta_keywords.into_iter().map(Into::into).collect(),
            embedding: record.embedding,
            display_name,
            created: record.created,
            updated: record.updated,
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for CategoryRecord {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            config.fake_with_rng::<Category, R>(rng).try_into().unwrap()
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::category::record::CategoryRecord;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_category_record() {
            Faker.fake::<CategoryRecord>();
        }
    }
}
