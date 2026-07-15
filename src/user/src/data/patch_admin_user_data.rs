use crate::core::{first_name::FirstName, last_name::LastName};
use crate::data::{role_data::UserRoleData, tier_data::UserTierData};
use common::{
    currency::data::CurrencyData, language::data::LanguageData,
    measurement_unit::data::MeasurementUnitData, stripe_customer_id::StripeCustomerId,
};
use geo::data::address_data::StructuredAddressData;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchAdminUserData {
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prohibited_content_consent: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<UserTierData>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<UserRoleData>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stripe_customer_id: Option<StripeCustomerId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_address: Option<StructuredAddressData>,
}

#[cfg(feature = "test-data")]
mod fake {
    use super::PatchAdminUserData;
    use fake::Fake;

    impl fake::Dummy<fake::Faker> for PatchAdminUserData {
        fn dummy_with_rng<R: fake::rand::RngExt + ?Sized>(
            config: &fake::Faker,
            rng: &mut R,
        ) -> Self {
            PatchAdminUserData {
                first_name: config.fake_with_rng(rng),
                last_name: config.fake_with_rng(rng),
                language: config.fake_with_rng(rng),
                currency: config.fake_with_rng(rng),
                measurement_unit: config.fake_with_rng(rng),
                prohibited_content_consent: config.fake_with_rng(rng),
                tier: config.fake_with_rng(rng),
                role: config.fake_with_rng(rng),
                stripe_customer_id: config.fake_with_rng(rng),
                structured_address: config.fake_with_rng(rng),
            }
        }
    }
}
