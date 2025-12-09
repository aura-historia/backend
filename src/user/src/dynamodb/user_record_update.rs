use crate::core::{first_name::FirstName, last_name::LastName};
use common::{
    currency::record::CurrencyRecord, dynamodb_update::DynamoDbUpdate,
    language::record::LanguageRecord,
};
use serde::{Deserialize, Serialize};
use serde_email::Email;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserRecordUpdate {
    pub email: Option<Email>,
    pub first_name: Option<FirstName>,
    pub last_name: Option<LastName>,
    pub language: Option<LanguageRecord>,
    pub currency: Option<CurrencyRecord>,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl DynamoDbUpdate for UserRecordUpdate {}

#[cfg(feature = "test-data")]
mod fake {
    use crate::dynamodb::user_record_update::UserRecordUpdate;
    use fake::{Fake, faker::internet::en::SafeEmail};
    use time::OffsetDateTime;

    impl fake::Dummy<fake::Faker> for UserRecordUpdate {
        fn dummy_with_rng<R: fake::rand::Rng + ?Sized>(config: &fake::Faker, rng: &mut R) -> Self {
            UserRecordUpdate {
                email: Some(
                    SafeEmail()
                        .fake_with_rng::<String, R>(rng)
                        .try_into()
                        .unwrap(),
                ),
                first_name: config.fake_with_rng(rng),
                last_name: config.fake_with_rng(rng),
                language: config.fake_with_rng(rng),
                currency: config.fake_with_rng(rng),
                updated: OffsetDateTime::now_utc(),
            }
        }
    }
}
