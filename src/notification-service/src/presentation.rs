use common::{currency::domain::Currency, language::domain::Language};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotificationPresentationPreferences {
    pub language: Language,
    pub currency: Currency,
    pub prohibited_content_consent: bool,
}
