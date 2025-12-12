use common::language::data::LanguageData;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MailTemplateType {
    WatchlistUpdate,
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MailTemplate {
    pub template_type: MailTemplateType,
    pub language: LanguageData,
}

impl MailTemplateType {
    pub fn as_s3_dir_str(&self) -> &'static str {
        match self {
            MailTemplateType::WatchlistUpdate => "mjml/watchlist/product-update",
        }
    }
}

impl MailTemplate {
    pub fn as_s3_blob_str(&self) -> String {
        format!(
            "{}/{}",
            self.template_type.as_s3_dir_str(),
            self.language.as_str()
        )
    }
}
