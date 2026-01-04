use crate::{core::authenticity::Authenticity, dynamodb::authenticity_record::AuthenticityRecord};
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(
    Copy,
    Clone,
    Eq,
    PartialEq,
    Debug,
    Hash,
    Default,
    Serialize,
    Deserialize,
    strum_macros::EnumCount,
)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AuthenticityDocument {
    Original,
    LaterCopy,
    Reproduction,
    Questionable,

    #[default]
    Unknown,
}

impl AuthenticityDocument {
    pub fn as_str(&self) -> &'static str {
        match self {
            AuthenticityDocument::Original => "ORIGINAL",
            AuthenticityDocument::LaterCopy => "LATER_COPY",
            AuthenticityDocument::Reproduction => "REPRODUCTION",
            AuthenticityDocument::Questionable => "QUESTIONABLE",
            AuthenticityDocument::Unknown => "UNKNOWN",
        }
    }
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

impl From<Authenticity> for AuthenticityDocument {
    fn from(value: Authenticity) -> Self {
        match value {
            Authenticity::Original => AuthenticityDocument::Original,
            Authenticity::LaterCopy => AuthenticityDocument::LaterCopy,
            Authenticity::Reproduction => AuthenticityDocument::Reproduction,
            Authenticity::Questionable => AuthenticityDocument::Questionable,
            Authenticity::Unknown => AuthenticityDocument::Unknown,
        }
    }
}
