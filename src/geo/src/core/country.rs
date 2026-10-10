//! The existing ISO country type remains the single country vocabulary.
use crate::reference::ReferenceRelease;
pub use isocountry::CountryCode;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("unrecognized canonical ISO 3166-1 alpha-2 country code")]
pub struct InvalidCountryCode;

/// Strict input/rehydration: no casing, trimming, provider aliases or alpha-3 fallback.
pub fn country_from_code(
    code: &str,
    release: ReferenceRelease,
) -> Result<CountryCode, InvalidCountryCode> {
    if release.country_codes().binary_search(&code).is_err() {
        return Err(InvalidCountryCode);
    }
    CountryCode::for_alpha2(code).map_err(|_| InvalidCountryCode)
}
