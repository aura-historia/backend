use geo::{
    AddressText, GeoPoint, GeographicDescription, PostalCode, SpatialPosition, SpatialPrecision,
    SubdivisionCode, country_from_code, reference::ReferenceRelease,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid geographic input")]
pub struct InvalidGeographyData;

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GeographyData {
    pub reference_release: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subdivision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub postal_code: Option<String>,
}
impl GeographyData {
    pub fn into_core(self) -> Result<GeographicDescription, InvalidGeographyData> {
        let release = ReferenceRelease::from_code(&self.reference_release)
            .map_err(|_| InvalidGeographyData)?;
        GeographicDescription::new_in_release(
            release,
            self.address_text
                .map(AddressText::new)
                .transpose()
                .map_err(|_| InvalidGeographyData)?,
            self.country
                .map(|v| country_from_code(&v, release))
                .transpose()
                .map_err(|_| InvalidGeographyData)?,
            self.subdivision
                .map(|v| SubdivisionCode::from_code(&v, release))
                .transpose()
                .map_err(|_| InvalidGeographyData)?,
            self.postal_code
                .map(PostalCode::new)
                .transpose()
                .map_err(|_| InvalidGeographyData)?,
        )
        .map_err(|_| InvalidGeographyData)?
        .ok_or(InvalidGeographyData)
    }
}
impl From<GeographicDescription> for GeographyData {
    fn from(v: GeographicDescription) -> Self {
        Self {
            reference_release: v.reference_release().as_str().to_owned(),
            address_text: v.address_text().map(|v| v.as_str().to_owned()),
            country: v.country().map(|v| v.alpha2().to_owned()),
            subdivision: v.subdivision().map(|v| v.as_str().to_owned()),
            postal_code: v.postal_code().map(|v| v.as_str().to_owned()),
        }
    }
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PositionData {
    pub latitude_degrees: f64,
    pub longitude_degrees: f64,
    pub precision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accuracy_metres: Option<f64>,
}
impl PositionData {
    pub fn into_core(self) -> Result<SpatialPosition, InvalidGeographyData> {
        SpatialPosition::new(
            GeoPoint::new(self.latitude_degrees, self.longitude_degrees)
                .map_err(|_| InvalidGeographyData)?,
            SpatialPrecision::from_code(&self.precision).ok_or(InvalidGeographyData)?,
            self.accuracy_metres,
        )
        .map_err(|_| InvalidGeographyData)
    }
}
impl From<SpatialPosition> for PositionData {
    fn from(v: SpatialPosition) -> Self {
        Self {
            latitude_degrees: v.point().latitude_degrees(),
            longitude_degrees: v.point().longitude_degrees(),
            precision: v.precision().as_str().to_owned(),
            accuracy_metres: v.accuracy_metres(),
        }
    }
}
