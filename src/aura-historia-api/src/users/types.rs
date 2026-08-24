use crate::patch_value::PatchValue;
use geo::data::address_data::{GeoAddressData, StructuredAddressData};
use localization::Language;
use money::Currency;
use serde::{Deserialize, Serialize};
use user_core::measurement_unit::MeasurementUnit;
use user_core::role::UserRole;
use user_core::tier::UserTier;
use user_core::user_id::UserId;
use user_service::ports::UserDetailsView;
use user_service::use_cases::queries::search_users::UserSummary;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum UserTierData {
    Free,
    Pro,
    Ultimate,
}

impl From<UserTier> for UserTierData {
    fn from(tier: UserTier) -> Self {
        match tier {
            UserTier::Free => Self::Free,
            UserTier::Pro => Self::Pro,
            UserTier::Ultimate => Self::Ultimate,
        }
    }
}

impl From<UserTierData> for UserTier {
    fn from(tier: UserTierData) -> Self {
        match tier {
            UserTierData::Free => Self::Free,
            UserTierData::Pro => Self::Pro,
            UserTierData::Ultimate => Self::Ultimate,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum UserRoleData {
    User,
    Admin,
}

impl From<UserRole> for UserRoleData {
    fn from(role: UserRole) -> Self {
        match role {
            UserRole::User => Self::User,
            UserRole::Admin => Self::Admin,
        }
    }
}

impl From<UserRoleData> for UserRole {
    fn from(role: UserRoleData) -> Self {
        match role {
            UserRoleData::User => Self::User,
            UserRoleData::Admin => Self::Admin,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OwnUserData {
    pub(crate) user_id: UserId,
    pub(crate) email: serde_email::Email,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) first_name: Option<user_core::first_name::FirstName>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) last_name: Option<user_core::last_name::LastName>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::language::option"
    )]
    pub(crate) language: Option<Language>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::currency::option"
    )]
    pub(crate) currency: Option<Currency>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::measurement_unit::option"
    )]
    pub(crate) measurement_unit: Option<MeasurementUnit>,
    pub(crate) prohibited_content_consent: bool,
    pub(crate) tier: UserTierData,
    pub(crate) role: UserRoleData,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) stripe_customer_id: Option<user_core::stripe_customer_id::StripeCustomerId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) structured_address: Option<StructuredAddressData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) geo_address: Option<GeoAddressData>,
}

impl From<UserDetailsView> for OwnUserData {
    fn from(view: UserDetailsView) -> Self {
        Self {
            user_id: view.user_id,
            email: view.email,
            first_name: view.first_name,
            last_name: view.last_name,
            language: view.language,
            currency: view.currency,
            measurement_unit: view.measurement_unit,
            prohibited_content_consent: view.prohibited_content_consent,
            tier: view.tier.into(),
            role: view.role.into(),
            stripe_customer_id: view.stripe_customer_id,
            structured_address: view.structured_address.map(Into::into),
            geo_address: view.geo_address.map(Into::into),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AdminUserData {
    pub(crate) user_id: UserId,
    pub(crate) email: serde_email::Email,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) first_name: Option<user_core::first_name::FirstName>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) last_name: Option<user_core::last_name::LastName>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::language::option"
    )]
    pub(crate) language: Option<Language>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::currency::option"
    )]
    pub(crate) currency: Option<Currency>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::measurement_unit::option"
    )]
    pub(crate) measurement_unit: Option<MeasurementUnit>,
    pub(crate) prohibited_content_consent: bool,
    pub(crate) tier: UserTierData,
    pub(crate) role: UserRoleData,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) stripe_customer_id: Option<user_core::stripe_customer_id::StripeCustomerId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) structured_address: Option<StructuredAddressData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) geo_address: Option<GeoAddressData>,
}

impl From<UserDetailsView> for AdminUserData {
    fn from(view: UserDetailsView) -> Self {
        Self {
            user_id: view.user_id,
            email: view.email,
            first_name: view.first_name,
            last_name: view.last_name,
            language: view.language,
            currency: view.currency,
            measurement_unit: view.measurement_unit,
            prohibited_content_consent: view.prohibited_content_consent,
            tier: view.tier.into(),
            role: view.role.into(),
            stripe_customer_id: view.stripe_customer_id,
            structured_address: view.structured_address.map(Into::into),
            geo_address: view.geo_address.map(Into::into),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AdminUserSummaryData {
    pub(crate) user_id: UserId,
    pub(crate) email: serde_email::Email,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) first_name: Option<user_core::first_name::FirstName>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) last_name: Option<user_core::last_name::LastName>,
    pub(crate) tier: UserTierData,
    pub(crate) role: UserRoleData,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) stripe_customer_id: Option<user_core::stripe_customer_id::StripeCustomerId>,
}
impl From<UserSummary> for AdminUserSummaryData {
    fn from(v: UserSummary) -> Self {
        Self {
            user_id: v.user_id,
            email: v.email,
            first_name: v.first_name,
            last_name: v.last_name,
            tier: v.tier.into(),
            role: v.role.into(),
            stripe_customer_id: v.stripe_customer_id,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CursorData<T, C> {
    pub(crate) items: Vec<T>,
    pub(crate) size: u64,
    pub(crate) search_after: Option<C>,
    pub(crate) total: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PatchOwnUserData {
    #[serde(default)]
    pub(crate) email: PatchValue<serde_email::Email>,
    #[serde(default)]
    pub(crate) first_name: PatchValue<user_core::first_name::FirstName>,
    #[serde(default)]
    pub(crate) last_name: PatchValue<user_core::last_name::LastName>,
    #[serde(
        default,
        deserialize_with = "crate::wire::language::patch::deserialize"
    )]
    pub(crate) language: PatchValue<Language>,
    #[serde(
        default,
        deserialize_with = "crate::wire::currency::patch::deserialize"
    )]
    pub(crate) currency: PatchValue<Currency>,
    #[serde(
        default,
        deserialize_with = "crate::wire::measurement_unit::patch::deserialize"
    )]
    pub(crate) measurement_unit: PatchValue<MeasurementUnit>,
    #[serde(default)]
    pub(crate) prohibited_content_consent: PatchValue<bool>,
    #[serde(default)]
    pub(crate) structured_address: PatchValue<StructuredAddressData>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PatchAdminUserData {
    #[serde(default)]
    pub(crate) email: PatchValue<serde_email::Email>,
    #[serde(default)]
    pub(crate) first_name: PatchValue<user_core::first_name::FirstName>,
    #[serde(default)]
    pub(crate) last_name: PatchValue<user_core::last_name::LastName>,
    #[serde(
        default,
        deserialize_with = "crate::wire::language::patch::deserialize"
    )]
    pub(crate) language: PatchValue<Language>,
    #[serde(
        default,
        deserialize_with = "crate::wire::currency::patch::deserialize"
    )]
    pub(crate) currency: PatchValue<Currency>,
    #[serde(
        default,
        deserialize_with = "crate::wire::measurement_unit::patch::deserialize"
    )]
    pub(crate) measurement_unit: PatchValue<MeasurementUnit>,
    #[serde(default)]
    pub(crate) prohibited_content_consent: PatchValue<bool>,
    #[serde(default)]
    pub(crate) tier: PatchValue<UserTierData>,
    #[serde(default)]
    pub(crate) role: PatchValue<UserRoleData>,
    #[serde(default)]
    pub(crate) structured_address: PatchValue<StructuredAddressData>,
}
