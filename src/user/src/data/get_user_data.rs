use crate::{
    core::{first_name::FirstName, last_name::LastName, user::User},
    data::{role_data::UserRoleData, tier_data::UserTierData},
};
use common::{
    actor::data::ActorData, currency::data::CurrencyData, language::data::LanguageData,
    measurement_unit::data::MeasurementUnitData, shop_id::ShopId,
    stripe_customer_id::StripeCustomerId, user_id::UserId,
};
use geo::data::address_data::{GeoAddressData, StructuredAddressData};
use serde::{Deserialize, Serialize};
use serde_email::Email;
use std::collections::HashSet;
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

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measurement_unit: Option<MeasurementUnitData>,

    pub prohibited_content_consent: bool,

    pub tier: UserTierData,

    pub role: UserRoleData,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stripe_customer_id: Option<StripeCustomerId>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_address: Option<StructuredAddressData>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geo_address: Option<GeoAddressData>,

    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    pub partner_shops: HashSet<ShopId>,

    pub created_by: ActorData,
    pub updated_by: ActorData,
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
            measurement_unit: user.measurement_unit.map(MeasurementUnitData::from),
            prohibited_content_consent: user.prohibited_content_consent,
            tier: UserTierData::from(user.tier),
            role: UserRoleData::from(user.role),
            stripe_customer_id: user.stripe_customer_id,
            structured_address: user.structured_address.map(StructuredAddressData::from),
            geo_address: user.geo_address.map(GeoAddressData::from),
            partner_shops: user.partner_shops,
            created_by: user.created_by.into(),
            updated_by: user.updated_by.into(),
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
