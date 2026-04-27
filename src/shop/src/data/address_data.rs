use crate::core::{
    address::{GeoAddress, StructuredAddress},
    continent::Continent,
};
use isocountry::CountryCode;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StructuredAddressData {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub addressline: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub addressline_extra: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub locality: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub region: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub postal_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub country: Option<CountryCode>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub continent: Option<Continent>,
}

impl From<StructuredAddress> for StructuredAddressData {
    fn from(address: StructuredAddress) -> Self {
        Self {
            addressline: address.addressline,
            addressline_extra: address.addressline_extra,
            locality: address.locality,
            region: address.region,
            postal_code: address.postal_code,
            country: address.country,
            continent: address.continent,
        }
    }
}

impl From<StructuredAddressData> for StructuredAddress {
    fn from(address: StructuredAddressData) -> Self {
        let continent = address
            .continent
            .or_else(|| address.country.map(Continent::from));
        Self {
            addressline: address.addressline,
            addressline_extra: address.addressline_extra,
            locality: address.locality,
            region: address.region,
            postal_code: address.postal_code,
            country: address.country,
            continent,
        }
    }
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Copy, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeoAddressData {
    pub lat: f64,
    pub lon: f64,
}

impl From<GeoAddress> for GeoAddressData {
    fn from(address: GeoAddress) -> Self {
        Self {
            lat: address.lat,
            lon: address.lon,
        }
    }
}

impl From<GeoAddressData> for GeoAddress {
    fn from(address: GeoAddressData) -> Self {
        Self {
            lat: address.lat,
            lon: address.lon,
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::{Continent, CountryCode, StructuredAddressData};
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for StructuredAddressData {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let codes: Vec<CountryCode> = CountryCode::iter().copied().collect();
            let country = Some(codes[rng.random_range(0..codes.len())]);
            let continent = country.map(Continent::from);
            StructuredAddressData {
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
