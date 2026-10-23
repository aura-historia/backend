//! Public location discovery contracts, separate from protected location management.
pub mod management;

use crate::geographic_search::GeographicSearchData;
use application::pagination::Cursor;
use party_core::{party_id::PartyId, party_location_id::PartyLocationId};
use party_service::discovery::{
    DiscoverPartyLocationsRequest, DiscoverPartyLocationsResult, PartyLocationDiscoveryCursor,
    PublicPartyLocation,
};
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiscoverPartyLocationsData {
    geography: GeographicSearchData,
    /// Page size from 1 through 100, validated by the discovery use case.
    #[serde(default = "default_size")]
    #[schemars(range(max = u64::MAX))]
    size: u64,
    #[serde(default)]
    search_after: Option<PartyLocationCursorData>,
}

fn default_size() -> u64 {
    21
}

impl TryFrom<DiscoverPartyLocationsData> for DiscoverPartyLocationsRequest {
    type Error = &'static str;
    fn try_from(data: DiscoverPartyLocationsData) -> Result<Self, Self::Error> {
        Ok(Self {
            geography: data.geography.try_into()?,
            cursor: Cursor {
                size: data.size,
                search_after: data.search_after.map(Into::into),
            },
        })
    }
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PartyLocationCursorData {
    #[schemars(schema_with = "crate::wire::schema::party_location_id")]
    location_id: PartyLocationId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    distance_km: Option<f64>,
}

impl From<PartyLocationCursorData> for PartyLocationDiscoveryCursor {
    fn from(data: PartyLocationCursorData) -> Self {
        Self {
            location_id: data.location_id,
            distance_km: data.distance_km,
        }
    }
}

impl From<PartyLocationDiscoveryCursor> for PartyLocationCursorData {
    fn from(cursor: PartyLocationDiscoveryCursor) -> Self {
        Self {
            location_id: cursor.location_id,
            distance_km: cursor.distance_km,
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PublicPartyLocationData {
    #[schemars(schema_with = "party_id_schema")]
    party_id: PartyId,
    party_name: String,
    #[schemars(schema_with = "crate::wire::schema::party_location_id")]
    location_id: PartyLocationId,
    label: String,
    basis: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    country: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    subdivision: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    point: Option<PointData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    precision: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    distance_km: Option<f64>,
}

fn party_id_schema(_: &mut SchemaGenerator) -> Schema {
    crate::wire::schema::object_id(PartyId::PREFIX)
}

#[derive(Debug, Serialize, JsonSchema)]
struct PointData {
    latitude: f64,
    longitude: f64,
}

impl From<PublicPartyLocation> for PublicPartyLocationData {
    fn from(value: PublicPartyLocation) -> Self {
        Self {
            party_id: value.party_id,
            party_name: value.party_name,
            location_id: value.location_id,
            label: value.label,
            basis: value.basis,
            country: value.country.map(|country| country.alpha2()),
            subdivision: value.subdivision.map(|subdivision| subdivision.as_str()),
            point: value.position.map(|position| PointData {
                latitude: position.point().latitude_degrees(),
                longitude: position.point().longitude_degrees(),
            }),
            precision: value.position.map(|position| position.precision().as_str()),
            distance_km: value.distance_km,
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DiscoverPartyLocationsResultData {
    items: Vec<PublicPartyLocationData>,
    #[schemars(range(max = usize::MAX))]
    size: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    search_after: Option<PartyLocationCursorData>,
}

impl From<DiscoverPartyLocationsResult> for DiscoverPartyLocationsResultData {
    fn from(result: DiscoverPartyLocationsResult) -> Self {
        let items = result.items.into_iter().map(Into::into).collect::<Vec<_>>();
        Self {
            size: items.len(),
            items,
            search_after: result.cursor.search_after.map(Into::into),
        }
    }
}
