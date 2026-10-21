use application::patch_field::PatchField;
use aura_historia_contracts::{product_listings::search, saved_searches::*};
use product_listing_core::product_listing_search::ProductListingSearch;
use schemars::{JsonSchema, generate::SchemaSettings};
use search_filter_core::search_filter_state::SearchFilterState;
use search_filter_service::ports::{SearchFilterMatchView, SearchFilterView};
use serde_json::{Value, json};
use time::OffsetDateTime;
use user_core::user_id::UserId;

fn validator<T: JsonSchema>(input: bool) -> jsonschema::Validator {
    let settings = if input {
        SchemaSettings::default().for_deserialize()
    } else {
        SchemaSettings::default().for_serialize()
    };
    let schema =
        serde_json::to_value(settings.into_generator().into_root_schema_for::<T>()).unwrap();
    jsonschema::validator_for(&schema).unwrap()
}
fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/saved-search.json")).unwrap()
}

#[test]
fn should_preserve_complete_listing_filter_fixture_and_notification_defaults() {
    let fixture = fixture();
    assert!(validator::<CreateSearchFilterData>(true).is_valid(&fixture));
    let data: CreateSearchFilterData = serde_json::from_value(fixture.clone()).unwrap();
    let listing: search::ProductListingSearchData =
        serde_json::from_value(fixture["search"].clone()).unwrap();
    let command = data.into_command(UserId::new()).unwrap();
    assert_eq!(
        command.search,
        ProductListingSearch::try_from(listing).unwrap()
    );
    assert!(command.notifications);
    assert!(matches!(
        command.accept_shipping_destination,
        Some(search_filter_service::shipping_destination::AcceptShippingDestination::Preference)
    ));
    assert_eq!(
        Some(u64::MAX),
        command.search.price_query.unwrap().max.map(u64::from)
    );
}

#[test]
fn should_align_patch_schema_with_omission_null_and_atomic_availability_validation() {
    let schema = validator::<UpdateSearchFilterData>(true);
    for value in [
        json!({}),
        json!({"name":null,"notifications":null,"state":null,"search":null,"acceptShippingDestination":null}),
        json!({"search":{"geography":null,"shipping":null,"auctionId":null,"price":null,"created":null,"updated":null,"lotBiddingOpens":null,"lotScheduledCloses":null,"enhancedSearchDescription":null}}),
        json!({"search":{"availability":null,"orderability":null,"includeUnspecifiedAvailability":null}}),
        json!({"search":{"availability":[],"orderability":[],"includeUnspecifiedAvailability":false}}),
    ] {
        assert!(schema.is_valid(&value), "{value}");
        let data: UpdateSearchFilterData = serde_json::from_value(value).unwrap();
        assert!(data.into_fields().is_ok());
    }
    for value in [
        json!({"search":{"language":null}}),
        json!({"search":{"currency":null}}),
        json!({"search":{"productQuery":null}}),
        json!({"search":{"listingSourceId":null}}),
        json!({"search":{"excludeListingSourceId":null}}),
        json!({"search":{"availability":[]}}),
        json!({"search":{"availability":null,"orderability":[],"includeUnspecifiedAvailability":false}}),
    ] {
        assert!(!schema.is_valid(&value), "{value}");
        let data: UpdateSearchFilterData = serde_json::from_value(value).unwrap();
        assert!(data.into_fields().is_err());
    }
}

#[test]
fn should_keep_feedback_boolean_null_and_omission_distinct() {
    let schema = validator::<UpdateSearchFilterMatchFeedbackData>(true);
    for (value, expected) in [
        (json!({}), PatchField::Unchanged),
        (json!({"feedback":null}), PatchField::Clear),
        (json!({"feedback":true}), PatchField::Set(true)),
        (json!({"feedback":false}), PatchField::Set(false)),
    ] {
        assert!(schema.is_valid(&value));
        assert_eq!(
            expected,
            serde_json::from_value::<UpdateSearchFilterMatchFeedbackData>(value)
                .unwrap()
                .feedback()
        );
    }
}

#[test]
fn should_reject_null_date_ranges_on_creation_but_clear_them_in_patches() {
    let create_schema = validator::<CreateSearchFilterData>(true);
    let patch_schema = validator::<UpdateSearchFilterData>(true);
    let omitted = json!({"name":"Saved", "search":{}});
    assert!(create_schema.is_valid(&omitted));
    assert!(
        serde_json::from_value::<CreateSearchFilterData>(omitted)
            .unwrap()
            .into_command(Default::default())
            .is_ok()
    );
    for field in [
        "created",
        "updated",
        "lotBiddingOpens",
        "lotScheduledCloses",
    ] {
        let create = json!({"name":"Saved", "search":{field:null}});
        assert!(
            serde_json::from_value::<CreateSearchFilterData>(create.clone()).is_err(),
            "{field} creation must retain REST's object-only date range codec"
        );
        assert!(!create_schema.is_valid(&create), "{field}");

        let patch = json!({"search":{field:null}});
        assert!(patch_schema.is_valid(&patch), "{field}");
        let patch = serde_json::from_value::<UpdateSearchFilterData>(patch)
            .unwrap()
            .into_fields()
            .unwrap()
            .3;
        let actual = match field {
            "created" => patch.created_query,
            "updated" => patch.updated_query,
            "lotBiddingOpens" => patch.lot_bidding_opens_query,
            "lotScheduledCloses" => patch.lot_scheduled_closes_query,
            _ => unreachable!(),
        };
        assert_eq!(PatchField::Clear, actual, "{field}");
    }
}

#[test]
fn should_preserve_concrete_date_and_unsigned_price_range_patches() {
    use domain_primitives::query::range_query::RangeQuery;

    let min = "2026-01-01T12:00:00.123456789+02:00";
    let max = "2026-12-31T23:59:59.987654321Z";
    let value = json!({"search":{
        "price":{"min":null,"max":u64::MAX},
        "created":{"min":min,"max":null},
        "updated":{"max":max},
        "lotBiddingOpens":{},
        "lotScheduledCloses":{"min":min,"max":max}
    }});
    assert!(validator::<UpdateSearchFilterData>(true).is_valid(&value));
    let patch = serde_json::from_value::<UpdateSearchFilterData>(value)
        .unwrap()
        .into_fields()
        .unwrap()
        .3;
    let min = OffsetDateTime::parse(min, &time::format_description::well_known::Rfc3339).unwrap();
    let max = OffsetDateTime::parse(max, &time::format_description::well_known::Rfc3339).unwrap();
    assert_eq!(
        PatchField::Set(RangeQuery {
            min: None,
            max: Some(u64::MAX.into())
        }),
        patch.price_query
    );
    assert_eq!(
        PatchField::Set(RangeQuery {
            min: Some(min),
            max: None
        }),
        patch.created_query
    );
    assert_eq!(
        PatchField::Set(RangeQuery {
            min: None,
            max: Some(max)
        }),
        patch.updated_query
    );
    assert_eq!(
        PatchField::Set(RangeQuery::default()),
        patch.lot_bidding_opens_query
    );
    assert_eq!(
        PatchField::Set(RangeQuery {
            min: Some(min),
            max: Some(max)
        }),
        patch.lot_scheduled_closes_query
    );
}

#[test]
fn should_preserve_saved_search_response_and_match_snapshot_schemas() {
    let user_id = UserId::new();
    let data: CreateSearchFilterData = serde_json::from_value(fixture()).unwrap();
    let command = data.into_command(user_id).unwrap();
    let filter_id = Default::default();
    let filter = SearchFilterView {
        user_id,
        search_filter_id: filter_id,
        name: command.name,
        notifications: command.notifications,
        state: SearchFilterState::Active,
        search: command.search,
        embedding: Some(vec![0.5]),
        created: OffsetDateTime::UNIX_EPOCH,
        updated: OffsetDateTime::UNIX_EPOCH,
    };
    let value = serde_json::to_value(SearchFilterData::from(filter)).unwrap();
    assert!(validator::<SearchFilterData>(false).is_valid(&value));
    assert_eq!(u64::MAX, value["search"]["price"]["max"]);
    assert_eq!("de", value["search"]["language"]);
    assert!(value.get("embedding").is_none());
    assert!(value.get("createdBy").is_none());
    let valuation = search_filter_core::ShippingMatchValuation {
        fx_rate_id: Default::default(),
        captured_at: OffsetDateTime::UNIX_EPOCH,
        maximum_per_item: money::Price::new(9000_u64.into(), money::Currency::Jpy),
    };
    let matched = SearchFilterMatchView {
        user_id,
        search_filter_id: filter_id,
        search_filter_name: Some("snapshot".into()),
        product_listing_id: Default::default(),
        origin_event_id: Default::default(),
        shipping_match_valuation: Some(valuation),
        feedback: Some(false),
        created: OffsetDateTime::UNIX_EPOCH,
        updated: OffsetDateTime::UNIX_EPOCH,
    };
    let value = serde_json::to_value(SearchFilterMatchData::from(matched)).unwrap();
    assert!(validator::<SearchFilterMatchData>(false).is_valid(&value));
    assert_eq!(false, value["feedback"]);
    assert_eq!(
        serde_json::to_value(
            aura_historia_contracts::shipping::listing::ShippingMatchValuationData::from(valuation)
        )
        .unwrap(),
        value["shippingMatchValuation"]
    );
}

#[test]
fn should_retain_exact_json_match_cursor_and_page_size_semantics() {
    let product_id = product_listing_core::product_listing_id::ProductListingId::new();
    let cursor = json!(["2026-10-21T12:00:00.123456789Z", product_id]);
    let parsed = parse_matches_cursor(cursor.clone()).unwrap();
    assert_eq!(cursor, matches_cursor_value(parsed).unwrap());
    for (size, expected) in [(None, 21), (Some(0), 1), (Some(u64::MAX), 100)] {
        let query: SearchFilterMatchesQuery =
            serde_json::from_value(json!({"size":size,"language":"de-CH","currency":"JPY"}))
                .unwrap();
        let request = query
            .into_request(Default::default(), Default::default(), Some(cursor.clone()))
            .unwrap();
        assert_eq!(expected, request.cursor.unwrap().size);
        assert_eq!(localization::Language::De, request.language);
        assert_eq!(money::Currency::Jpy, request.currency);
    }
    for bad in [
        json!({}),
        json!([]),
        json!(["2026-10-21T12:00:00Z"]),
        json!(["bad", product_id]),
        json!(["2026-10-21T12:00:00Z", product_id.as_uuid().to_string()]),
    ] {
        assert!(parse_matches_cursor(bad).is_err());
    }
}
