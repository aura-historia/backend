use crate::{
    core::{first_name::FirstName, last_name::LastName, user::User},
    data::{role_data::UserRoleData, tier_data::UserTierData},
};
use common::{
    currency::data::CurrencyData, language::data::LanguageData,
    stripe_customer_id::StripeCustomerId, user_id::UserId,
};
use serde::{Deserialize, Serialize};
use serde_email::Email;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetUserAccountData {
    pub user_id: UserId,
    pub email: Email,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_name: Option<FirstName>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_name: Option<LastName>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<LanguageData>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<CurrencyData>,

    pub prohibited_content_consent: bool,

    pub tier: UserTierData,

    pub role: UserRoleData,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stripe_customer_id: Option<StripeCustomerId>,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl From<User> for GetUserAccountData {
    fn from(user: User) -> Self {
        GetUserAccountData {
            user_id: user.user_id,
            email: user.email,
            first_name: user.first_name,
            last_name: user.last_name,
            language: user.language.map(LanguageData::from),
            currency: user.currency.map(CurrencyData::from),
            prohibited_content_consent: user.prohibited_content_consent,
            tier: UserTierData::from(user.tier),
            role: UserRoleData::from(user.role),
            stripe_customer_id: user.stripe_customer_id,
            created: user.created,
            updated: user.updated,
        }
    }
}

#[cfg(feature = "test-data")]
mod fake {
    use crate::core::user::User;
    use crate::data::get_user_data::GetUserAccountData;
    use fake::Fake;

    impl fake::Dummy<fake::Faker> for GetUserAccountData {
        fn dummy_with_rng<R: fake::rand::RngExt + ?Sized>(
            config: &fake::Faker,
            rng: &mut R,
        ) -> Self {
            config.fake_with_rng::<User, R>(rng).into()
        }
    }
}
