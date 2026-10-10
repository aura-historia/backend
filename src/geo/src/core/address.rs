use super::{
    country::CountryCode,
    description::{GeographicDescription, InvalidGeographicDescription},
    text::{AddressText, InvalidGeoText, PostalCode},
};
use crate::core::continent::Continent;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidStructuredAddress {
    #[error("invalid legacy address text")]
    Text(
        #[from]
        #[source]
        InvalidGeoText,
    ),
    #[error("invalid legacy geographic assertions")]
    Description(
        #[from]
        #[source]
        InvalidGeographicDescription,
    ),
}

/// Legacy structured observations. Region remains free text, never an ISO subdivision.
/// Use `to_description` to cross into validated geographic values.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StructuredAddress {
    pub addressline: Option<String>,
    pub addressline_extra: Option<String>,
    pub locality: Option<String>,
    pub region: Option<String>,
    pub postal_code: Option<String>,
    pub country: Option<CountryCode>,
}

impl StructuredAddress {
    pub fn continent(&self) -> Option<Continent> {
        self.country.and_then(Continent::country_grouping)
    }

    pub fn to_description(
        &self,
    ) -> Result<Option<GeographicDescription>, InvalidStructuredAddress> {
        let parts = [
            self.addressline.as_deref(),
            self.addressline_extra.as_deref(),
            self.locality.as_deref(),
            self.region.as_deref(),
        ]
        .into_iter()
        .flatten()
        .map(AddressText::new)
        .collect::<Result<Vec<_>, _>>()?;
        let address_text = if parts.is_empty() {
            None
        } else {
            Some(AddressText::new(
                parts
                    .iter()
                    .map(AddressText::as_str)
                    .collect::<Vec<_>>()
                    .join("\n"),
            )?)
        };
        let postal_code = self
            .postal_code
            .as_deref()
            .map(PostalCode::new)
            .transpose()?;
        GeographicDescription::new(address_text, self.country, None, postal_code)
            .map_err(Into::into)
    }

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

#[cfg(feature = "test-data")]
mod faker {
    use super::{CountryCode, StructuredAddress};
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for StructuredAddress {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let codes: Vec<CountryCode> = CountryCode::iter().copied().collect();
            let country = Some(codes[rng.random_range(0..codes.len())]);
            StructuredAddress {
                addressline: config.fake_with_rng(rng),
                addressline_extra: config.fake_with_rng(rng),
                locality: config.fake_with_rng(rng),
                region: config.fake_with_rng(rng),
                postal_code: config.fake_with_rng(rng),
                country,
            }
        }
    }
}
