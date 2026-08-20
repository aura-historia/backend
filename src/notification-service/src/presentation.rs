use common::language::domain::Language;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotificationPresentationPreferences {
    pub language: Language,
    pub prohibited_content_consent: bool,
}
