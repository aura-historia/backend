use localization::Language;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MailTemplateType {
    WatchlistUpdatePrice,
    WatchlistUpdateState,
    SearchFilterMatch,
    PartnerApplicationApproval,
    PartnerApplicationRejection,
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MailTemplate {
    pub template_type: MailTemplateType,
    pub language: Language,
}

impl MailTemplateType {
    pub fn as_s3_dir_str(&self) -> &'static str {
        match self {
            MailTemplateType::WatchlistUpdatePrice => "mjml/watchlist/product-update/price",
            MailTemplateType::WatchlistUpdateState => "mjml/watchlist/product-update/state",
            MailTemplateType::SearchFilterMatch => "mjml/search-filter/match",
            MailTemplateType::PartnerApplicationApproval => "mjml/partner-application/approval",
            MailTemplateType::PartnerApplicationRejection => "mjml/partner-application/rejection",
        }
    }

    pub fn as_message_tag_value(&self) -> &'static str {
        match self {
            MailTemplateType::WatchlistUpdatePrice => "WATCHLIST_UPDATE_PRICE",
            MailTemplateType::WatchlistUpdateState => "WATCHLIST_UPDATE_STATE",
            MailTemplateType::SearchFilterMatch => "SEARCH_FILTER_MATCH",
            MailTemplateType::PartnerApplicationApproval => "PARTNER_APPLICATION_APPROVAL",
            MailTemplateType::PartnerApplicationRejection => "PARTNER_APPLICATION_REJECTION",
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
