use common::currency::data::CurrencyData;
use common::language::data::LanguageData;
use common::measurement_unit::data::MeasurementUnitData;
use common::user_id::UserId;
use geo::data::address_data::{GeoAddressData, StructuredAddressData};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use user_service::use_cases::queries::get_user::UserDetailsView;
use user_service::use_cases::queries::search_users::UserSummary;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UserData {
    pub(crate) user_id: UserId,
    pub(crate) email: serde_email::Email,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) first_name: Option<user_core::first_name::FirstName>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) last_name: Option<user_core::last_name::LastName>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) language: Option<LanguageData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) currency: Option<CurrencyData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) measurement_unit: Option<MeasurementUnitData>,
    pub(crate) prohibited_content_consent: bool,
    pub(crate) tier: String,
    pub(crate) role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) stripe_customer_id: Option<common::stripe_customer_id::StripeCustomerId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) structured_address: Option<StructuredAddressData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) geo_address: Option<GeoAddressData>,
}

impl From<UserDetailsView> for UserData {
    fn from(view: UserDetailsView) -> Self {
        Self {
            user_id: view.user_id,
            email: view.email,
            first_name: view.first_name,
            last_name: view.last_name,
            language: view.language.map(Into::into),
            currency: view.currency.map(Into::into),
            measurement_unit: view.measurement_unit.map(Into::into),
            prohibited_content_consent: view.prohibited_content_consent,
            tier: format!("{:?}", view.tier),
            role: format!("{:?}", view.role),
            stripe_customer_id: view.stripe_customer_id,
            structured_address: view.structured_address.map(Into::into),
            geo_address: view.geo_address.map(Into::into),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UserSummaryData {
    pub(crate) user_id: UserId,
    pub(crate) email: serde_email::Email,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) first_name: Option<user_core::first_name::FirstName>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) last_name: Option<user_core::last_name::LastName>,
    pub(crate) tier: String,
    pub(crate) role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) stripe_customer_id: Option<common::stripe_customer_id::StripeCustomerId>,
}
impl From<UserSummary> for UserSummaryData {
    fn from(v: UserSummary) -> Self {
        Self {
            user_id: v.user_id,
            email: v.email,
            first_name: v.first_name,
            last_name: v.last_name,
            tier: format!("{:?}", v.tier),
            role: format!("{:?}", v.role),
            stripe_customer_id: v.stripe_customer_id,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CursorData<T> {
    pub(crate) items: Vec<T>,
    pub(crate) size: u64,
    pub(crate) search_after: Option<Value>,
    pub(crate) total: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PatchUserData {
    pub(crate) email: Option<serde_email::Email>,
    pub(crate) first_name: Option<user_core::first_name::FirstName>,
    pub(crate) last_name: Option<user_core::last_name::LastName>,
    pub(crate) language: Option<LanguageData>,
    pub(crate) currency: Option<CurrencyData>,
    pub(crate) measurement_unit: Option<MeasurementUnitData>,
    pub(crate) prohibited_content_consent: Option<bool>,
    pub(crate) tier: Option<String>,
    pub(crate) role: Option<String>,
    pub(crate) structured_address: Option<StructuredAddressData>,
}
