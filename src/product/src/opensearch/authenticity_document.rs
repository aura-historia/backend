use crate::{core::authenticity::Authenticity, dynamodb::authenticity_record::AuthenticityRecord};
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AuthenticityDocument {
    Original,
    LaterCopy,
    Reproduction,
    Questionable,

    #[default]
    Unknown,
}

impl From<AuthenticityRecord> for AuthenticityDocument {
    fn from(record: AuthenticityRecord) -> Self {
        match record {
            AuthenticityRecord::Original => AuthenticityDocument::Original,
            AuthenticityRecord::LaterCopy => AuthenticityDocument::LaterCopy,
            AuthenticityRecord::Reproduction => AuthenticityDocument::Reproduction,
            AuthenticityRecord::Questionable => AuthenticityDocument::Questionable,
            AuthenticityRecord::Unknown => AuthenticityDocument::Unknown,
        }
    }
}

impl From<AuthenticityDocument> for Authenticity {
    fn from(doc: AuthenticityDocument) -> Self {
        match doc {
            AuthenticityDocument::Original => Authenticity::Original,
            AuthenticityDocument::LaterCopy => Authenticity::LaterCopy,
            AuthenticityDocument::Reproduction => Authenticity::Reproduction,
            AuthenticityDocument::Questionable => Authenticity::Questionable,
            AuthenticityDocument::Unknown => Authenticity::Unknown,
        }
    }
}

