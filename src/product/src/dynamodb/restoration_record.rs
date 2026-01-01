use crate::core::restoration::Restoration;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RestorationRecord {
    None,
    Minor,
    Major,

    #[default]
    Unknown,
}

impl From<RestorationRecord> for Restoration {
    fn from(record: RestorationRecord) -> Self {
        match record {
            RestorationRecord::None => Restoration::None,
            RestorationRecord::Minor => Restoration::Minor,
            RestorationRecord::Major => Restoration::Major,
            RestorationRecord::Unknown => Restoration::Unknown,
        }
    }
}
