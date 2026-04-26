use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StructuredAddress {
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub address_lines: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub locality: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub region: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub postal_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub country: Option<String>,
}

impl StructuredAddress {
    pub fn is_empty(&self) -> bool {
        self.address_lines.is_empty()
            && self.locality.is_none()
            && self.region.is_none()
            && self.postal_code.is_none()
            && self.country.is_none()
    }

    pub fn format_for_geocoding(&self) -> Option<String> {
        let mut parts = self.address_lines.clone();
        parts.extend(
            [
                self.postal_code.as_ref(),
                self.locality.as_ref(),
                self.region.as_ref(),
                self.country.as_ref(),
            ]
            .into_iter()
            .flatten()
            .cloned(),
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
#[derive(Debug, Copy, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeoAddress {
    pub lat: f64,
    pub lon: f64,
}

impl GeoAddress {
    pub fn to_opensearch_geo_point(self) -> String {
        format!("{},{}", self.lat, self.lon)
    }

    pub fn from_opensearch_geo_point(value: &str) -> Option<Self> {
        let (lat, lon) = value.split_once(',')?;
        Some(Self {
            lat: lat.trim().parse().ok()?,
            lon: lon.trim().parse().ok()?,
        })
    }
}
