use crate::core::{first_name::FirstName, last_name::LastName};
use common::{currency::data::CurrencyData, language::data::LanguageData};
use serde::{Deserialize, Serialize};
use serde_email::Email;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PatchUserData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<Email>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_name: Option<FirstName>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_name: Option<LastName>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<LanguageData>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<CurrencyData>,
}
