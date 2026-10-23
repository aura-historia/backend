use aura_historia_contracts::{
    product_listings::{
        presentation::ProductListingPresentationData,
        search::{ProductListingSearchData, ProductListingSearchOptionsData},
    },
    shipping::terms,
};
use localization::Language;
use money::Currency;
use schemars::{JsonSchema, generate::SchemaSettings};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use shipping_core::fixtures;

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/listing-search.json")).unwrap()
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
fn agreement<T: JsonSchema + DeserializeOwned>(samples: impl IntoIterator<Item = (Value, bool)>) {
    let schema = input_schema::<T>();
    for (input, valid) in samples {
        assert_eq!(
            valid,
            serde_json::from_value::<T>(input.clone()).is_ok(),
            "Serde {input}"
        );
        assert_eq!(
            valid,
            jsonschema::is_valid(&schema, &input),
            "schema {input}"
        );
    }
}
#[test]
fn should_preserve_optional_search_options_when_values_are_omitted() {
    agreement::<ProductListingSearchOptionsData>([
        (json!({}), true),
        (json!({"order":null}), true),
        (json!({"sort":"updated"}), true),
        (json!({"size":100}), true),
    ]);
    let options: ProductListingSearchOptionsData = serde_json::from_value(json!({})).unwrap();
    assert!(options.order.is_none());
    assert!(options.sort.is_none());
    assert!(options.size.is_none());
    assert!(options.search_after.is_none());
}

#[test]
fn should_preserve_omitted_bounds_in_each_shared_search_date_range() {
    for field in [
        "created",
        "updated",
        "lotBiddingOpens",
        "lotScheduledCloses",
    ] {
        for range in [
            json!({}),
            json!({"min":"2026-01-01T00:00:00Z"}),
            json!({"max":"2026-01-01T00:00:00Z"}),
            json!({"min":null}),
            json!({"max":null}),
        ] {
            let mut input = json!({});
            input[field] = range;
            agreement::<ProductListingSearchData>([(input, true)]);
        }
    }
}

#[test]
fn should_preserve_complete_search_values_through_shared_constructors() {
    let fixture = fixture();
    let data: ProductListingSearchData = serde_json::from_value(fixture.clone()).unwrap();
    assert!(jsonschema::is_valid(
        &input_schema::<ProductListingSearchData>(),
        &fixture
    ));
    assert!(jsonschema::is_valid(
        &input_schema::<ProductListingSearchOptionsData>(),
        &fixture
    ));
    let request = serde_json::from_value::<ProductListingSearchOptionsData>(fixture.clone())
        .unwrap()
        .into_request(data)
        .unwrap();
    let search = request.search;
    assert_eq!(Language::De, search.language);
    assert_eq!(Currency::Jpy, search.currency);
    assert_eq!(
        vec!["cabinet", "oak"],
        search
            .product_listing_query
            .iter()
            .map(|q| q.as_ref())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        fixture["enhancedSearchDescription"].as_str().unwrap(),
        search.enhanced_search_description.unwrap().as_ref()
    );
    assert_eq!(1, search.exclude_product_listing_id_query.len());
    assert_eq!(1, search.listing_source_id_query.len());
    assert_eq!(1, search.exclude_listing_source_id_query.len());
    assert_eq!(1, search.auction_id_query.len());
    assert_eq!(
        Some(u64::MAX),
        search.price_query.unwrap().max.map(u64::from)
    );
    let availability = search.availability_query.unwrap();
    assert_eq!(2, availability.any_of.len());
    assert_eq!(2, availability.orderability.len());
    assert!(availability.include_unspecified);
    for range in [
        search.created_query,
        search.updated_query,
        search.lot_bidding_opens_query,
        search.lot_scheduled_closes_query,
    ] {
        let range = range.unwrap();
        assert!(range.min.is_some());
        assert!(range.max.is_some());
    }
    let geography = serde_json::to_value(
        aura_historia_contracts::geographic_search::GeographicSearchData::from(
            search.geography.unwrap(),
        ),
    )
    .unwrap();
    assert_eq!(fixture["geography"], geography);
    let shipping = search.shipping.unwrap();
    assert_eq!("STANDARD", shipping.service().unwrap().as_str());
    assert_eq!(
        Some(i64::MAX as u64),
        shipping
            .maximum_per_item()
            .map(|p| u64::from(p.monetary_amount))
    );
    assert!(shipping.collection_offered());
    assert!(shipping.fx_converted());
    assert_eq!(100, request.cursor.unwrap().size);
    assert_eq!("asc", request.sort.unwrap().order.as_str());
}
#[test]
fn should_match_schema_and_serde_for_ranges_enum_sets_and_presentation_aliases() {
    agreement::<ProductListingSearchData>([
        (json!({}), true),
        (json!({"price":{"min":0,"max":u64::MAX}}), true),
        (
            json!({"price":{"min":null},"availability":null,"orderability":null}),
            true,
        ),
        (
            json!({"availability":["AVAILABLE","AVAILABLE"],"orderability":[]}),
            true,
        ),
        (
            json!({"created":{"min":null,"max":"2026-01-01T00:00:00Z"}}),
            true,
        ),
        (json!({"created":null}), false),
        (json!({"price":{"max":-1}}), false),
        (json!({"price":{"max":"18446744073709551615"}}), false),
        (json!({"availability":["UNKNOWN"]}), false),
        (json!({"orderability":["UNKNOWN"]}), false),
        (json!({"language":"xx"}), false),
        (json!({"currency":"eur"}), false),
        (json!({"productQuery":[""]}), false),
    ]);
    agreement::<ProductListingPresentationData>([
        (json!({}), true),
        (json!({"language":"de-CH","currency":"USD"}), true),
        (json!({"language":"DE"}), false),
        (json!({"currency":"XXX"}), false),
    ]);
}
#[test]
fn should_normalize_explicit_no_shipping_destination_without_losing_other_predicates() {
    let data: ProductListingSearchData =
        serde_json::from_value(json!({"shipping":{"destination":{"kind":"NONE"}}})).unwrap();
    assert!(
        ProductListingSearchOptionsData::default()
            .into_request(data)
            .unwrap()
            .search
            .shipping
            .is_none()
    );
    let data: ProductListingSearchData = serde_json::from_value(
        json!({"shipping":{"destination":{"kind":"NONE"},"collectionOffered":true}}),
    )
    .unwrap();
    assert!(
        ProductListingSearchOptionsData::default()
            .into_request(data)
            .unwrap()
            .search
            .shipping
            .unwrap()
            .collection_offered()
    );
}
#[derive(Deserialize, JsonSchema)]
struct TermsSchema {
    #[schemars(schema_with = "terms_schema")]
    terms: Value,
}
fn terms_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    shipping_json::terms_schema(
        generator,
        aura_historia_contracts::wire::schema::object_id(party_core::party_id::PartyId::PREFIX),
        aura_historia_contracts::wire::schema::object_id(
            party_core::party_location_id::PartyLocationId::PREFIX,
        ),
    )
}
#[test]
fn should_describe_terms_required_null_assertions_and_canonical_codes() {
    let source = listing_source_core::ListingSourceId::new();
    let terms = terms::encode(&fixtures::gallery_terms(u64::MAX)).unwrap();
    let input = json!({"terms":terms});
    let decoded: TermsSchema = serde_json::from_value(input.clone()).unwrap();
    assert_eq!(input["terms"], decoded.terms);
    let schema = input_schema::<TermsSchema>();
    assert!(jsonschema::is_valid(&schema, &input));
    for field in ["shipping", "collection"] {
        let mut value = input.clone();
        value["terms"].as_object_mut().unwrap().remove(field);
        assert!(!jsonschema::is_valid(&schema, &value));
        assert!(terms::decode(value["terms"].clone(), source).is_err());
    }
    let party = party_core::party_id::PartyId::new();
    let mut referenced = input.clone();
    referenced["terms"]["shipping"]["coverage"]["zones"][0]["services"][0]["arranger"] =
        json!({"kind":"PARTY","party_id":party});
    assert!(jsonschema::is_valid(&schema, &referenced));
    assert!(terms::decode(referenced["terms"].clone(), source).is_ok());
    referenced["terms"]["shipping"]["coverage"]["zones"][0]["services"][0]["arranger"]["party_id"] =
        json!(party.as_uuid().to_string());
    assert!(!jsonschema::is_valid(&schema, &referenced));
    assert!(terms::decode(referenced["terms"].clone(), source).is_err());
    let mut invalid = input.clone();
    invalid["terms"]["shipping"]["coverage"]["completeness"] = json!("complete");
    assert!(!jsonschema::is_valid(&schema, &invalid));
    assert!(terms::decode(invalid["terms"].clone(), source).is_err());
}
