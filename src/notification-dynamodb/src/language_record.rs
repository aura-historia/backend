use localization::Language;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LanguageRecord {
    De,
    En,
    Fr,
    Es,
    It,
}

impl From<LanguageRecord> for Language {
    fn from(value: LanguageRecord) -> Self {
        match value {
            LanguageRecord::De => Self::De,
            LanguageRecord::En => Self::En,
            LanguageRecord::Fr => Self::Fr,
            LanguageRecord::Es => Self::Es,
            LanguageRecord::It => Self::It,
        }
    }
}
