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

#[cfg(test)]
mod tests {
    use super::AuthenticityDocument;
    use rstest::rstest;

    #[rstest]
    #[trace]
    #[case(AuthenticityDocument::Original, "\"ORIGINAL\"")]
    #[case(AuthenticityDocument::LaterCopy, "\"LATER_COPY\"")]
    #[case(AuthenticityDocument::Reproduction, "\"REPRODUCTION\"")]
    #[case(AuthenticityDocument::Questionable, "\"QUESTIONABLE\"")]
    #[case(AuthenticityDocument::Unknown, "\"UNKNOWN\"")]
    fn should_serialize_authenticity_document_in_screaming_snake_case(
        #[case] authenticity: AuthenticityDocument,
        #[case] expected: &str,
    ) {
        let actual = serde_json::to_string(&authenticity).unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[trace]
    #[case("\"ORIGINAL\"", AuthenticityDocument::Original)]
    #[case("\"LATER_COPY\"", AuthenticityDocument::LaterCopy)]
    #[case("\"REPRODUCTION\"", AuthenticityDocument::Reproduction)]
    #[case("\"QUESTIONABLE\"", AuthenticityDocument::Questionable)]
    #[case("\"UNKNOWN\"", AuthenticityDocument::Unknown)]
    fn should_deserialize_authenticity_document_in_screaming_snake_case(
        #[case] authenticity: &str,
        #[case] expected: AuthenticityDocument,
    ) {
        let actual = serde_json::from_str::<AuthenticityDocument>(authenticity).unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[trace]
    #[case(AuthenticityDocument::Original)]
    #[case(AuthenticityDocument::LaterCopy)]
    #[case(AuthenticityDocument::Reproduction)]
    #[case(AuthenticityDocument::Questionable)]
    #[case(AuthenticityDocument::Unknown)]
    fn should_as_str_match_serialized(#[case] authenticity: AuthenticityDocument) {
        let serialized = serde_json::to_string::<AuthenticityDocument>(&authenticity)
            .unwrap()
            .replace("\"", "");
        let as_str = authenticity.as_str();
        assert_eq!(serialized, as_str);
    }
}
