//! Release-pinned compatibility codec for private geographic predicates.
use crate::core::discovery::{DistanceOrder, GeographicQuery, ProximityMode};
use crate::{GeoPoint, SubdivisionCode, country_from_code, reference::ReferenceRelease};
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeographicQueryData {
    reference_release: String,
    countries: Vec<String>,
    subdivisions: Vec<String>,
    origin: Option<super::geography_data::GeoPointData>,
    radius_km: Option<f64>,
    mode: String,
    distance_order: Option<String>,
}
impl std::fmt::Debug for GeographicQueryData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GeographicQueryData(<private predicates>)")
    }
}
#[derive(Debug, thiserror::Error)]
pub enum InvalidGeographicQueryData {
    #[error("invalid geographic value")]
    Value(
        #[from]
        #[source]
        super::geography_data::InvalidGeographyData,
    ),
    #[error("invalid geographic query combination")]
    Query(
        #[from]
        #[source]
        crate::core::discovery::InvalidGeographicQuery,
    ),
    #[error("unsupported canonical proximity mode or distance order")]
    Code,
}
impl TryFrom<GeographicQueryData> for GeographicQuery {
    type Error = InvalidGeographicQueryData;
    fn try_from(d: GeographicQueryData) -> Result<Self, Self::Error> {
        if d.countries.len() > 100 || d.subdivisions.len() > 100 {
            return Err(crate::core::discovery::InvalidGeographicQuery::Codes.into());
        }
        let invalid = InvalidGeographicQueryData::Value;
        let release = ReferenceRelease::from_code(&d.reference_release)
            .map_err(super::geography_data::InvalidGeographyData::from)
            .map_err(invalid)?;
        let countries = d
            .countries
            .into_iter()
            .map(|c| country_from_code(&c, release))
            .collect::<Result<_, _>>()
            .map_err(super::geography_data::InvalidGeographyData::from)
            .map_err(invalid)?;
        let subdivisions = d
            .subdivisions
            .into_iter()
            .map(|s| SubdivisionCode::from_code(&s, release))
            .collect::<Result<_, _>>()
            .map_err(super::geography_data::InvalidGeographyData::from)
            .map_err(invalid)?;
        let origin = d
            .origin
            .map(GeoPoint::try_from)
            .transpose()
            .map_err(super::geography_data::InvalidGeographyData::from)
            .map_err(invalid)?;
        let mode = ProximityMode::from_code(&d.mode).ok_or(InvalidGeographicQueryData::Code)?;
        let order = d
            .distance_order
            .as_deref()
            .map(|o| DistanceOrder::from_code(o).ok_or(InvalidGeographicQueryData::Code))
            .transpose()?;
        Self::new(
            release,
            countries,
            subdivisions,
            origin,
            d.radius_km,
            mode,
            order,
        )
        .map_err(Into::into)
    }
}
impl From<&GeographicQuery> for GeographicQueryData {
    fn from(q: &GeographicQuery) -> Self {
        let mut countries = q
            .countries()
            .iter()
            .map(|c| c.alpha2().to_owned())
            .collect::<Vec<_>>();
        let mut subdivisions = q
            .subdivisions()
            .iter()
            .map(|s| s.as_str().to_owned())
            .collect::<Vec<_>>();
        countries.sort_unstable();
        subdivisions.sort_unstable();
        Self {
            reference_release: q.reference_release().as_str().into(),
            countries,
            subdivisions,
            origin: q.origin().map(Into::into),
            radius_km: q.radius_km(),
            mode: q.mode().as_str().into(),
            distance_order: q.distance_order().map(|o| o.as_str().into()),
        }
    }
}
