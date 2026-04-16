use common::{currency::data::CurrencyData, language::data::LanguageData};
use serde::{Deserialize, Serialize};
use serde_email::Email;
use user::core::{first_name::FirstName, last_name::LastName};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PutNewsletterSubscriptionData {
    pub email: Email,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_name: Option<FirstName>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_name: Option<LastName>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<LanguageData>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<CurrencyData>,
}

#[cfg(test)]
mod fake {
    use super::PutNewsletterSubscriptionData;
    use fake::{Fake, faker::internet::en::SafeEmail};

    impl fake::Dummy<fake::Faker> for PutNewsletterSubscriptionData {
        fn dummy_with_rng<R: fake::rand::RngExt + ?Sized>(
            config: &fake::Faker,
            rng: &mut R,
        ) -> Self {
            let email_str: String = SafeEmail().fake_with_rng(rng);
            PutNewsletterSubscriptionData {
                email: email_str.try_into().unwrap(),
                first_name: config.fake_with_rng(rng),
                last_name: config.fake_with_rng(rng),
                language: config.fake_with_rng(rng),
                currency: config.fake_with_rng(rng),
            }
        }
    }
}
