use application::pagination::{Cursor, CursoredResult};
use aura_historia_contracts::{
    listing_sources::{
        GetPublicListingSourceBySlugData, PublicListingSourceData,
        PublicListingSourceSearchCollectionData, SearchPublicListingSourcesData,
        cursor::{SourceCursorError, encode_continuation, parse_continuation},
    },
    party_locations::{DiscoverPartyLocationsData, DiscoverPartyLocationsResultData},
    public_overview::PublicOverviewData,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
use listing_source_service::use_cases::queries::{
    public_listing_source::{PublicListingSourceOperatorSummary, PublicListingSourceSummary},
    search_public_listing_sources::{
        PublicListingSourceSearchContinuation, PublicListingSourceSearchPosition,
        SearchPublicListingSourcesRequest, SearchPublicListingSourcesResult,
    },
};
use party_core::{party_id::PartyId, party_location_id::PartyLocationId, party_name::PartyName};
use party_service::discovery::{
    DiscoverPartyLocationsRequest, PartyLocationDiscoveryCursor, PublicPartyLocation,
};
use product_listing_service::use_cases::PublicCatalogueOverview;
use schemars::{JsonSchema, generate::SchemaSettings};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/public-discovery.json")).unwrap()
}
fn input_schema<T: JsonSchema>() -> Value {
    serde_json::to_value(
        SchemaSettings::default()
            .for_deserialize()
            .into_generator()
            .into_root_schema_for::<T>(),
    )
    .unwrap()
}
fn check_shapes<T: DeserializeOwned + JsonSchema>(samples: &Value, accepted: bool) {
    let schema = input_schema::<T>();
    for sample in samples.as_array().unwrap() {
        assert_eq!(
            accepted,
            serde_json::from_value::<T>(sample.clone()).is_ok(),
            "Serde: {sample}"
        );
        assert_eq!(
            accepted,
            jsonschema::is_valid(&schema, sample),
            "schema: {sample}"
        );
    }
}
fn output<T: Serialize + JsonSchema>(data: T) -> Value {
    let schema = SchemaSettings::default()
        .for_serialize()
        .into_generator()
        .into_root_schema_for::<T>();
    let value = serde_json::to_value(data).unwrap();
    assert!(
        jsonschema::is_valid(schema.as_value(), &value),
        "actual output: {value}"
    );
    value
}
fn source_id() -> ListingSourceId {
    "ls_01h455vb4pex5vy7enb1p677vn".parse().unwrap()
}
fn location_id() -> PartyLocationId {
    "ploc_01h455vb4pex5vy7enb1p677vn".parse().unwrap()
}

#[test]
fn should_agree_on_shared_source_input_shapes_and_run_semantic_constructors() {
    let fixtures = fixture();
    check_shapes::<SearchPublicListingSourcesData>(&fixtures["sourceSearchInputs"], true);
    check_shapes::<SearchPublicListingSourcesData>(&fixtures["invalidSourceSearchShapes"], false);
    for value in fixtures["sourceSearchInputs"].as_array().unwrap() {
        let input =
            serde_json::from_value::<SearchPublicListingSourcesData>(value.clone()).unwrap();
        assert!(SearchPublicListingSourcesRequest::try_from(input).is_ok());
    }
    for value in [
        json!({"query":"a".repeat(257)}),
        json!({"query":"a\u{0000}b"}),
        json!({"size":0}),
        json!({"size":51}),
        json!({"searchAfter":"invalid"}),
    ] {
        let input = serde_json::from_value::<SearchPublicListingSourcesData>(value).unwrap();
        assert!(SearchPublicListingSourcesRequest::try_from(input).is_err());
    }
    check_shapes::<GetPublicListingSourceBySlugData>(
        &json!([{"listingSourceSlugId":"museum-source"}]),
        true,
    );
    check_shapes::<GetPublicListingSourceBySlugData>(
        &json!([{}, {"listingSourceSlugId":"museum-source","providerConfiguration":{}}]),
        false,
    );
}

#[test]
fn should_preserve_bounded_source_cursor_encoding_and_validate_position_and_ids() {
    let continuation = PublicListingSourceSearchContinuation::new(
        [7; 32],
        PublicListingSourceSearchPosition::new(1, "museum source".into(), source_id()).unwrap(),
    );
    let encoded = encode_continuation(&continuation).unwrap();
    assert_eq!(continuation, parse_continuation(&encoded).unwrap());
    let payload: Value =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(&encoded).unwrap()).unwrap();
    assert_eq!(
        json!({"binding":URL_SAFE_NO_PAD.encode([7;32]),"matchTier":1,"nameSearch":"museum source","listingSourceId":source_id()}),
        payload
    );
    assert!(matches!(
        parse_continuation(&"x".repeat(4097)),
        Err(SourceCursorError::TooLarge)
    ));
    assert!(parse_continuation(&URL_SAFE_NO_PAD.encode(vec![b' '; 3073])).is_err());
    for change in [
        json!({"matchTier":6}),
        json!({"nameSearch":""}),
        json!({"nameSearch":"x".repeat(2049)}),
        json!({"binding":"a"}),
        json!({"listingSourceId":"ploc_01h455vb4pex5vy7enb1p677vn"}),
        json!({"unknown":true}),
    ] {
        let mut changed = payload.clone();
        changed
            .as_object_mut()
            .unwrap()
            .extend(change.as_object().unwrap().clone());
        let encoded = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&changed).unwrap());
        assert!(parse_continuation(&encoded).is_err(), "{changed}");
    }
}

#[test]
fn should_expose_exact_public_source_fields_and_omit_absent_images_and_urls() {
    let source = PublicListingSourceSummary {
        listing_source_id: source_id(),
        listing_source_slug_id: ListingSourceSlugId::raw("museum-source").unwrap(),
        name: ListingSourceName::try_from("Museum Source").unwrap(),
        operator: PublicListingSourceOperatorSummary {
            name: PartyName::try_from("Museum Operator").unwrap(),
        },
        url: None,
        image: None,
    };
    let detail = output(PublicListingSourceData::from(source.clone()));
    assert_eq!(
        json!({"listingSourceId":source_id(),"listingSourceSlugId":"museum-source","name":"Museum Source","operator":{"name":"Museum Operator"}}),
        detail
    );
    let collection = output(PublicListingSourceSearchCollectionData::new(
        SearchPublicListingSourcesResult {
            items: vec![source],
            page_size: 21,
            continuation: None,
        },
        None,
    ));
    assert_eq!(json!({"items":[detail],"size":1}), collection);
    assert_eq!(
        json!({"items":[],"size":0}),
        output(PublicListingSourceSearchCollectionData::new(
            SearchPublicListingSourcesResult {
                items: vec![],
                page_size: 21,
                continuation: None
            },
            None
        ))
    );
}

#[test]
fn should_describe_complete_geography_and_location_cursor_shapes() {
    let fixtures = fixture();
    check_shapes::<DiscoverPartyLocationsData>(&fixtures["locationInputs"], true);
    check_shapes::<DiscoverPartyLocationsData>(&fixtures["invalidLocationShapes"], false);
    for value in fixtures["locationInputs"].as_array().unwrap() {
        let input = serde_json::from_value::<DiscoverPartyLocationsData>(value.clone()).unwrap();
        assert!(DiscoverPartyLocationsRequest::try_from(input).is_ok());
    }
    for geography in [
        json!({}),
        json!({"countries":["XX"]}),
        json!({"countries":["DE"],"subdivisions":["FR-IDF"]}),
        json!({"origin":{"latitude":91,"longitude":0},"distanceOrder":"ASC"}),
        json!({"radius":{"amount":25,"unit":"KILOMETERS"}}),
    ] {
        let input =
            serde_json::from_value::<DiscoverPartyLocationsData>(json!({"geography":geography}))
                .unwrap();
        assert!(DiscoverPartyLocationsRequest::try_from(input).is_err());
    }
}

#[test]
fn should_keep_all_public_location_context_and_cursor_fields_in_output_schema() {
    let result = CursoredResult {
        items: vec![PublicPartyLocation {
            party_id: "pty_01h455vb4pex5vy7enb1p677vn".parse::<PartyId>().unwrap(),
            party_name: "Museum Operator".into(),
            location_id: location_id(),
            label: "Gallery".into(),
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
            distance_km: Some(23.5),
        }],
        total: None,
        cursor: Cursor {
            size: 100,
            search_after: Some(PartyLocationDiscoveryCursor {
                location_id: location_id(),
                distance_km: Some(23.5),
            }),
        },
    };
    let result = output(DiscoverPartyLocationsResultData::from(result));
    assert_eq!(
        json!({"items":[{"partyId":"pty_01h455vb4pex5vy7enb1p677vn","partyName":"Museum Operator","locationId":location_id(),"label":"Gallery","country":"DE","subdivision":"DE-BY","point":{"latitude":48.137,"longitude":11.575},"basis":"ACCEPTED_DERIVED","precision":"LOCALITY","distanceKm":23.5}],"size":1,"searchAfter":{"locationId":location_id(),"distanceKm":23.5}}),
        result
    );
}

#[test]
fn should_describe_every_overview_aggregate_without_losing_integer_precision() {
    let result = output(PublicOverviewData::from(PublicCatalogueOverview {
        as_of: time::macros::datetime!(2026-10-19 12:34:56.789 UTC),
        sources_indexed: u64::MAX,
        live_objects: u64::MAX,
        countries: 7,
        languages: 5,
        new_objects_last_7_days: 89,
    }));
    assert_eq!(
        json!({"asOf":"2026-10-19T12:34:56.789Z","sourcesIndexed":u64::MAX,"liveObjects":u64::MAX,"countries":7,"languages":5,"newObjectsLast7Days":89}),
        result
    );
}
