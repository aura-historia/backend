//! Public geographic query DTOs. No text/address/default origin resolution.
use geo::core::discovery::{DistanceOrder, GeographicQuery, ProximityMode};
use geo::{Distance, GeoPoint, SubdivisionCode, country_from_code, reference::ReferenceRelease};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GeographicSearchData {
    /// ISO 3166-1 alpha-2 country codes, at most 100; a listing matches when it is located in any of them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    countries: Vec<String>,
    /// ISO 3166-2 subdivision codes, at most 100, that belong to `countries` when those are given; a listing matches when it is located in any of them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    subdivisions: Vec<String>,
    /// Point that `radius` and `distanceOrder` measure from; required by either of them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    origin: Option<OriginData>,
    /// Only listings within this distance of `origin`, at most 20,000 km.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "crate::wire::schema::optional_distance")]
    radius: Option<geo::data::geography_data::DistanceData>,
    /// Which locations count when measuring distance: `STRICT` (default) only exact premises, `APPROXIMATE` also street, locality and representative points; `APPROXIMATE` needs `origin` with `radius` or `distanceOrder`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mode: Option<String>,
    /// Order results by distance from `origin`, `ASC` (nearest first) or `DESC`; requires `origin`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    distance_order: Option<String>,
}
impl std::fmt::Debug for GeographicSearchData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GeographicSearchData(<private predicates>)")
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct OriginData {
    /// Latitude in decimal degrees.
    latitude: f64,
    /// Longitude in decimal degrees.
    longitude: f64,
}
impl TryFrom<GeographicSearchData> for GeographicQuery {
    type Error = &'static str;
    fn try_from(d: GeographicSearchData) -> Result<Self, Self::Error> {
        let release = ReferenceRelease::CURRENT;
        if d.countries.len() > 100 || d.subdivisions.len() > 100 {
            return Err("geographic code sets are limited to 100 entries");
        }
        let countries = d
            .countries
            .iter()
            .map(|c| country_from_code(c, release))
            .collect::<Result<_, _>>()
            .map_err(|_| "invalid ISO country code")?;
        let subdivisions = d
            .subdivisions
            .iter()
            .map(|s| SubdivisionCode::from_code(s, release))
            .collect::<Result<_, _>>()
            .map_err(|_| "invalid ISO subdivision code")?;
        let origin = d
            .origin
            .map(|p| GeoPoint::new(p.latitude, p.longitude))
            .transpose()
            .map_err(|_| "invalid coordinate origin")?;
        let radius = d
            .radius
            .map(Distance::try_from)
            .transpose()
            .map_err(|_| "invalid radius amount or distance unit")?;
        let mode = d
            .mode
            .as_deref()
            .map(|m| ProximityMode::from_code(m).ok_or("invalid proximity mode"))
            .transpose()?
            .unwrap_or_default();
        let order = d
            .distance_order
            .as_deref()
            .map(|o| DistanceOrder::from_code(o).ok_or("invalid distance order"))
            .transpose()?;
        Self::new(
            release,
            countries,
            subdivisions,
            origin,
            radius,
            mode,
            order,
        )
        .map_err(|_| "incompatible geographic predicates or missing numeric origin")
    }
}
impl From<GeographicQuery> for GeographicSearchData {
    fn from(q: GeographicQuery) -> Self {
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
            countries,
            subdivisions,
            origin: q.origin().map(|p| OriginData {
                latitude: p.latitude_degrees(),
                longitude: p.longitude_degrees(),
            }),
            radius: q.radius().map(Into::into),
            mode: q.origin().map(|_| q.mode().as_str().into()),
            distance_order: q.distance_order().map(|o| o.as_str().into()),
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListingGeographyData {
    #[schemars(schema_with = "crate::wire::schema::party_location_id")]
    location_id: party_core::party_location_id::PartyLocationId,
    basis: &'static str,
    reference_release: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    country: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    subdivision: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    point: Option<OriginData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    precision: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    distance_km: Option<f64>,
}
impl From<product_listing_service::geographic_search::ListingDiscoveryGeography>
    for ListingGeographyData
{
    fn from(d: product_listing_service::geographic_search::ListingDiscoveryGeography) -> Self {
        Self {
            location_id: d.geography.location_id,
            basis: product_listing_service::geography::GeographicDimension::BASIS,
            reference_release: d.geography.reference_release.as_str(),
            country: d.geography.country.map(|c| c.alpha2()),
            subdivision: d.geography.subdivision.map(|s| s.as_str()),
            point: d.geography.position.map(|p| OriginData {
                latitude: p.point().latitude_degrees(),
                longitude: p.point().longitude_degrees(),
            }),
            precision: d.geography.position.map(|p| p.precision().as_str()),
            distance_km: d.distance_km,
        }
    }
}
