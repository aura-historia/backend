use std::collections::HashMap;

use crate::period::core::Period;
use common::{
    error::missing_field::MissingRequiredField,
    language::domain::Language,
    period_key::{PeriodId, PeriodKey},
};
use serde::{Deserialize, Serialize};
use strum::EnumCount;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PeriodRecord {
    pub pk: String,
    pub sk: String,

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

pub fn mk_pk() -> &'static str {
    "global#periods"
}

pub fn mk_sk(period_id: &PeriodId) -> String {
    format!("period#{period_id}")
}

impl TryFrom<Period> for PeriodRecord {
    type Error = MissingRequiredField;

    fn try_from(period: Period) -> Result<Self, Self::Error> {
        let mut period = period;
        Ok(Self {
            pk: mk_pk().to_string(),
            sk: mk_sk(&period.period_id),
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

impl From<PeriodRecord> for Period {
    fn from(record: PeriodRecord) -> Self {
        let mut display_name = HashMap::with_capacity(Language::COUNT);
        display_name.insert(Language::De, record.display_name_de.into());
        display_name.insert(Language::En, record.display_name_en.into());
        display_name.insert(Language::Fr, record.display_name_fr.into());
        display_name.insert(Language::Es, record.display_name_es.into());
        display_name.insert(Language::It, record.display_name_it.into());
        let mut display_description = HashMap::with_capacity(Language::COUNT);
        display_description.insert(Language::De, record.display_description_de.into());
        display_description.insert(Language::En, record.display_description_en.into());
        display_description.insert(Language::Fr, record.display_description_fr.into());
        display_description.insert(Language::Es, record.display_description_es.into());
        display_description.insert(Language::It, record.display_description_it.into());
        Self {
            period_id: record.period_id,
            period_key: record.period_key,
            meta_name: record.meta_name.into(),
            meta_description: record.meta_description.into(),
            meta_keywords: record.meta_keywords.into_iter().map(Into::into).collect(),
            embedding: record.embedding,
            display_name,
            display_description,
            created: record.created,
            updated: record.updated,
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, Rng};

    impl Dummy<Faker> for PeriodRecord {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            config.fake_with_rng::<Period, R>(rng).try_into().unwrap()
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::period::record::PeriodRecord;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_period_record() {
            Faker.fake::<PeriodRecord>();
        }
    }
}
