use crate::core::continent::Continent;
use isocountry::CountryCode;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StructuredAddress {
    pub addressline: Option<String>,
    pub addressline_extra: Option<String>,
    pub locality: Option<String>,
    pub region: Option<String>,
    pub postal_code: Option<String>,
    pub country: Option<CountryCode>,
    pub continent: Option<Continent>,
}

impl StructuredAddress {
    pub fn is_empty(&self) -> bool {
        self.addressline.is_none()
            && self.addressline_extra.is_none()
            && self.locality.is_none()
            && self.region.is_none()
            && self.postal_code.is_none()
            && self.country.is_none()
    }

    pub fn format_for_geocoding(&self) -> Option<String> {
        let mut parts: Vec<String> = Vec::new();
        if let Some(line) = &self.addressline {
            parts.push(line.clone());
        }
        if let Some(line) = &self.addressline_extra {
            parts.push(line.clone());
        }
        parts.extend(
            [
                self.postal_code.as_deref(),
                self.locality.as_deref(),
                self.region.as_deref(),
                self.country.map(|c| c.name()),
            ]
            .into_iter()
            .flatten()
            .map(ToOwned::to_owned),
        );
        let address = parts
            .into_iter()
            .map(|part| part.trim().to_owned())
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(", ");
        (!address.is_empty()).then_some(address)
    }
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct GeoAddress {
    pub lat: f64,
    pub lon: f64,
}

#[cfg(feature = "test-data")]
mod faker {
    use super::{Continent, CountryCode, StructuredAddress};
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for StructuredAddress {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let codes: Vec<CountryCode> = CountryCode::iter().copied().collect();
            let country = Some(codes[rng.random_range(0..codes.len())]);
            let continent = country.map(Continent::from);
            StructuredAddress {
                addressline: config.fake_with_rng(rng),
                addressline_extra: config.fake_with_rng(rng),
                locality: config.fake_with_rng(rng),
                region: config.fake_with_rng(rng),
                postal_code: config.fake_with_rng(rng),
                country,
                continent,
            }
        }
    }
}
