use crate::core::prohibited_content::ProhibitedContent;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProhibitedContentData {
    #[default]
    Unknown,
    None,
    NaziGermany,
}

impl From<ProhibitedContent> for ProhibitedContentData {
    fn from(value: ProhibitedContent) -> Self {
        match value {
            ProhibitedContent::Unknown => ProhibitedContentData::Unknown,
            ProhibitedContent::None => ProhibitedContentData::None,
            ProhibitedContent::NaziGermany => ProhibitedContentData::NaziGermany,
        }
    }
}
