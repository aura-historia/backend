use aura_historia_contracts::party_locations::{
    DiscoverPartyLocationsData, DiscoverPartyLocationsResultData,
};
use axum::{
    Json,
    extract::{RawQuery, State},
    response::{IntoResponse, Response},
};
use party_service::discovery::*;
use std::sync::Arc;

pub async fn discover(
    State(use_case): State<Arc<dyn DiscoverPartyLocationsUseCase>>,
    RawQuery(raw): RawQuery,
) -> Response {
    let data = match crate::wire::from_query_str::<DiscoverPartyLocationsData>(
        raw.as_deref().unwrap_or_default(),
    ) {
        Ok(data) => data,
        Err(_) => {
            return crate::error::ApiError::bad_request(crate::error::BAD_QUERY_PARAMETER_VALUE)
                .into_response();
        }
    };
    let request = match DiscoverPartyLocationsRequest::try_from(data) {
        Ok(request) => request,
        Err(detail) => {
            return crate::error::ApiError::bad_request(crate::error::BAD_QUERY_PARAMETER_VALUE)
                .with_detail(detail)
                .into_response();
        }
    };
    match use_case.execute(request).await {
        Ok(result) => crate::transport::cache::private_no_store(
            Json(DiscoverPartyLocationsResultData::from(result)).into_response(),
        ),
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
    use application::pagination::Cursor;
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
