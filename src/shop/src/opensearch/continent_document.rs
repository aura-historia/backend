use crate::core::continent::Continent;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ContinentDocument {
    Africa,
    Antarctica,
    Asia,
    Europe,
    NorthAmerica,
    Oceania,
    SouthAmerica,
}

impl ContinentDocument {
    pub fn as_str(&self) -> &'static str {
        match self {
            ContinentDocument::Africa => "AFRICA",
            ContinentDocument::Antarctica => "ANTARCTICA",
            ContinentDocument::Asia => "ASIA",
            ContinentDocument::Europe => "EUROPE",
            ContinentDocument::NorthAmerica => "NORTH_AMERICA",
            ContinentDocument::Oceania => "OCEANIA",
            ContinentDocument::SouthAmerica => "SOUTH_AMERICA",
        }
    }
}

impl From<Continent> for ContinentDocument {
    fn from(continent: Continent) -> Self {
        match continent {
            Continent::Africa => ContinentDocument::Africa,
            Continent::Antarctica => ContinentDocument::Antarctica,
            Continent::Asia => ContinentDocument::Asia,
            Continent::Europe => ContinentDocument::Europe,
            Continent::NorthAmerica => ContinentDocument::NorthAmerica,
            Continent::Oceania => ContinentDocument::Oceania,
            Continent::SouthAmerica => ContinentDocument::SouthAmerica,
        }
    }
}

impl From<ContinentDocument> for Continent {
    fn from(document: ContinentDocument) -> Self {
        match document {
            ContinentDocument::Africa => Continent::Africa,
            ContinentDocument::Antarctica => Continent::Antarctica,
            ContinentDocument::Asia => Continent::Asia,
            ContinentDocument::Europe => Continent::Europe,
            ContinentDocument::NorthAmerica => Continent::NorthAmerica,
            ContinentDocument::Oceania => Continent::Oceania,
            ContinentDocument::SouthAmerica => Continent::SouthAmerica,
        }
    }
}
