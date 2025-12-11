use crate::core::{first_name::FirstName, last_name::LastName};
use common::{
    currency::record::CurrencyRecord, dynamodb_update::DynamoDbUpdate,
    language::record::LanguageRecord,
};
use serde::{Deserialize, Serialize};
use serde_email::Email;
use serde_fields::SerdeField;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
pub struct UserRecordUpdate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<Email>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_name: Option<FirstName>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_name: Option<LastName>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<LanguageRecord>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
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

#[cfg(test)]
mod tests {
    use crate::dynamodb::{user_record::UserRecord, user_record_update::UserRecordUpdate};

    #[test]
    fn should_be_subset_of_user_record() {
        assert!(
            UserRecordUpdate::SERDE_FIELDS
                .iter()
                .all(|field| UserRecord::SERDE_FIELDS.contains(field))
        )
    }
}
