use crate::{
    core::{
        first_name::FirstName, last_name::LastName, role::UserRole, tier::UserTier, user::User,
    },
    dynamodb::user_record::UserRecord,
};
use common::{
    currency::record::CurrencyRecord, language::record::LanguageRecord,
    stripe_customer_id::StripeCustomerId, user_id::UserId,
};
use serde::{Deserialize, Serialize};
use serde_email::Email;
use serde_fields::SerdeField;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
#[serde(rename_all = "camelCase")]
pub struct UserDocument {
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
    pub prohibited_content_consent: bool,
    pub tier: UserTier,
    pub role: UserRole,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stripe_customer_id: Option<StripeCustomerId>,
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl UserDocument {
    pub fn _id(&self) -> UserId {
        self.user_id
    }
}

impl From<User> for UserDocument {
    fn from(user: User) -> Self {
        UserDocument {
            user_id: user.user_id,
            email: user.email,
            first_name: user.first_name,
            last_name: user.last_name,
            language: user.language.map(LanguageRecord::from),
            currency: user.currency.map(CurrencyRecord::from),
            prohibited_content_consent: user.prohibited_content_consent,
            tier: user.tier,
            role: user.role,
            stripe_customer_id: user.stripe_customer_id,
            created: user.created,
            updated: user.updated,
        }
    }
}

impl From<UserDocument> for User {
    fn from(document: UserDocument) -> Self {
        User {
            user_id: document.user_id,
            email: document.email,
            first_name: document.first_name,
            last_name: document.last_name,
            language: document.language.map(Into::into),
            currency: document.currency.map(Into::into),
            prohibited_content_consent: document.prohibited_content_consent,
            tier: document.tier,
            role: document.role,
            stripe_customer_id: document.stripe_customer_id,
            created: document.created,
            updated: document.updated,
        }
    }
}

impl From<UserRecord> for UserDocument {
    fn from(record: UserRecord) -> Self {
        UserDocument::from(User::from(record))
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for UserDocument {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            config.fake_with_rng::<User, _>(rng).into()
        }
    }
}
