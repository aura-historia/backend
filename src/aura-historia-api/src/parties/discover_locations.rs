use application::pagination::Cursor;
use axum::{
    Json,
    extract::{RawQuery, State},
    response::{IntoResponse, Response},
};
use party_service::discovery::*;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct QueryData {
    geography: crate::geographic_search::GeographicSearchData,
    #[serde(default = "default_size")]
    size: u64,
    #[serde(default)]
    search_after: Option<CursorData>,
}
fn default_size() -> u64 {
    21
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CursorData {
    location_id: party_core::party_location_id::PartyLocationId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    distance_km: Option<f64>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LocationData {
    party_id: party_core::party_id::PartyId,
    party_name: String,
    location_id: party_core::party_location_id::PartyLocationId,
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
#[derive(Serialize)]
struct PointData {
    latitude: f64,
    longitude: f64,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ResultData {
    items: Vec<LocationData>,
    size: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    search_after: Option<CursorData>,
}

pub async fn discover(
    State(use_case): State<Arc<dyn DiscoverPartyLocationsUseCase>>,
    RawQuery(raw): RawQuery,
) -> Response {
    let data = match crate::wire::from_query_str::<QueryData>(raw.as_deref().unwrap_or_default()) {
        Ok(data) => data,
        Err(_) => {
            return crate::error::ApiError::bad_request(crate::error::BAD_QUERY_PARAMETER_VALUE)
                .into_response();
        }
    };
    let geography = match geo::core::discovery::GeographicQuery::try_from(data.geography) {
        Ok(query) => query,
        Err(detail) => {
            return crate::error::ApiError::bad_request(crate::error::BAD_QUERY_PARAMETER_VALUE)
                .with_detail(detail)
                .into_response();
        }
    };
    match use_case
        .execute(DiscoverPartyLocationsRequest {
            geography,
            cursor: Cursor {
                size: data.size,
                search_after: data.search_after.map(|c| PartyLocationDiscoveryCursor {
                    location_id: c.location_id,
                    distance_km: c.distance_km,
                }),
            },
        })
        .await
    {
        Ok(result) => {
            let items = result
                .items
                .into_iter()
                .map(|v| LocationData {
                    party_id: v.party_id,
                    party_name: v.party_name,
                    location_id: v.location_id,
                    label: v.label,
                    basis: v.basis,
                    country: v.country.map(|c| c.alpha2()),
                    subdivision: v.subdivision.map(|s| s.as_str()),
                    point: v.position.map(|p| PointData {
                        latitude: p.point().latitude_degrees(),
                        longitude: p.point().longitude_degrees(),
                    }),
                    precision: v.position.map(|p| p.precision().as_str()),
                    distance_km: v.distance_km,
                })
                .collect::<Vec<_>>();
            crate::transport::cache::private_no_store(
                Json(ResultData {
                    size: items.len(),
                    items,
                    search_after: result.cursor.search_after.map(|c| CursorData {
                        location_id: c.location_id,
                        distance_km: c.distance_km,
                    }),
                })
                .into_response(),
            )
        }
        Err(DiscoverPartyLocationsError::InvalidInput) => {
            crate::error::ApiError::bad_request(crate::error::BAD_QUERY_PARAMETER_VALUE)
                .into_response()
        }
        Err(_) => {
            crate::error::ApiError::service_unavailable(crate::error::PARTY_TEMPORARILY_UNAVAILABLE)
                .into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{StatusCode, header};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Discovery(Arc<AtomicUsize>);
    #[async_trait::async_trait]
    impl DiscoverPartyLocationsUseCase for Discovery {
        async fn execute(
            &self,
            request: DiscoverPartyLocationsRequest,
        ) -> Result<DiscoverPartyLocationsResult, DiscoverPartyLocationsError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            assert_eq!(None, request.geography.origin());
            assert!(
                request
                    .geography
                    .countries()
                    .contains(&geo::CountryCode::DEU)
            );
            Ok(DiscoverPartyLocationsResult {
                cursor: Cursor {
                    size: 1,
                    search_after: None,
                },
                total: None,
                items: vec![PublicPartyLocation {
                    party_id: party_core::party_id::PartyId::new(),
                    party_name: "Munich business".to_owned(),
                    location_id: party_core::party_location_id::PartyLocationId::new(),
                    label: "Gallery".to_owned(),
                    country: Some(geo::CountryCode::DEU),
                    subdivision: Some(geo::SubdivisionCode::new("DE-BY").unwrap()),
                    position: Some(
                        geo::SpatialPosition::new(
                            geo::GeoPoint::new(48.137, 11.575).unwrap(),
                            geo::SpatialPrecision::Locality,
                            None,
                        )
                        .unwrap(),
                    ),
                    basis: "ACCEPTED_DERIVED",
                    distance_km: None,
                }],
            })
        }
    }
    #[tokio::test]
    async fn anonymous_discovery_exposes_eligible_point_and_precision_without_management_state() {
        let calls = Arc::new(AtomicUsize::new(0));
        let uc: Arc<dyn DiscoverPartyLocationsUseCase> = Arc::new(Discovery(calls.clone()));
        let response = discover(
            State(uc),
            RawQuery(Some(
                "geography[countries][0]=DE&geography[subdivisions][0]=DE-BY".into(),
            )),
        )
        .await;
        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(
            "private, no-store",
            response.headers()[header::CACHE_CONTROL]
        );
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(1, json["size"]);
        assert_eq!("LOCALITY", json["items"][0]["precision"]);
        assert_eq!("ACCEPTED_DERIVED", json["items"][0]["basis"]);
        assert_eq!(48.137, json["items"][0]["point"]["latitude"]);
        let mut fields = json["items"][0]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>();
        fields.sort_unstable();
        assert_eq!(
            vec![
                "basis",
                "country",
                "label",
                "locationId",
                "partyId",
                "partyName",
                "point",
                "precision",
                "subdivision"
            ],
            fields
        );
        assert_eq!(1, calls.load(Ordering::SeqCst));
    }
    #[tokio::test]
    async fn percent_encoded_deep_object_keys_are_accepted_like_literal_brackets() {
        let calls = Arc::new(AtomicUsize::new(0));
        let uc: Arc<dyn DiscoverPartyLocationsUseCase> = Arc::new(Discovery(calls.clone()));
        let query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("size", "100")
            .append_pair("geography[countries][0]", "DE")
            .finish();
        assert!(query.contains("geography%5Bcountries%5D%5B0%5D=DE"));
        let response = discover(State(uc.clone()), RawQuery(Some(query))).await;
        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(1, calls.load(Ordering::SeqCst));

        let rejected = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("geography[countries][0]", "DE")
            .append_pair("geography[subdivisions][0]", "FR-IDF")
            .finish();
        let response = discover(State(uc), RawQuery(Some(rejected))).await;
        assert_eq!(StatusCode::BAD_REQUEST, response.status());
        assert_eq!(1, calls.load(Ordering::SeqCst));
    }
    #[tokio::test]
    async fn invalid_or_implicit_origins_never_start_public_discovery() {
        let calls = Arc::new(AtomicUsize::new(0));
        let uc: Arc<dyn DiscoverPartyLocationsUseCase> = Arc::new(Discovery(calls.clone()));
        for query in [
            "",
            "geography[radius][amount]=25&geography[radius][unit]=KILOMETERS",
            "geography[origin][latitude]=91&geography[origin][longitude]=0&geography[radius][amount]=25&geography[radius][unit]=KILOMETERS",
            "geography[origin][address]=Munich&geography[radius][amount]=25&geography[radius][unit]=KILOMETERS",
            "geography[origin][userAddressId]=primary&geography[radius][amount]=25&geography[radius][unit]=KILOMETERS",
            "geography[countries][0]=DE&geography[subdivisions][0]=FR-IDF",
            "geography[countries][0]=DE&disclosure=public",
        ] {
            let response = discover(State(uc.clone()), RawQuery(Some(query.to_owned()))).await;
            assert_eq!(StatusCode::BAD_REQUEST, response.status(), "{query}");
        }
        assert_eq!(0, calls.load(Ordering::SeqCst));
    }
}
