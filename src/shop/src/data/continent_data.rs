use crate::core::continent::Continent;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ContinentData {
    Africa,
    Antarctica,
    Asia,
    Europe,
    NorthAmerica,
    Oceania,
    SouthAmerica,
}

impl From<Continent> for ContinentData {
    fn from(continent: Continent) -> Self {
        match continent {
            Continent::Africa => ContinentData::Africa,
            Continent::Antarctica => ContinentData::Antarctica,
            Continent::Asia => ContinentData::Asia,
            Continent::Europe => ContinentData::Europe,
            Continent::NorthAmerica => ContinentData::NorthAmerica,
            Continent::Oceania => ContinentData::Oceania,
            Continent::SouthAmerica => ContinentData::SouthAmerica,
        }
    }
}

impl From<ContinentData> for Continent {
    fn from(data: ContinentData) -> Self {
        match data {
            ContinentData::Africa => Continent::Africa,
            ContinentData::Antarctica => Continent::Antarctica,
            ContinentData::Asia => Continent::Asia,
            ContinentData::Europe => Continent::Europe,
            ContinentData::NorthAmerica => Continent::NorthAmerica,
            ContinentData::Oceania => Continent::Oceania,
            ContinentData::SouthAmerica => Continent::SouthAmerica,
        }
    }
}
