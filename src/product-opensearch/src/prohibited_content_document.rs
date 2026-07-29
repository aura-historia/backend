use product_core::prohibited_content::ProhibitedContent;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum ProhibitedContentDocument {
    #[default]
    Unknown,
    None,
    NaziGermany,
}

impl From<ProhibitedContent> for ProhibitedContentDocument {
    fn from(value: ProhibitedContent) -> Self {
        match value {
            ProhibitedContent::Unknown => Self::Unknown,
            ProhibitedContent::None => Self::None,
            ProhibitedContent::NaziGermany => Self::NaziGermany,
        }
    }
}

impl From<ProhibitedContentDocument> for ProhibitedContent {
    fn from(value: ProhibitedContentDocument) -> Self {
        match value {
            ProhibitedContentDocument::Unknown => Self::Unknown,
            ProhibitedContentDocument::None => Self::None,
            ProhibitedContentDocument::NaziGermany => Self::NaziGermany,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[rstest::rstest]
    #[case(ProhibitedContentDocument::Unknown, "\"UNKNOWN\"")]
    #[case(ProhibitedContentDocument::None, "\"NONE\"")]
    #[case(ProhibitedContentDocument::NaziGermany, "\"NAZI_GERMANY\"")]
    fn should_serialize_prohibited_content_document_in_screaming_snake_case(
        #[case] document: ProhibitedContentDocument,
        #[case] expected: &'static str,
    ) -> Result<(), serde_json::Error> {
        assert_eq!(expected, serde_json::to_string(&document)?);
        Ok(())
    }

    #[rstest::rstest]
    #[case(ProhibitedContent::Unknown, ProhibitedContentDocument::Unknown)]
    #[case(ProhibitedContent::None, ProhibitedContentDocument::None)]
    #[case(ProhibitedContent::NaziGermany, ProhibitedContentDocument::NaziGermany)]
    fn should_roundtrip_prohibited_content(
        #[case] domain: ProhibitedContent,
        #[case] document: ProhibitedContentDocument,
    ) {
        assert_eq!(document, ProhibitedContentDocument::from(domain));
        assert_eq!(domain, ProhibitedContent::from(document));
    }
}
