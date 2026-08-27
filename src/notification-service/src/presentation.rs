use localization::Language;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotificationPresentationPreferences {
    pub language: Language,
    pub show_unassessed_or_sensitive_content: bool,
}
