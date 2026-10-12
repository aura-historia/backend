//! Immutable, local reference data. Persist the release alongside geographic assertions.
#[path = "reference_codes.rs"]
mod codes;

use codes::{COUNTRIES, SUBDIVISIONS};

/// Historical reference evidence, never converted to a current country automatically.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormerCountry {
    pub alpha4: &'static str,
    pub former_alpha2: &'static str,
    pub name: &'static str,
    pub withdrawal_date: &'static str,
}

/// A release is part of the interpretation of stored codes, never a moving alias.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReferenceRelease {
    IsoCodes4_20_1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("unsupported geographic reference release")]
pub struct UnsupportedReferenceRelease;

impl ReferenceRelease {
    pub const CURRENT: Self = Self::IsoCodes4_20_1;

    pub fn as_str(self) -> &'static str {
        match self {
            Self::IsoCodes4_20_1 => "iso-codes-4.20.1",
        }
    }

    pub fn from_code(value: &str) -> Result<Self, UnsupportedReferenceRelease> {
        match value {
            "iso-codes-4.20.1" => Ok(Self::IsoCodes4_20_1),
            _ => Err(UnsupportedReferenceRelease),
        }
    }

    pub fn country_codes(self) -> &'static [&'static str] {
        match self {
            Self::IsoCodes4_20_1 => COUNTRIES,
        }
    }

    pub fn subdivision_codes(self) -> &'static [&'static str] {
        match self {
            Self::IsoCodes4_20_1 => SUBDIVISIONS,
        }
    }

    /// ISO 3166-3 alpha-4 disambiguates withdrawn and subsequently reused alpha-2 codes.
    pub fn former_country(self, alpha4: &str) -> Option<&'static FormerCountry> {
        match self {
            Self::IsoCodes4_20_1 => codes::FORMER_COUNTRIES
                .binary_search_by_key(&alpha4, |row| row.alpha4)
                .ok()
                .map(|index| &codes::FORMER_COUNTRIES[index]),
        }
    }
}
