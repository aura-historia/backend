use crate::{
    core::{first_name::FirstName, last_name::LastName, user::User},
    dynamodb::tier_record::UserTierRecord,
};
use common::{
    currency::{domain::Currency, record::CurrencyRecord},
    language::{domain::Language, record::LanguageRecord},
    user_id::UserId,
};
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

impl From<User> for UserRecord {
    fn from(user: User) -> Self {
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
