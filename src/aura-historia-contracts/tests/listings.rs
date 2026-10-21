use aura_historia_contracts::{
    product_listings::{
        presentation::ProductListingPresentationData,
        search::{ProductListingSearchData, ProductListingSearchOptionsData},
    },
    shipping::{listing::AdvertisedShippingData, terms},
};
use localization::Language;
use money::Currency;
use schemars::{JsonSchema, generate::SchemaSettings};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use shipping_core::{assessment::EffectiveDeliveryTerms, fixtures};
use time::OffsetDateTime;

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
#[test]
fn should_preserve_unsigned_shipping_revisions_required_nulls_and_public_terms_redaction() {
    let fixture = fixtures::gallery();
    let source = fixture.source_id;
    let effective = EffectiveDeliveryTerms::resolve(
        &fixture.painting,
        Some(&fixture.source_default),
        std::slice::from_ref(&fixture.profile),
    );
    let advertised =
        AdvertisedShippingData(product_listing_service::shipping::AdvertisedShipping {
            display: None,
            effective,
            shipping_reasons: vec![],
            collection_reasons: vec![],
            listing_revision: u64::MAX,
            source_revision: u64::MAX,
            default_revision: None,
            profile: None,
            listing_evidence: None,
            geographic_valid_until: None,
            evaluated_at: OffsetDateTime::UNIX_EPOCH,
            notices: vec![],
        });
    let value = serde_json::to_value(&advertised).unwrap();
    assert_eq!(u64::MAX, value["listingRevision"]);
    assert_eq!(u64::MAX, value["sourceRevision"]);
    for key in ["display", "defaultRevision", "profile", "listingEvidence"] {
        assert!(value[key].is_null());
        assert!(value.as_object().unwrap().contains_key(key));
    }
    assert!(value["terms"]["shipping"]["coverage"]["dispatch_origin"].is_null());
    assert!(value["terms"]["collection"]["site"].is_null());
    let schema = serde_json::to_value(
        SchemaSettings::default()
            .for_serialize()
            .into_generator()
            .into_root_schema_for::<AdvertisedShippingData>(),
    )
    .unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    assert!(
        validator.is_valid(&value),
        "{}",
        validator
            .iter_errors(&value)
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    );
    for key in ["display", "defaultRevision", "profile", "listingEvidence"] {
        let mut invalid = value.clone();
        invalid.as_object_mut().unwrap().remove(key);
        assert!(!validator.is_valid(&invalid));
    }
    let decoded = terms::decode(value["terms"].clone(), source).unwrap();
    assert_eq!(value["terms"], terms::encode(&decoded).unwrap());
    let mut populated = advertised;
    populated.0.default_revision = Some(u64::MAX);
    populated.0.profile = Some(shipping_service::assessment::ProfileAssessmentBasis {
        id: shipping_core::profile::ShippingProfileId::new(),
        terms_revision: u64::MAX,
        management_revision: u64::MAX,
        lifecycle: shipping_core::profile::ShippingProfileLifecycle::Active,
        evidence: shipping_service::assessment::AssertionEvidence {
            verified_at: OffsetDateTime::UNIX_EPOCH,
        },
    });
    populated.0.listing_evidence = Some(shipping_service::assessment::AssertionEvidence {
        verified_at: OffsetDateTime::UNIX_EPOCH,
    });
    populated.0.shipping_reasons = vec![
        shipping_core::assessment::AssessmentReason::EarlierZoneUndetermined { index: 0 },
        shipping_core::assessment::AssessmentReason::MissingInput(
            shipping_core::assessment::MissingInput::QualifyingAmount(
                shipping_core::pricing::QualifyingScope::Item,
            ),
        ),
    ];
    populated.0.display = Some(product_listing_service::shipping::ShippingDisplay {
        currency: Currency::Eur,
        fx_rate_id: fxrate_core::FxRateId::new(),
        captured_at: OffsetDateTime::UNIX_EPOCH,
        rates: vec![product_listing_service::shipping::ConvertedShippingRate {
            zone_index: 0,
            service_index: 0,
            price: shipping_core::pricing::ShippingPrice::Fixed(money::Price::new(
                u64::MAX.into(),
                Currency::Eur,
            )),
            basis: Some(shipping_core::pricing::ChargeBasis::PerItem),
        }],
    });
    let value = serde_json::to_value(&populated).unwrap();
    assert!(
        validator.is_valid(&value),
        "{}",
        validator
            .iter_errors(&value)
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert_eq!(u64::MAX, value["profile"]["managementRevision"]);
    assert_eq!(
        u64::MAX,
        value["display"]["rates"][0]["price"]["amount"]["amount_minor"]
    );
    let input = fixture.assessment_input();
    let assessment = shipping_core::assessment::assess(&populated.0.effective, &input, &[]);
    let personalized = aura_historia_contracts::shipping::listing::PersonalizedShippingData(
        product_listing_service::shipping::PersonalizedShipping {
            assessment,
            destination: input.destination.unwrap(),
            evaluated_at: OffsetDateTime::UNIX_EPOCH,
            attribution: Some(product_listing_service::shipping::DestinationAttribution {
                provider: "fixture".into(),
                attribution_url: "https://provider.example/attribution".into(),
            }),
        },
    );
    let value = serde_json::to_value(&personalized).unwrap();
    let schema=serde_json::to_value(SchemaSettings::default().for_serialize().into_generator().into_root_schema_for::<aura_historia_contracts::shipping::listing::PersonalizedShippingData>()).unwrap();
    assert!(jsonschema::is_valid(&schema, &value));
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
