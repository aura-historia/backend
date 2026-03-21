use crate::core::{first_name::FirstName, last_name::LastName};
use common::{currency::data::CurrencyData, language::data::LanguageData};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchUserAccountData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_name: Option<FirstName>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_name: Option<LastName>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<LanguageData>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<CurrencyData>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prohibited_content_consent: Option<bool>,
}

#[cfg(feature = "test-data")]
mod fake {
    use crate::data::patch_user_data::PatchUserAccountData;
    use fake::Fake;

    impl fake::Dummy<fake::Faker> for PatchUserAccountData {
        fn dummy_with_rng<R: fake::rand::RngExt + ?Sized>(
            config: &fake::Faker,
            rng: &mut R,
        ) -> Self {
            PatchUserAccountData {
                first_name: config.fake_with_rng(rng),
                last_name: config.fake_with_rng(rng),
                language: config.fake_with_rng(rng),
                currency: config.fake_with_rng(rng),
                prohibited_content_consent: config.fake_with_rng(rng),
            }
        }
    }
}
