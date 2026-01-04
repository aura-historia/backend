use crate::core::authenticity::Authenticity;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AuthenticityRecord {
    Original,
    LaterCopy,
    Reproduction,
    Questionable,

    #[default]
    Unknown,
}

impl From<AuthenticityRecord> for Authenticity {
    fn from(record: AuthenticityRecord) -> Self {
        match record {
            AuthenticityRecord::Original => Authenticity::Original,
            AuthenticityRecord::LaterCopy => Authenticity::LaterCopy,
            AuthenticityRecord::Reproduction => Authenticity::Reproduction,
            AuthenticityRecord::Questionable => Authenticity::Questionable,
            AuthenticityRecord::Unknown => Authenticity::Unknown,
        }
    }
}

impl From<Authenticity> for AuthenticityRecord {
    fn from(value: Authenticity) -> Self {
        match value {
            Authenticity::Original => AuthenticityRecord::Original,
            Authenticity::LaterCopy => AuthenticityRecord::LaterCopy,
            Authenticity::Reproduction => AuthenticityRecord::Reproduction,
            Authenticity::Questionable => AuthenticityRecord::Questionable,
            Authenticity::Unknown => AuthenticityRecord::Unknown,
        }
    }
}
