use crate::{
    core::{first_name::FirstName, last_name::LastName, user::User},
    dynamodb::{role_record::UserRoleRecord, tier_record::UserTierRecord},
};
use common::{
    currency::{domain::Currency, record::CurrencyRecord},
    language::{domain::Language, record::LanguageRecord},
    stripe_customer_id::StripeCustomerId,
    user_id::UserId,
};
use geo::dynamodb::{geo_address_from_record, structured_address_from_record};
use isocountry::CountryCode;
use serde::{Deserialize, Serialize};
use serde_email::Email;
use serde_fields::SerdeField;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
pub struct UserRecord {
    pub pk: String,
    pub sk: String,
    pub user_id: UserId,
    pub email: Email,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_name: Option<FirstName>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_name: Option<LastName>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<LanguageRecord>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<CurrencyRecord>,

    #[serde(default)]
    pub prohibited_content_consent: bool,

    pub tier: UserTierRecord,

    #[serde(default)]
    pub role: UserRoleRecord,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stripe_customer_id: Option<StripeCustomerId>,

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

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gsi1_pk: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gsi1_sk: Option<String>,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

pub fn mk_pk(user_id: &UserId) -> String {
    format!("user#{user_id}")
}

pub fn mk_sk() -> &'static str {
    "user#details"
}

pub fn mk_gsi1_pk(stripe_customer_id: &StripeCustomerId) -> String {
    format!("user#stripe_customer_id#{stripe_customer_id}")
}

pub fn mk_gsi1_sk() -> &'static str {
    "user#details"
}

impl From<User> for UserRecord {
    fn from(user: User) -> Self {
        let (gsi1_pk, gsi1_sk) = match user.stripe_customer_id.as_ref() {
            Some(scid) => (Some(mk_gsi1_pk(scid)), Some(mk_gsi1_sk().to_owned())),
            None => (None, None),
        };
        let structured_address = user.structured_address;
        let geo_address = user.geo_address;
        UserRecord {
            pk: mk_pk(&user.user_id),
            sk: mk_sk().to_owned(),
            user_id: user.user_id,
            email: user.email,
            first_name: user.first_name,
            last_name: user.last_name,
            language: user.language.map(LanguageRecord::from),
            currency: user.currency.map(CurrencyRecord::from),
            prohibited_content_consent: user.prohibited_content_consent,
            tier: UserTierRecord::from(user.tier),
            role: UserRoleRecord::from(user.role),
            stripe_customer_id: user.stripe_customer_id,
            structured_address_addressline: structured_address
                .as_ref()
                .and_then(|a| a.addressline.clone()),
            structured_address_addressline_extra: structured_address
                .as_ref()
                .and_then(|a| a.addressline_extra.clone()),
            structured_address_locality: structured_address
                .as_ref()
                .and_then(|a| a.locality.clone()),
            structured_address_region: structured_address.as_ref().and_then(|a| a.region.clone()),
            structured_address_postal_code: structured_address
                .as_ref()
                .and_then(|a| a.postal_code.clone()),
            structured_address_country: structured_address.as_ref().and_then(|a| a.country),
            geo_address_lat: geo_address.map(|address| address.lat),
            geo_address_lon: geo_address.map(|address| address.lon),
            gsi1_pk,
            gsi1_sk,
            created: user.created,
            updated: user.updated,
        }
    }
}

impl From<UserRecord> for User {
    fn from(record: UserRecord) -> Self {
        User {
            user_id: record.user_id,
            email: record.email,
            first_name: record.first_name,
            last_name: record.last_name,
            language: record.language.map(Language::from),
            currency: record.currency.map(Currency::from),
            prohibited_content_consent: record.prohibited_content_consent,
            tier: record.tier.into(),
            role: record.role.into(),
            stripe_customer_id: record.stripe_customer_id,
            structured_address: structured_address_from_record(
                record.structured_address_addressline,
                record.structured_address_addressline_extra,
                record.structured_address_locality,
                record.structured_address_region,
                record.structured_address_postal_code,
                record.structured_address_country,
            ),
            geo_address: geo_address_from_record(record.geo_address_lat, record.geo_address_lon),
            created: record.created,
            updated: record.updated,
        }
    }
}

#[cfg(feature = "test-data")]
mod fake {
    use crate::core::user::User;
    use crate::dynamodb::user_record::UserRecord;
    use fake::Fake;

    impl fake::Dummy<fake::Faker> for UserRecord {
        fn dummy_with_rng<R: fake::rand::RngExt + ?Sized>(
            config: &fake::Faker,
            rng: &mut R,
        ) -> Self {
            config.fake_with_rng::<User, R>(rng).into()
        }
    }
}
