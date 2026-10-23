//! Public listing shipping read projections: the agreed collection/detail examples, bounded
//! collection summaries, recursive omission, display conversion and disclosure limits.
mod shipping_fixture;

use aura_historia_contracts::shipping::listing::{
    ListingShippingDetailData, ListingShippingProjection, ListingShippingSummaryData,
    PersonalizedShippingDetailData, PersonalizedShippingSummaryData,
};
use geo::{CountryCode, GeographicDescription, reference::ReferenceRelease};
use money::{Currency, Price};
use product_listing_service::shipping::{
    AdvertisedShipping, DestinationAttribution, PersonalizedShipping, ShippingDisplay,
    ShippingPricePreview,
};
use schemars::{JsonSchema, generate::SchemaSettings};
use serde::Serialize;
use serde_json::{Value, json};
use shipping_core::{
    assessment::{EffectiveDeliveryTerms, assess},
    coverage::{
        CoverageCompleteness, DeliveryArea, ReferenceCountry, ShippingCoverage, ShippingZone,
    },
    inputs::{Applicability, AssessmentInput},
    pricing::{ChargeBasis, ShippingPrice, ShippingRate},
    selection::{ListingDeliveryConfiguration, ProfileSelection},
    terms::{
        CollectionDependency, CollectionPolicy, DeliverySite, DeliveryTerms, ServiceFeature,
        ShippingEligibility, ShippingPolicy, ShippingService, ShippingServiceCode,
        ShippingServiceTerms,
    },
    time::CollectionArrangement,
};
use std::sync::LazyLock;
use time::OffsetDateTime;

const RELEASE: ReferenceRelease = ReferenceRelease::IsoCodes4_20_1;

fn fixture(name: &str) -> Value {
    let path = format!(
        "{}/tests/fixtures/listing-shipping/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Serializes a value and proves both its generated MCP output schema and the REST OpenAPI
/// component accept the actual JSON.
fn output<T: Serialize + JsonSchema>(value: &T) -> Value {
    let serialized = serde_json::to_value(value).unwrap();
    for validator in validators::<T>() {
        let errors: Vec<_> = validator
            .iter_errors(&serialized)
            .map(|e| e.to_string())
            .collect();
        assert!(errors.is_empty(), "{serialized}: {errors:?}");
    }
    assert_no_nulls(&serialized);
    serialized
}
/// The generated schema and the hand-written OpenAPI component of the same name.
fn validators<T: JsonSchema>() -> [jsonschema::Validator; 2] {
    let generated = SchemaSettings::default()
        .for_serialize()
        .into_generator()
        .into_root_schema_for::<T>();
    let mut openapi = OPENAPI.clone();
    openapi["$ref"] = json!(format!("#/components/schemas/{}", T::schema_name()));
    [generated.as_value(), &openapi].map(|schema| jsonschema::validator_for(schema).unwrap())
}
static OPENAPI: LazyLock<Value> = LazyLock::new(|| {
    let swagger: Value = yaml_serde::from_str(include_str!("../../../docs/swagger.yaml")).unwrap();
    json!({"components": swagger["components"]})
});
fn assert_no_nulls(value: &Value) {
    match value {
        Value::Null => panic!("public shipping reads omit absent values"),
        Value::Array(values) => values.iter().for_each(assert_no_nulls),
        Value::Object(members) => members.values().for_each(assert_no_nulls),
        _ => {}
    }
}

fn eur(minor: u64) -> Price {
    Price::new(minor.into(), Currency::Eur)
}
fn germany() -> GeographicDescription {
    GeographicDescription::new_in_release(RELEASE, None, Some(CountryCode::DEU), None, None)
        .unwrap()
        .unwrap()
}
fn effective(terms: DeliveryTerms) -> EffectiveDeliveryTerms {
    let configuration = ListingDeliveryConfiguration::new(
        shipping_core::fixtures::gallery().source_id,
        ProfileSelection::inline(terms),
        Default::default(),
        Default::default(),
    )
    .unwrap();
    EffectiveDeliveryTerms::resolve(&configuration, None, &[])
}
fn pickup_by_appointment() -> CollectionPolicy {
    CollectionPolicy::new(
        Applicability::default(),
        CollectionDependency::Independent,
        Some(ShippingEligibility::Offered),
        None,
        Some(CollectionArrangement::ByAppointment),
    )
}
/// The agreed example: insured parcel shipping to Germany at EUR 12 per item, with
/// independent pickup by appointment.
fn insured_parcel(dispatch_origin: Option<DeliverySite>) -> EffectiveDeliveryTerms {
    let parcel = ShippingService::new(
        ShippingServiceCode::new("PARCEL".to_owned()).unwrap(),
        ShippingServiceTerms {
            eligibility: Some(ShippingEligibility::Offered),
            rate: Some(ShippingRate::new(
                ShippingPrice::Fixed(eur(1_200)),
                Some(ChargeBasis::PerItem),
                None,
                Default::default(),
            )),
            features: [ServiceFeature::Insured].into(),
            ..Default::default()
        },
    );
    let zone = ShippingZone::new(
        vec![DeliveryArea::Country(
            ReferenceCountry::new("DE", RELEASE).unwrap(),
        )],
        Applicability::default(),
        vec![parcel],
    )
    .unwrap();
    effective(DeliveryTerms::new(
        Some(ShippingPolicy::coverage(
            ShippingCoverage::new(
                RELEASE,
                CoverageCompleteness::Partial,
                Applicability::default(),
                vec![],
                vec![zone],
                dispatch_origin,
            )
            .unwrap(),
        )),
        Some(pickup_by_appointment()),
    ))
}

fn advertised(effective: &EffectiveDeliveryTerms, currency: Currency) -> AdvertisedShipping {
    AdvertisedShipping {
        assessment: assess(effective, &AssessmentInput::default(), &[]),
        display: Some(shipping_fixture::display(effective, currency)),
        effective: effective.clone(),
        geographic_valid_until: None,
        evaluated_at: OffsetDateTime::UNIX_EPOCH,
    }
}
/// Mirrors enrichment: the display covers the assessed zone, the preview its selected rate.
fn personalized(
    effective: &EffectiveDeliveryTerms,
    destination: GeographicDescription,
    currency: Currency,
    preview: Option<(&str, usize)>,
) -> PersonalizedShipping {
    let assessment = assess(
        effective,
        &AssessmentInput {
            destination: Some(destination.clone()),
            ..Default::default()
        },
        &[],
    );
    let display = shipping_fixture::display(effective, currency);
    let zone = assessment.selected_zone;
    let display = zone.map(|zone| ShippingDisplay {
        rates: display
            .rates
            .iter()
            .filter(|r| r.zone_index == zone)
            .cloned()
            .collect(),
        ..display.clone()
    });
    let price_preview = preview.map(|(code, index)| {
        let service = &assessment.services[index].service;
        assert_eq!(code, service.code().as_str());
        let rate = service.rate().unwrap();
        ShippingPricePreview {
            service: service.code().clone(),
            basis: rate.basis().unwrap(),
            price: rate.price(),
            display_price: display.as_ref().and_then(|d| d.price(zone.unwrap(), index)),
        }
    });
    PersonalizedShipping {
        assessment,
        destination,
        attribution: None,
        display,
        price_preview,
    }
}
fn summary(shipping: AdvertisedShipping) -> Value {
    output(&ListingShippingSummaryData::project(shipping).unwrap())
}
fn detail(shipping: AdvertisedShipping) -> Value {
    output(&ListingShippingDetailData::project(shipping).unwrap())
}
fn personalized_summary(shipping: PersonalizedShipping) -> Value {
    output(&PersonalizedShippingSummaryData::project(shipping).unwrap())
}
fn personalized_detail(shipping: PersonalizedShipping) -> Value {
    output(&PersonalizedShippingDetailData::project(shipping).unwrap())
}

#[test]
fn should_serialize_the_agreed_collection_and_detail_examples() {
    let terms = insured_parcel(None);
    let item = |shipping| json!({"item": {"shipping": shipping}});
    let personal = |shipping, private| json!({"item": {"shipping": shipping}, "userState": {"shipping": private}});
    let private = || personalized(&terms, germany(), Currency::Eur, Some(("PARCEL", 0)));
    assert_eq!(
        fixture("anonymous-collection"),
        item(summary(advertised(&terms, Currency::Eur)))
    );
    assert_eq!(
        fixture("personalized-collection"),
        personal(
            summary(advertised(&terms, Currency::Eur)),
            personalized_summary(private())
        )
    );
    assert_eq!(
        fixture("anonymous-detail"),
        item(detail(advertised(&terms, Currency::Eur)))
    );
    assert_eq!(
        fixture("personalized-detail"),
        personal(
            detail(advertised(&terms, Currency::Eur)),
            personalized_detail(private())
        )
    );
}

#[test]
fn should_keep_collection_summaries_bounded_as_unrelated_zones_grow() {
    let small = shipping_fixture::large_policy(8);
    let large = shipping_fixture::large_policy(38);
    let collection = |policy: &shipping_fixture::LargePolicy| {
        json!([
            summary(advertised(&policy.effective, Currency::Usd)),
            personalized_summary(personalized(
                &policy.effective,
                policy.destination.clone(),
                Currency::Usd,
                Some(("PARCEL", 0)),
            )),
        ])
    };
    assert_eq!(collection(&small), collection(&large));
    let text = collection(&large).to_string();
    for internal in [
        "terms",
        "zones",
        "zoneIndex",
        "serviceIndex",
        "selectedZone",
        "FREIGHT",
    ] {
        assert!(!text.contains(internal), "{internal} in {text}");
    }
    assert!(
        detail(advertised(&large.effective, Currency::Usd))
            .to_string()
            .len()
            > 2 * detail(advertised(&small.effective, Currency::Usd))
                .to_string()
                .len()
    );
}

#[test]
fn should_distinguish_prohibition_unknown_eligibility_and_confirmation() {
    let prohibited = effective(DeliveryTerms::new(
        Some(ShippingPolicy::Prohibited),
        Some(pickup_by_appointment()),
    ));
    assert_eq!(
        json!({"eligibility": "NOT_OFFERED", "collection": {"eligibility": "OFFERED"}}),
        summary(advertised(&prohibited, Currency::Eur))
    );
    assert_eq!(
        json!({"kind": "PROHIBITED"}),
        detail(advertised(&prohibited, Currency::Eur))["terms"]["shipping"]
    );
    let unasserted = effective(DeliveryTerms::default());
    assert!(
        ListingShippingSummaryData::project(advertised(&unasserted, Currency::Eur)).is_none(),
        "unasserted shipping omits the summary instead of asserting no shipping"
    );
    assert!(ListingShippingDetailData::project(advertised(&unasserted, Currency::Eur)).is_none());
    // Unresolvable dependencies leave no terms to show, but the explanation remains.
    let gallery = shipping_core::fixtures::gallery();
    let unresolved = EffectiveDeliveryTerms::resolve(
        &gallery.painting,
        Some(&gallery.source_default),
        &[gallery.profile.clone(), gallery.profile],
    );
    let explained = json!({
        "explanations": [{"code": "UNRESOLVED_TERMS"}],
        "collection": {"explanations": [{"code": "UNRESOLVED_TERMS"}]}
    });
    assert_eq!(explained, summary(advertised(&unresolved, Currency::Eur)));
    assert_eq!(
        json!({"explanations": [{"code": "UNRESOLVED_TERMS"}]}),
        detail(advertised(&unresolved, Currency::Eur))
    );

    let policy = shipping_fixture::large_policy(8);
    let international =
        GeographicDescription::new_in_release(RELEASE, None, Some(CountryCode::USA), None, None)
            .unwrap()
            .unwrap();
    assert_eq!(
        json!({
            "eligibility": "SUBJECT_TO_CONFIRMATION",
            "pricePreview": {
                "service": "INTERNATIONAL",
                "basis": "PER_ITEM",
                "price": {"kind": "ON_REQUEST", "currency": "EUR"}
            },
            "collection": {"eligibility": "OFFERED"}
        }),
        {
            let mut shipping = personalized(&policy.effective, international, Currency::Usd, None);
            // Enrichment previews no basis-less rate; with a basis, on-request stays explicit.
            shipping.price_preview = Some(ShippingPricePreview {
                service: ShippingServiceCode::new("INTERNATIONAL".to_owned()).unwrap(),
                basis: ChargeBasis::PerItem,
                price: ShippingPrice::OnRequest(Some(Currency::Eur)),
                display_price: None,
            });
            personalized_summary(shipping)
        }
    );
    let uncovered =
        GeographicDescription::new_in_release(RELEASE, None, Some(CountryCode::FIN), None, None)
            .unwrap()
            .unwrap();
    assert_eq!(
        json!({
            "explanations": [{"code": "INCOMPLETE_COVERAGE"}],
            "collection": {"eligibility": "OFFERED"}
        }),
        personalized_summary(personalized(
            &policy.effective,
            uncovered,
            Currency::Eur,
            None
        ))
    );
}

#[test]
fn should_explain_conditional_and_unresolved_options_without_inventing_charges() {
    let policy = shipping_fixture::large_policy(8);
    let mut shipping = personalized(
        &policy.effective,
        policy.destination.clone(),
        Currency::Eur,
        Some(("PARCEL", 0)),
    );
    shipping.attribution = Some(DestinationAttribution {
        provider: "fixture".into(),
        attribution_url: "https://provider.example/attribution".into(),
    });
    let value = personalized_detail(shipping);
    assert_eq!(
        json!({"country": "DE", "referenceRelease": "iso-codes-4.20.1"}),
        value["destination"]
    );
    assert_eq!("fixture", value["attribution"]["provider"]);
    assert_eq!("OFFERED", value["eligibility"]);
    let options = value["options"].as_array().unwrap();
    assert_eq!(3, options.len());
    assert_eq!(
        json!({"code": "QUALIFYING_AMOUNT_REQUIRED", "scope": "SELLER_ORDER"}),
        options[1]["explanations"][0]
    );
    assert_eq!("FREE", options[1]["rate"]["price"]["kind"]);
    assert!(options[1].get("eligibility").is_none());
    assert_eq!(
        json!([{"code": "UNRESOLVED_TERMS"}]),
        options[2]["explanations"]
    );
    assert!(options.iter().all(|option| option.get("charge").is_none()));
    assert_eq!(
        json!({"service": "PARCEL", "basis": "PER_ITEM", "price": {"kind": "FIXED", "amount": {"amount_minor": 1200, "currency": "EUR"}}}),
        personalized_summary(personalized(
            &policy.effective,
            policy.destination.clone(),
            Currency::Eur,
            Some(("PARCEL", 0)),
        ))["pricePreview"]
    );
}

#[test]
fn should_convert_only_amounts_in_another_currency_and_share_one_valuation() {
    let policy = shipping_fixture::large_policy(8);
    let same = detail(advertised(&policy.effective, Currency::Eur));
    assert!(same.get("display").is_none());
    assert!(!same.to_string().contains("display_price"));

    let value = detail(advertised(&policy.effective, Currency::Usd));
    assert_eq!(
        json!({"currency": "USD", "fxRateId": "fx_01h455vb4pex5vy7enb1p677v4", "capturedAt": "2026-10-23T06:00:00Z"}),
        value["display"]
    );
    let zones = &value["terms"]["shipping"]["coverage"]["zones"];
    let parcel = &zones[0]["services"][0]["rate"];
    assert_eq!(
        json!({"amount_minor": 1200, "currency": "EUR"}),
        parcel["price"]["amount"]
    );
    assert_eq!(
        json!({"kind": "FIXED", "amount": {"amount_minor": 1320, "currency": "USD"}}),
        parcel["display_price"]
    );
    assert_eq!(
        "FROM",
        zones[1]["services"][1]["rate"]["display_price"]["kind"]
    );
    let free = &zones[0]["services"][1]["rate"];
    assert_eq!(json!({"kind": "FREE", "currency": "EUR"}), free["price"]);
    assert!(free.get("display_price").is_none());
    let on_request = &zones[9]["services"][0]["rate"];
    assert_eq!(
        json!({"price": {"kind": "ON_REQUEST", "currency": "EUR"}}),
        *on_request
    );

    let preview = personalized_summary(personalized(
        &policy.effective,
        policy.destination.clone(),
        Currency::Usd,
        Some(("PARCEL", 0)),
    ));
    assert_eq!(
        json!({"kind": "FIXED", "amount": {"amount_minor": 1320, "currency": "USD"}}),
        preview["pricePreview"]["price"]
    );
    assert_eq!(value["display"], preview["display"]);
    let options = personalized_detail(personalized(
        &policy.effective,
        policy.destination.clone(),
        Currency::Usd,
        None,
    ));
    assert_eq!(value["display"], options["display"]);
    assert_eq!(
        parcel["display_price"],
        options["options"][0]["rate"]["display_price"]
    );
}

#[test]
fn should_never_disclose_dispatch_or_collection_sites() {
    let site = DeliverySite::PartyLocation(party_core::party_location_id::PartyLocationId::new());
    let mut terms = insured_parcel(Some(site.clone()))
        .terms()
        .clone()
        .shipping()
        .cloned();
    let collection = CollectionPolicy::new(
        Applicability::default(),
        CollectionDependency::Independent,
        Some(ShippingEligibility::Offered),
        Some(site),
        None,
    );
    let value = detail(advertised(
        &effective(DeliveryTerms::new(terms.take(), Some(collection))),
        Currency::Eur,
    ));
    let text = value.to_string();
    assert!(!text.contains("dispatch_origin") && !text.contains("site"));
    assert!(!text.contains("ploc_"));
}

#[test]
fn should_describe_omission_rather_than_null_in_output_schemas() {
    for summaries in validators::<ListingShippingSummaryData>() {
        assert!(summaries.is_valid(&json!({"requiredInputs": ["DESTINATION"]})));
        for invalid in [
            json!({}),
            json!({"eligibility": null}),
            json!({"requiredInputs": []}),
            json!({"requiredInputs": ["ORIGIN_COUNTRY"]}),
            json!({"explanations": [{"code": "QUALIFYING_AMOUNT_REQUIRED"}]}),
            json!({"collection": null}),
            json!({"collection": {}}),
        ] {
            assert!(!summaries.is_valid(&invalid), "{invalid}");
        }
    }
    for details in validators::<ListingShippingDetailData>() {
        assert!(details.is_valid(&json!({"explanations": [{"code": "UNRESOLVED_TERMS"}]})));
        assert!(!details.is_valid(&json!({})), "empty details are omitted");
        assert!(!details.is_valid(&json!({"terms": {"shipping": null}})));
        assert!(!details.is_valid(&json!({"terms": {}})));
    }
    for personalized in validators::<PersonalizedShippingSummaryData>() {
        assert!(!personalized.is_valid(&json!({})));
    }
}

/// Records decoded JSON sizes of a repeated large-policy collection. Run explicitly with
/// `--ignored --nocapture`; compressed transfer size and latency are not measured here.
#[test]
#[ignore]
fn measure_representative_listing_shipping_sizes() {
    for regions in [8, 38] {
        let policy = shipping_fixture::large_policy(regions);
        for currency in [Currency::Eur, Currency::Usd] {
            let public = summary(advertised(&policy.effective, currency));
            let private = personalized_summary(personalized(
                &policy.effective,
                policy.destination.clone(),
                currency,
                Some(("PARCEL", 0)),
            ));
            let anonymous: Vec<_> = (0..21)
                .map(|_| json!({"item": {"shipping": public}}))
                .collect();
            let personal: Vec<_> = (0..21)
                .map(|_| json!({"item": {"shipping": public}, "userState": {"shipping": private}}))
                .collect();
            let item = detail(advertised(&policy.effective, currency));
            let user = personalized_detail(personalized(
                &policy.effective,
                policy.destination.clone(),
                currency,
                None,
            ));
            let size = |value: &Value| serde_json::to_vec(value).unwrap().len();
            println!(
                "zones={} currency={} collection21_anon={} collection21_personal={} detail_anon={} detail_personal={}",
                regions + 2,
                currency.as_str(),
                size(&Value::Array(anonymous)),
                size(&Value::Array(personal)),
                size(&json!({"item": {"shipping": item}})),
                size(&json!({"item": {"shipping": item}, "userState": {"shipping": user}})),
            );
        }
    }
}
