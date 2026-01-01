use crate::core::authenticity::Authenticity;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AuthenticityData {
    Original,
    LaterCopy,
    Reproduction,
    Questionable,

    #[default]
    Unknown,
}

impl From<Authenticity> for AuthenticityData {
    fn from(value: Authenticity) -> Self {
        match value {
            Authenticity::Original => AuthenticityData::Original,
            Authenticity::LaterCopy => AuthenticityData::LaterCopy,
            Authenticity::Reproduction => AuthenticityData::Reproduction,
            Authenticity::Questionable => AuthenticityData::Questionable,
            Authenticity::Unknown => AuthenticityData::Unknown,
        }
    }
}
