use crate::core::{first_name::FirstName, last_name::LastName};
use common::{currency::data::CurrencyData, language::data::LanguageData, user_id::UserId};
use serde::{Deserialize, Serialize};
use serde_email::Email;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GetUserData {
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

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}
