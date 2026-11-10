use crate::patch_value::PatchValue;
use localization::Language;
use money::Currency;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use user_core::{
    measurement_unit::MeasurementUnit, role::UserRole, tier::UserTier, user_id::UserId,
};
use user_service::ports::UserDetailsView;

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OwnUserData {
    #[schemars(schema_with = "crate::wire::schema::user_id")]
    pub user_id: UserId,
    #[schemars(with = "String")]
    pub email: serde_email::Email,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub first_name: Option<user_core::first_name::FirstName>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub last_name: Option<user_core::last_name::LastName>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::language::option"
    )]
    #[schemars(schema_with = "crate::wire::language::option::schema")]
    pub language: Option<Language>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::currency::option"
    )]
    #[schemars(schema_with = "crate::wire::currency::option::schema")]
    pub currency: Option<Currency>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::measurement_unit::option"
    )]
    #[schemars(schema_with = "crate::wire::measurement_unit::option::schema")]
    pub measurement_unit: Option<MeasurementUnit>,
    #[schemars(with = "bool")]
    pub show_unassessed_or_sensitive_content: bool,
    pub marketing_email_consent: bool,
    #[serde(with = "crate::wire::user_tier")]
    #[schemars(schema_with = "crate::wire::user_tier::schema")]
    pub tier: UserTier,
    #[serde(with = "crate::wire::user_role")]
    #[schemars(schema_with = "crate::wire::user_role::schema")]
    pub role: UserRole,
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
            show_unassessed_or_sensitive_content: view.show_unassessed_or_sensitive_content,
            marketing_email_consent: view.marketing_email_consent,
            tier: view.tier,
            role: view.role,
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PatchOwnUserData {
    /// First name; omit to keep it, null clears it.
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub first_name: PatchValue<user_core::first_name::FirstName>,
    /// Last name; omit to keep it, null clears it.
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub last_name: PatchValue<user_core::last_name::LastName>,
    /// Preferred language, as an ISO 639-1 code; omit to keep it, null clears it.
    #[serde(
        default,
        deserialize_with = "crate::wire::language::patch::deserialize"
    )]
    #[schemars(schema_with = "crate::wire::language::option::schema")]
    pub language: PatchValue<Language>,
    /// Preferred currency, as an ISO 4217 code; omit to keep it, null clears it.
    #[serde(
        default,
        deserialize_with = "crate::wire::currency::patch::deserialize"
    )]
    #[schemars(schema_with = "crate::wire::currency::option::schema")]
    pub currency: PatchValue<Currency>,
    /// Preferred measurement unit system; omit to keep it, null clears it.
    #[serde(
        default,
        deserialize_with = "crate::wire::measurement_unit::patch::deserialize"
    )]
    #[schemars(schema_with = "crate::wire::measurement_unit::option::schema")]
    pub measurement_unit: PatchValue<MeasurementUnit>,
    /// Whether listings with unassessed or sensitive content are shown; omit to keep it, null is rejected.
    #[serde(default)]
    #[schemars(with = "bool")]
    pub show_unassessed_or_sensitive_content: PatchValue<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("Body field 'showUnassessedOrSensitiveContent' must not be null.")]
pub struct NullSensitiveContentPreference;

impl PatchOwnUserData {
    pub fn into_command(
        self,
        user_id: UserId,
    ) -> Result<
        user_service::use_cases::commands::update_user_profile::UpdateUserProfileCommand,
        NullSensitiveContentPreference,
    > {
        use crate::patch_value::clearable;
        use application::patch_field::PatchField;
        use user_service::use_cases::commands::update_user_profile::UpdateUserProfileCommand;
        Ok(UpdateUserProfileCommand {
            user_id,
            first_name: clearable(self.first_name),
            last_name: clearable(self.last_name),
            language: clearable(self.language),
            currency: clearable(self.currency),
            measurement_unit: clearable(self.measurement_unit),
            show_unassessed_or_sensitive_content: match self.show_unassessed_or_sensitive_content {
                PatchValue::Omitted => PatchField::Unchanged,
                PatchValue::Value(value) => PatchField::Set(value),
                PatchValue::Null => return Err(NullSensitiveContentPreference),
            },
        })
    }
}
