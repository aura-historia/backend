use crate::core::restoration::Restoration;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RestorationData {
    None,
    Minor,
    Major,

    #[default]
    Unknown,
}

impl From<Restoration> for RestorationData {
    fn from(value: Restoration) -> Self {
        match value {
            Restoration::None => RestorationData::None,
            Restoration::Minor => RestorationData::Minor,
            Restoration::Major => RestorationData::Major,
            Restoration::Unknown => RestorationData::Unknown,
        }
    }
}

impl From<RestorationData> for Restoration {
    fn from(value: RestorationData) -> Self {
        match value {
            RestorationData::None => Restoration::None,
            RestorationData::Minor => Restoration::Minor,
            RestorationData::Major => Restoration::Major,
            RestorationData::Unknown => Restoration::Unknown,
        }
    }
}
