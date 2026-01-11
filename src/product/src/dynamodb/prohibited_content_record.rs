use crate::core::prohibited_content::ProhibitedContent;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProhibitedContentRecord {
    #[default]
    Unknown,
    None,
    NaziGermany,
}

impl From<ProhibitedContent> for ProhibitedContentRecord {
    fn from(value: ProhibitedContent) -> Self {
        match value {
            ProhibitedContent::Unknown => ProhibitedContentRecord::Unknown,
            ProhibitedContent::None => ProhibitedContentRecord::None,
            ProhibitedContent::NaziGermany => ProhibitedContentRecord::NaziGermany,
        }
    }
}
impl From<ProhibitedContentRecord> for ProhibitedContent {
    fn from(value: ProhibitedContentRecord) -> Self {
        match value {
            ProhibitedContentRecord::Unknown => ProhibitedContent::Unknown,
            ProhibitedContentRecord::None => ProhibitedContent::None,
            ProhibitedContentRecord::NaziGermany => ProhibitedContent::NaziGermany,
        }
    }
}
