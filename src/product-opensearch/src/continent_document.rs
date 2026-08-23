use geo::core::continent::Continent;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum ContinentDocument {
    Africa,
    Antarctica,
    Asia,
    Europe,
    NorthAmerica,
    Oceania,
    SouthAmerica,
}

impl ContinentDocument {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Africa => "AFRICA",
            Self::Antarctica => "ANTARCTICA",
            Self::Asia => "ASIA",
            Self::Europe => "EUROPE",
            Self::NorthAmerica => "NORTH_AMERICA",
            Self::Oceania => "OCEANIA",
            Self::SouthAmerica => "SOUTH_AMERICA",
        }
    }
}

impl From<Continent> for ContinentDocument {
    fn from(continent: Continent) -> Self {
        match continent {
            Continent::Africa => Self::Africa,
            Continent::Antarctica => Self::Antarctica,
            Continent::Asia => Self::Asia,
            Continent::Europe => Self::Europe,
            Continent::NorthAmerica => Self::NorthAmerica,
            Continent::Oceania => Self::Oceania,
            Continent::SouthAmerica => Self::SouthAmerica,
        }
    }
}

impl From<ContinentDocument> for Continent {
    fn from(document: ContinentDocument) -> Self {
        match document {
            ContinentDocument::Africa => Self::Africa,
            ContinentDocument::Antarctica => Self::Antarctica,
            ContinentDocument::Asia => Self::Asia,
            ContinentDocument::Europe => Self::Europe,
            ContinentDocument::NorthAmerica => Self::NorthAmerica,
            ContinentDocument::Oceania => Self::Oceania,
            ContinentDocument::SouthAmerica => Self::SouthAmerica,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[rstest::rstest]
    #[case(ContinentDocument::Africa, "\"AFRICA\"")]
    #[case(ContinentDocument::Antarctica, "\"ANTARCTICA\"")]
    #[case(ContinentDocument::Asia, "\"ASIA\"")]
    #[case(ContinentDocument::Europe, "\"EUROPE\"")]
    #[case(ContinentDocument::NorthAmerica, "\"NORTH_AMERICA\"")]
    #[case(ContinentDocument::Oceania, "\"OCEANIA\"")]
    #[case(ContinentDocument::SouthAmerica, "\"SOUTH_AMERICA\"")]
    fn should_serialize_continent_document_in_screaming_snake_case(
        #[case] continent: ContinentDocument,
        #[case] expected: &'static str,
    ) -> Result<(), serde_json::Error> {
        assert_eq!(expected, serde_json::to_string(&continent)?);
        assert_eq!(expected.replace('"', ""), continent.as_str());
        Ok(())
    }

    #[rstest::rstest]
    #[case(Continent::Africa, ContinentDocument::Africa)]
    #[case(Continent::Antarctica, ContinentDocument::Antarctica)]
    #[case(Continent::Asia, ContinentDocument::Asia)]
    #[case(Continent::Europe, ContinentDocument::Europe)]
    #[case(Continent::NorthAmerica, ContinentDocument::NorthAmerica)]
    #[case(Continent::Oceania, ContinentDocument::Oceania)]
    #[case(Continent::SouthAmerica, ContinentDocument::SouthAmerica)]
    fn should_roundtrip_continent(#[case] domain: Continent, #[case] document: ContinentDocument) {
        assert_eq!(document, ContinentDocument::from(domain));
        assert_eq!(domain, Continent::from(document));
    }
}
