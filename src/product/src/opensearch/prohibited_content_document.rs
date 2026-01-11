use crate::{
    core::prohibited_content::ProhibitedContent,
    dynamodb::prohibited_content_record::ProhibitedContentRecord,
};
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProhibitedContentDocument {
    #[default]
    Unknown,
    None,
    NaziGermany,
}

impl From<ProhibitedContent> for ProhibitedContentDocument {
    fn from(value: ProhibitedContent) -> Self {
        match value {
            ProhibitedContent::Unknown => ProhibitedContentDocument::Unknown,
            ProhibitedContent::None => ProhibitedContentDocument::None,
            ProhibitedContent::NaziGermany => ProhibitedContentDocument::NaziGermany,
        }
    }
}

impl From<ProhibitedContentDocument> for ProhibitedContent {
    fn from(value: ProhibitedContentDocument) -> Self {
        match value {
            ProhibitedContentDocument::Unknown => ProhibitedContent::Unknown,
            ProhibitedContentDocument::None => ProhibitedContent::None,
            ProhibitedContentDocument::NaziGermany => ProhibitedContent::NaziGermany,
        }
    }
}

impl From<ProhibitedContentRecord> for ProhibitedContentDocument {
    fn from(value: ProhibitedContentRecord) -> Self {
        match value {
            ProhibitedContentRecord::Unknown => ProhibitedContentDocument::Unknown,
            ProhibitedContentRecord::None => ProhibitedContentDocument::None,
            ProhibitedContentRecord::NaziGermany => ProhibitedContentDocument::NaziGermany,
        }
    }
}
