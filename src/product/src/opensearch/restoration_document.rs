use crate::{core::restoration::Restoration, dynamodb::restoration_record::RestorationRecord};
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RestorationDocument {
    None,
    Minor,
    Major,

    #[default]
    Unknown,
}

impl From<RestorationRecord> for RestorationDocument {
    fn from(record: RestorationRecord) -> Self {
        match record {
            RestorationRecord::None => RestorationDocument::None,
            RestorationRecord::Minor => RestorationDocument::Minor,
            RestorationRecord::Major => RestorationDocument::Major,
            RestorationRecord::Unknown => RestorationDocument::Unknown,
        }
    }
}

impl From<RestorationDocument> for Restoration {
    fn from(doc: RestorationDocument) -> Self {
        match doc {
            RestorationDocument::None => Restoration::None,
            RestorationDocument::Minor => Restoration::Minor,
            RestorationDocument::Major => Restoration::Major,
            RestorationDocument::Unknown => Restoration::Unknown,
        }
    }
}
