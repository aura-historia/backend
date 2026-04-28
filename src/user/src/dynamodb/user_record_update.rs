use crate::{
    core::{first_name::FirstName, last_name::LastName},
    dynamodb::{role_record::UserRoleRecord, tier_record::UserTierRecord},
};
use common::{
    currency::record::CurrencyRecord, dynamodb_update::DynamoDbUpdate,
    language::record::LanguageRecord, stripe_customer_id::StripeCustomerId,
};
use isocountry::CountryCode;
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
pub struct UserRecordUpdate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_name: Option<FirstName>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_name: Option<LastName>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<LanguageRecord>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<CurrencyRecord>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prohibited_content_consent: Option<bool>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<UserTierRecord>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<UserRoleRecord>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stripe_customer_id: Option<StripeCustomerId>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gsi1_pk: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gsi1_sk: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_address_addressline: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_address_addressline_extra: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_address_locality: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_address_region: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_address_postal_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_address_country: Option<CountryCode>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geo_address_lat: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geo_address_lon: Option<f64>,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl DynamoDbUpdate for UserRecordUpdate {}

#[cfg(feature = "test-data")]
mod fake {
    use crate::dynamodb::user_record_update::UserRecordUpdate;
    use fake::Fake;
    use time::OffsetDateTime;

    impl fake::Dummy<fake::Faker> for UserRecordUpdate {
        fn dummy_with_rng<R: fake::rand::RngExt + ?Sized>(
            config: &fake::Faker,
            rng: &mut R,
        ) -> Self {
            UserRecordUpdate {
                first_name: config.fake_with_rng(rng),
                last_name: config.fake_with_rng(rng),
                language: config.fake_with_rng(rng),
                currency: config.fake_with_rng(rng),
                prohibited_content_consent: config.fake_with_rng(rng),
                tier: config.fake_with_rng(rng),
                role: config.fake_with_rng(rng),
                stripe_customer_id: config.fake_with_rng(rng),
                gsi1_pk: config.fake_with_rng(rng),
                gsi1_sk: config.fake_with_rng(rng),
                structured_address_addressline: config.fake_with_rng(rng),
                structured_address_addressline_extra: config.fake_with_rng(rng),
                structured_address_locality: config.fake_with_rng(rng),
                structured_address_region: config.fake_with_rng(rng),
                structured_address_postal_code: config.fake_with_rng(rng),
                structured_address_country: None,
                geo_address_lat: config.fake_with_rng(rng),
                geo_address_lon: config.fake_with_rng(rng),
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
