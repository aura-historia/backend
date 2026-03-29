use common::{
    language::domain::Language,
    localized::Localized,
    period_key::{PeriodId, PeriodKey},
    string_newtype,
};
use std::collections::HashMap;
use time::OffsetDateTime;
use tracing::error;

string_newtype!(PeriodMetaName);
string_newtype!(PeriodMetaDescription);
string_newtype!(PeriodMetaKeyword);
string_newtype!(PeriodName);
string_newtype!(PeriodDescription);

#[derive(Debug, Clone, PartialEq)]
pub struct Period {
    pub period_id: PeriodId,
    pub period_key: PeriodKey,
    pub meta_name: PeriodMetaName,
    pub meta_description: PeriodMetaDescription,
    pub meta_keywords: Vec<PeriodMetaKeyword>,
    pub embedding: Vec<f32>,
    pub display_name: HashMap<Language, PeriodName>,
    pub display_description: HashMap<Language, PeriodDescription>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

impl Period {
    pub fn embedding_text(&self) -> String {
        format!(
            "{} [SEP] {} [SEP] {}",
            self.meta_name,
            self.meta_description,
            self.meta_keywords
                .iter()
                .map(|k| k.to_string())
                .collect::<Vec<String>>()
                .join(", ")
        )
    }

    pub fn localized(self, preferred_languages: &[Language]) -> LocalizedPeriod {
        LocalizedPeriod {
            period_id: self.period_id.clone(),
            period_key: self.period_key.clone(),
            display_name: Language::resolve(preferred_languages, self.display_name).unwrap_or_else(
                || {
                    error!(field = "display_name", "Failed resolving field.");
                    Localized {
                        localization: Language::En,
                        payload: "period-name temporarily unavailable".into(),
                    }
                },
            ),
            display_description: Language::resolve(preferred_languages, self.display_description)
                .unwrap_or_else(|| {
                    error!(field = "display_description", "Failed resolving field.");
                    Localized {
                        localization: Language::En,
                        payload: "period-description temporarily unavailable".into(),
                    }
                }),
            created: self.created,
            updated: self.updated,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalizedPeriod {
    pub period_id: PeriodId,
    pub period_key: PeriodKey,
    pub display_name: Localized<Language, PeriodName>,
    pub display_description: Localized<Language, PeriodDescription>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[cfg(feature = "test-data")]
pub mod faker {
    use super::*;
    use common::slug_id::SlugId;
    use fake::{Dummy, Fake, Faker, RngExt};
    use serde::{Deserialize, Serialize};
    use strum::{EnumCount, IntoEnumIterator};

    static PERIODS_DATA: &str = include_str!(concat!(
        env!("CARGO_WORKSPACE_DIR"),
        "src/product-classification/data/periods.json"
    ));

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct PeriodTestPayload {
        period_id: String,
        period_key: String,
        meta_name: String,
        meta_description: String,
        meta_keywords: Vec<String>,
        display_name_de: String,
        display_name_en: String,
        display_name_fr: String,
        display_name_es: String,
        display_name_it: String,
        display_description_de: String,
        display_description_en: String,
        display_description_fr: String,
        display_description_es: String,
        display_description_it: String,
    }

    impl From<PeriodTestPayload> for Period {
        fn from(payload: PeriodTestPayload) -> Self {
            let mut display_name = HashMap::with_capacity(Language::COUNT);
            display_name.insert(Language::De, PeriodName(payload.display_name_de));
            display_name.insert(Language::En, PeriodName(payload.display_name_en));
            display_name.insert(Language::Fr, PeriodName(payload.display_name_fr));
            display_name.insert(Language::Es, PeriodName(payload.display_name_es));
            display_name.insert(Language::It, PeriodName(payload.display_name_it));
            let mut display_description = HashMap::with_capacity(Language::COUNT);
            display_description.insert(
                Language::De,
                PeriodDescription(payload.display_description_de),
            );
            display_description.insert(
                Language::En,
                PeriodDescription(payload.display_description_en),
            );
            display_description.insert(
                Language::Fr,
                PeriodDescription(payload.display_description_fr),
            );
            display_description.insert(
                Language::Es,
                PeriodDescription(payload.display_description_es),
            );
            display_description.insert(
                Language::It,
                PeriodDescription(payload.display_description_it),
            );
            Period {
                period_id: payload.period_id.into(),
                period_key: payload.period_key.into(),
                meta_name: payload.meta_name.into(),
                meta_description: payload.meta_description.into(),
                meta_keywords: payload
                    .meta_keywords
                    .into_iter()
                    .map(PeriodMetaKeyword)
                    .collect(),
                embedding: fake::vec![f32; 1024],
                display_name,
                display_description,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        }
    }

    impl Period {
        pub fn load_periods() -> Vec<Self> {
            serde_json::from_str::<Vec<PeriodTestPayload>>(PERIODS_DATA)
                .expect("shouldn't fail parsing periods data")
                .into_iter()
                .map(Period::from)
                .collect()
        }
    }

    impl Dummy<Faker> for Period {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let mut display_name = HashMap::new();
            for language in Language::iter() {
                display_name.insert(language, config.fake_with_rng(rng));
            }
            let mut display_description = HashMap::new();
            for language in Language::iter() {
                display_description.insert(language, config.fake_with_rng(rng));
            }
            let period_key: PeriodKey = config.fake_with_rng(rng);
            Period {
                period_id: SlugId::from(period_key.as_ref()),
                period_key,
                meta_name: config.fake_with_rng(rng),
                meta_description: config.fake_with_rng(rng),
                meta_keywords: config.fake_with_rng(rng),
                embedding: fake::vec![f32; 1024],
                display_name,
                display_description,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::period::core::Period;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_period() {
            Faker.fake::<Period>();
        }
    }
}
