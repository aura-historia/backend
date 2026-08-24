use isocountry::CountryCode;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum_macros::EnumIter)]
pub enum Continent {
    Africa,
    Antarctica,
    Asia,
    Europe,
    NorthAmerica,
    Oceania,
    SouthAmerica,
}

impl Continent {
    pub fn from_code(value: &str) -> Option<Self> {
        use strum::IntoEnumIterator;

        Self::iter().find(|continent| continent.as_str() == value)
    }

    pub fn as_str(self) -> &'static str {
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

impl From<CountryCode> for Continent {
    #[allow(clippy::too_many_lines)]
    fn from(code: CountryCode) -> Self {
        match code {
            // Africa
            CountryCode::AGO
            | CountryCode::BEN
            | CountryCode::BWA
            | CountryCode::BFA
            | CountryCode::BDI
            | CountryCode::CPV
            | CountryCode::CMR
            | CountryCode::CAF
            | CountryCode::TCD
            | CountryCode::COM
            | CountryCode::COD
            | CountryCode::COG
            | CountryCode::DJI
            | CountryCode::EGY
            | CountryCode::GNQ
            | CountryCode::ERI
            | CountryCode::SWZ
            | CountryCode::ETH
            | CountryCode::GAB
            | CountryCode::GMB
            | CountryCode::GHA
            | CountryCode::GIN
            | CountryCode::GNB
            | CountryCode::CIV
            | CountryCode::KEN
            | CountryCode::LSO
            | CountryCode::LBR
            | CountryCode::LBY
            | CountryCode::MDG
            | CountryCode::MWI
            | CountryCode::MLI
            | CountryCode::MRT
            | CountryCode::MUS
            | CountryCode::MAR
            | CountryCode::MOZ
            | CountryCode::NAM
            | CountryCode::NER
            | CountryCode::NGA
            | CountryCode::RWA
            | CountryCode::STP
            | CountryCode::SEN
            | CountryCode::SYC
            | CountryCode::SLE
            | CountryCode::SOM
            | CountryCode::ZAF
            | CountryCode::SSD
            | CountryCode::SDN
            | CountryCode::TZA
            | CountryCode::TGO
            | CountryCode::TUN
            | CountryCode::UGA
            | CountryCode::ZMB
            | CountryCode::ZWE
            | CountryCode::DZA
            | CountryCode::REU
            | CountryCode::MYT
            | CountryCode::SHN
            | CountryCode::ESH
            | CountryCode::ATF => Continent::Africa,

            // Antarctica
            CountryCode::ATA | CountryCode::BVT | CountryCode::HMD | CountryCode::SGS => {
                Continent::Antarctica
            }

            // Asia
            CountryCode::AFG
            | CountryCode::ARM
            | CountryCode::AZE
            | CountryCode::BHR
            | CountryCode::BGD
            | CountryCode::BTN
            | CountryCode::BRN
            | CountryCode::KHM
            | CountryCode::CHN
            | CountryCode::CXR
            | CountryCode::CCK
            | CountryCode::GEO
            | CountryCode::HKG
            | CountryCode::IND
            | CountryCode::IDN
            | CountryCode::IRN
            | CountryCode::IRQ
            | CountryCode::ISR
            | CountryCode::JPN
            | CountryCode::JOR
            | CountryCode::KAZ
            | CountryCode::KWT
            | CountryCode::KGZ
            | CountryCode::LAO
            | CountryCode::LBN
            | CountryCode::MAC
            | CountryCode::MYS
            | CountryCode::MDV
            | CountryCode::MNG
            | CountryCode::MMR
            | CountryCode::NPL
            | CountryCode::PRK
            | CountryCode::OMN
            | CountryCode::PAK
            | CountryCode::PSE
            | CountryCode::PHL
            | CountryCode::QAT
            | CountryCode::SAU
            | CountryCode::SGP
            | CountryCode::LKA
            | CountryCode::SYR
            | CountryCode::TWN
            | CountryCode::TJK
            | CountryCode::THA
            | CountryCode::TLS
            | CountryCode::TKM
            | CountryCode::ARE
            | CountryCode::UZB
            | CountryCode::VNM
            | CountryCode::YEM
            | CountryCode::IOT
            | CountryCode::KOR => Continent::Asia,

            // Europe
            CountryCode::ALB
            | CountryCode::AND
            | CountryCode::AUT
            | CountryCode::BLR
            | CountryCode::BEL
            | CountryCode::BIH
            | CountryCode::BGR
            | CountryCode::HRV
            | CountryCode::CYP
            | CountryCode::CZE
            | CountryCode::DNK
            | CountryCode::EST
            | CountryCode::FIN
            | CountryCode::FRA
            | CountryCode::DEU
            | CountryCode::GRC
            | CountryCode::HUN
            | CountryCode::ISL
            | CountryCode::IRL
            | CountryCode::ITA
            | CountryCode::LVA
            | CountryCode::LIE
            | CountryCode::LTU
            | CountryCode::LUX
            | CountryCode::MLT
            | CountryCode::MDA
            | CountryCode::MCO
            | CountryCode::MNE
            | CountryCode::NLD
            | CountryCode::MKD
            | CountryCode::NOR
            | CountryCode::POL
            | CountryCode::PRT
            | CountryCode::ROU
            | CountryCode::RUS
            | CountryCode::SMR
            | CountryCode::SRB
            | CountryCode::SVK
            | CountryCode::SVN
            | CountryCode::ESP
            | CountryCode::SWE
            | CountryCode::CHE
            | CountryCode::UKR
            | CountryCode::GBR
            | CountryCode::VAT
            | CountryCode::ALA
            | CountryCode::FRO
            | CountryCode::GIB
            | CountryCode::GGY
            | CountryCode::IMN
            | CountryCode::JEY
            | CountryCode::SJM
            | CountryCode::TUR => Continent::Europe,

            // North America
            CountryCode::ATG
            | CountryCode::BHS
            | CountryCode::BLZ
            | CountryCode::CAN
            | CountryCode::CRI
            | CountryCode::CUB
            | CountryCode::DMA
            | CountryCode::DOM
            | CountryCode::SLV
            | CountryCode::GRD
            | CountryCode::GTM
            | CountryCode::HTI
            | CountryCode::HND
            | CountryCode::JAM
            | CountryCode::MEX
            | CountryCode::NIC
            | CountryCode::PAN
            | CountryCode::KNA
            | CountryCode::LCA
            | CountryCode::VCT
            | CountryCode::TTO
            | CountryCode::USA
            | CountryCode::ABW
            | CountryCode::AIA
            | CountryCode::BMU
            | CountryCode::CYM
            | CountryCode::GLP
            | CountryCode::MTQ
            | CountryCode::MSR
            | CountryCode::PRI
            | CountryCode::SXM
            | CountryCode::TCA
            | CountryCode::VIR
            | CountryCode::CUW
            | CountryCode::BLM
            | CountryCode::MAF
            | CountryCode::SPM
            | CountryCode::BES
            | CountryCode::VGB => Continent::NorthAmerica,

            // Oceania
            CountryCode::AUS
            | CountryCode::FJI
            | CountryCode::KIR
            | CountryCode::MHL
            | CountryCode::FSM
            | CountryCode::NRU
            | CountryCode::NZL
            | CountryCode::PLW
            | CountryCode::PNG
            | CountryCode::WSM
            | CountryCode::SLB
            | CountryCode::TON
            | CountryCode::TUV
            | CountryCode::VUT
            | CountryCode::ASM
            | CountryCode::COK
            | CountryCode::PYF
            | CountryCode::GUM
            | CountryCode::MNP
            | CountryCode::NCL
            | CountryCode::NFK
            | CountryCode::NIU
            | CountryCode::PCN
            | CountryCode::TKL
            | CountryCode::WLF => Continent::Oceania,

            // South America
            CountryCode::ARG
            | CountryCode::BOL
            | CountryCode::BRA
            | CountryCode::CHL
            | CountryCode::COL
            | CountryCode::ECU
            | CountryCode::GUY
            | CountryCode::PRY
            | CountryCode::PER
            | CountryCode::SUR
            | CountryCode::URY
            | CountryCode::VEN
            | CountryCode::FLK
            | CountryCode::GUF => Continent::SouthAmerica,

            // North America (remaining)
            CountryCode::BRB | CountryCode::GRL => Continent::NorthAmerica,

            // Oceania (remaining)
            CountryCode::UMI => Continent::Oceania,
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::Continent;
    use fake::{Dummy, Faker, RngExt};

    impl Dummy<Faker> for Continent {
        fn dummy_with_rng<R: RngExt + ?Sized>(_config: &Faker, rng: &mut R) -> Self {
            let variants = [
                Continent::Africa,
                Continent::Antarctica,
                Continent::Asia,
                Continent::Europe,
                Continent::NorthAmerica,
                Continent::Oceania,
                Continent::SouthAmerica,
            ];
            variants[rng.random_range(0..variants.len())]
        }
    }
}

#[cfg(test)]
mod tests {
    use isocountry::CountryCode;

    use super::Continent;

    #[test]
    fn should_map_all_country_codes_to_a_continent() {
        for code in CountryCode::iter().copied() {
            let _ = Continent::from(code);
        }
    }

    #[test]
    fn should_round_trip_all_canonical_continent_codes() {
        use std::collections::HashSet;
        use strum::IntoEnumIterator;

        let identifiers = Continent::iter()
            .map(Continent::as_str)
            .collect::<HashSet<_>>();

        assert_eq!(Continent::iter().count(), identifiers.len());
        for continent in Continent::iter() {
            assert_eq!(Some(continent), Continent::from_code(continent.as_str()));
        }
        assert_eq!(None, Continent::from_code("europe"));
    }
}
