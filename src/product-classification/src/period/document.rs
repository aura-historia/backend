use crate::period::core::Period;
use common::{
    error::missing_field::MissingRequiredField,
    language::domain::Language,
    period_key::{PeriodId, PeriodKey},
};
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use std::collections::HashMap;
use strum::EnumCount;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
#[serde(rename_all = "camelCase")]
pub struct PeriodDocument {
    pub period_id: PeriodId,
    pub period_key: PeriodKey,
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
    pub display_description_de: String,
    pub display_description_en: String,
    pub display_description_fr: String,
    pub display_description_es: String,
    pub display_description_it: String,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl PeriodDocument {
    pub fn _id(&self) -> &PeriodKey {
        &self.period_key
    }
}

impl From<PeriodDocument> for Period {
    fn from(document: PeriodDocument) -> Self {
        let mut display_name = HashMap::with_capacity(Language::COUNT);
        display_name.insert(Language::De, document.display_name_de.into());
        display_name.insert(Language::En, document.display_name_en.into());
        display_name.insert(Language::Fr, document.display_name_fr.into());
        display_name.insert(Language::Es, document.display_name_es.into());
        display_name.insert(Language::It, document.display_name_it.into());
        let mut display_description = HashMap::with_capacity(Language::COUNT);
        display_description.insert(Language::De, document.display_description_de.into());
        display_description.insert(Language::En, document.display_description_en.into());
        display_description.insert(Language::Fr, document.display_description_fr.into());
        display_description.insert(Language::Es, document.display_description_es.into());
        display_description.insert(Language::It, document.display_description_it.into());

        Self {
            period_id: document.period_id,
            period_key: document.period_key,
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

impl TryFrom<Period> for PeriodDocument {
    type Error = MissingRequiredField;

    fn try_from(period: Period) -> Result<Self, Self::Error> {
        let mut period = period;
        Ok(Self {
            period_id: period.period_id,
            period_key: period.period_key,
            meta_name: period.meta_name.into(),
            meta_description: period.meta_description.into(),
            meta_keywords: period.meta_keywords.into_iter().map(Into::into).collect(),
            embedding: period.embedding,
            display_name_de: period
                .display_name
                .remove(&Language::De)
                .ok_or(MissingRequiredField::new("display_name_de"))?
                .into(),
            display_name_en: period
                .display_name
                .remove(&Language::En)
                .ok_or(MissingRequiredField::new("display_name_en"))?
                .into(),
            display_name_fr: period
                .display_name
                .remove(&Language::Fr)
                .ok_or(MissingRequiredField::new("display_name_fr"))?
                .into(),
            display_name_es: period
                .display_name
                .remove(&Language::Es)
                .ok_or(MissingRequiredField::new("display_name_es"))?
                .into(),
            display_name_it: period
                .display_name
                .remove(&Language::It)
                .ok_or(MissingRequiredField::new("display_name_it"))?
                .into(),
            display_description_de: period
                .display_description
                .remove(&Language::De)
                .ok_or(MissingRequiredField::new("display_description_de"))?
                .into(),
            display_description_en: period
                .display_description
                .remove(&Language::En)
                .ok_or(MissingRequiredField::new("display_description_en"))?
                .into(),
            display_description_fr: period
                .display_description
                .remove(&Language::Fr)
                .ok_or(MissingRequiredField::new("display_description_fr"))?
                .into(),
            display_description_es: period
                .display_description
                .remove(&Language::Es)
                .ok_or(MissingRequiredField::new("display_description_es"))?
                .into(),
            display_description_it: period
                .display_description
                .remove(&Language::It)
                .ok_or(MissingRequiredField::new("display_description_it"))?
                .into(),
            created: period.created,
            updated: period.updated,
        })
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for PeriodDocument {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            config.fake_with_rng::<Period, R>(rng).try_into().unwrap()
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::period::document::PeriodDocument;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_period_document() {
            Faker.fake::<PeriodDocument>();
        }
    }
}
