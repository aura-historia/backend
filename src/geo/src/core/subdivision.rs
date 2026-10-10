use super::country::{CountryCode, country_from_code};
use crate::reference::ReferenceRelease;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SubdivisionCode {
    code: &'static str,
    country: CountryCode,
    release: ReferenceRelease,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("unrecognized canonical ISO 3166-2 subdivision code")]
pub struct InvalidSubdivisionCode;

impl SubdivisionCode {
    pub fn new(code: &str) -> Result<Self, InvalidSubdivisionCode> {
        Self::from_code(code, ReferenceRelease::CURRENT)
    }

    pub fn from_code(
        code: &str,
        release: ReferenceRelease,
    ) -> Result<Self, InvalidSubdivisionCode> {
        let codes = release.subdivision_codes();
        let index = codes
            .binary_search(&code)
            .map_err(|_| InvalidSubdivisionCode)?;
        let code = codes[index];
        let country = country_from_code(&code[..2], release).map_err(|_| InvalidSubdivisionCode)?;
        Ok(Self {
            code,
            country,
            release,
        })
    }

    pub fn as_str(self) -> &'static str {
        self.code
    }
    pub fn country(self) -> CountryCode {
        self.country
    }
    pub fn reference_release(self) -> ReferenceRelease {
        self.release
    }
}
