//! Public geographic query DTOs. No text/address/default origin resolution.
use geo::core::discovery::{DistanceOrder, GeographicQuery, ProximityMode};
use geo::{GeoPoint, SubdivisionCode, country_from_code, reference::ReferenceRelease};
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct GeographicSearchData {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    countries: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    subdivisions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    origin: Option<OriginData>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    radius_km: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    distance_order: Option<String>,
}
impl std::fmt::Debug for GeographicSearchData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GeographicSearchData(<private predicates>)")
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OriginData {
    latitude: f64,
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
            d.radius_km,
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
            radius_km: q.radius_km(),
            mode: q.origin().map(|_| q.mode().as_str().into()),
            distance_order: q.distance_order().map(|o| o.as_str().into()),
        }
    }
}

pub(crate) fn parse_query(
    d: Option<GeographicSearchData>,
    field: &'static str,
) -> Result<Option<GeographicQuery>, crate::error::ApiError> {
    d.map(GeographicQuery::try_from)
        .transpose()
        .map_err(|detail| {
            crate::error::ApiError::bad_request(crate::error::BAD_QUERY_PARAMETER_VALUE)
                .with_query_field(field)
                .with_detail(detail)
        })
}
pub(crate) fn parse_body(
    d: Option<GeographicSearchData>,
    field: &'static str,
) -> Result<Option<GeographicQuery>, crate::error::ApiError> {
    d.map(GeographicQuery::try_from)
        .transpose()
        .map_err(|detail| {
            crate::error::ApiError::bad_request(crate::error::BAD_BODY_VALUE)
                .with_body_field(field)
                .with_detail(detail)
        })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ListingGeographyData {
    #[serde(skip_serializing_if = "Option::is_none")]
    business: Option<DiscoveryDimensionData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    inventory: Option<DiscoveryDimensionData>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiscoveryDimensionData {
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
impl From<product_listing_service::geographic_search::DiscoveryDimension>
    for DiscoveryDimensionData
{
    fn from(d: product_listing_service::geographic_search::DiscoveryDimension) -> Self {
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
impl From<product_listing_service::geographic_search::ListingDiscoveryGeography>
    for ListingGeographyData
{
    fn from(g: product_listing_service::geographic_search::ListingDiscoveryGeography) -> Self {
        Self {
            business: g.business.map(Into::into),
            inventory: g.inventory.map(Into::into),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn diagnostics_hide_private_coordinate_snapshots_at_every_boundary() {
        let data: GeographicSearchData = serde_json::from_value(serde_json::json!({
            "origin": {"latitude":48.137,"longitude":11.575}, "radiusKm":25
        }))
        .unwrap();
        assert_eq!(
            "GeographicSearchData(<private predicates>)",
            format!("{data:?}")
        );
        let query = GeographicQuery::try_from(data).unwrap();
        let snapshot = geo::data::discovery_data::GeographicQueryData::from(&query);
        assert_eq!(
            "GeographicQueryData(<private predicates>)",
            format!("{snapshot:?}")
        );
        assert_eq!(
            "GeographicQuery(<private predicates>)",
            format!("{query:?}")
        );
    }
    #[test]
    fn numeric_origin_is_required_and_unknown_origin_sources_fail() {
        for value in [
            serde_json::json!({"radiusKm":25}),
            serde_json::json!({"origin":{"latitude":48.1},"radiusKm":25}),
            serde_json::json!({"origin":{"latitude":"48.1","longitude":11.5},"radiusKm":25}),
            serde_json::json!({"origin":{"latitude":91,"longitude":11.5},"radiusKm":25}),
            serde_json::json!({"origin":{"text":"Munich"},"radiusKm":25}),
            serde_json::json!({"userAddressId":"uaddr_example","radiusKm":25}),
            serde_json::json!({"primaryAddress":true,"radiusKm":25}),
            serde_json::json!({"preference":true,"radiusKm":25}),
            serde_json::json!({"countries":["DE"],"subdivisions":["FR-IDF"]}),
        ] {
            let valid = serde_json::from_value::<GeographicSearchData>(value)
                .ok()
                .and_then(|d| GeographicQuery::try_from(d).ok());
            assert!(valid.is_none());
        }
        let d: GeographicSearchData =
            serde_qs::from_str("origin[latitude]=NaN&origin[longitude]=11.5&radiusKm=25").unwrap();
        assert!(GeographicQuery::try_from(d).is_err());
        let d: GeographicSearchData = serde_qs::from_str(
            "origin[latitude]=48.137&origin[longitude]=11.575&radiusKm=25&distanceOrder=ASC",
        )
        .unwrap();
        assert_eq!(
            Some(25.0),
            GeographicQuery::try_from(d).unwrap().radius_km()
        );
    }
}
