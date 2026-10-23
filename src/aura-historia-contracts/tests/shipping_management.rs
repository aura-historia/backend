use aura_historia_contracts::shipping::{imports::ImportData, management::*, terms};
use listing_source_core::ListingSourceId;
use party_core::{party_id::PartyId, party_location_id::PartyLocationId};
use schemars::{JsonSchema, generate::SchemaSettings};
use serde_json::{Value, json};
use shipping_core::fixtures::gallery_terms;

fn validator<T: JsonSchema>(output: bool) -> jsonschema::Validator {
    let settings = if output {
        SchemaSettings::default().for_serialize()
    } else {
        SchemaSettings::default().for_deserialize()
    };
    let schema =
        serde_json::to_value(settings.into_generator().into_root_schema_for::<T>()).unwrap();
    jsonschema::validator_for(&schema).unwrap()
}
fn content() -> Value {
    json!({"name":"Complete terms","attributedPartyId":null,"terms":terms::encode(&gallery_terms(8000)).unwrap()})
}
fn profile_output(source: ListingSourceId, terms: shipping_core::terms::DeliveryTerms) -> Value {
    let terms_content = shipping_core::profile::ShippingProfileContent {
        name: shipping_core::profile::ShippingProfileName::new("Complete terms".to_owned())
            .unwrap(),
        attributed_party: None,
        terms,
    };
    let profile = shipping_core::profile::ShippingProfile::create(
        shipping_core::profile::ShippingProfileId::new(),
        source,
        terms_content,
    )
    .unwrap();
    serde_json::to_value(
        ProfileData::try_from(shipping_service::use_cases::ProfileView {
            id: profile.id(),
            source_id: profile.source_id(),
            management_revision: 1u64.try_into().unwrap(),
            revision: profile.revision(),
            lifecycle: profile.lifecycle(),
            content: profile.content().clone(),
        })
        .unwrap(),
    )
    .unwrap()
}
fn assert_terms(value: Value) {
    let input = json!({"creationKey":"complete","content":{"name":"Complete terms","attributedPartyId":null,"terms":value},"evidence":[]});
    let schema = validator::<CreateData>(false);
    assert!(
        schema.is_valid(&input),
        "{input}: {:?}",
        schema
            .iter_errors(&input)
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
    );
    let data: CreateData = serde_json::from_value(input.clone()).unwrap();
    let command = data.into_command(ListingSourceId::new()).unwrap();
    assert_eq!(
        input["content"]["terms"],
        terms::encode(&command.content.terms).unwrap()
    );
    let output = profile_output(command.source_id, command.content.terms);
    assert!(validator::<ProfileData>(true).is_valid(&output), "{output}");
    assert_eq!(input["content"]["terms"], output["content"]["terms"]);
}
#[test]
fn should_preserve_required_nullable_attribution_revisions_evidence_and_import_provenance() {
    let source = ListingSourceId::new();
    let value = json!({"creationKey":"caller-key","content":content(),"evidence":[{"reference":"https://example.test/terms","assertion":"ADVERTISED"}],"import":{"integration":{"kind":"PROVIDER","method":"SHOPIFY"},"sequence":1,"acquiredAt":"2026-10-20T01:02:03.123456Z","requestKey":"profile-create","objectKey":"profile-1","observation":{}}});
    assert!(validator::<CreateData>(false).is_valid(&value));
    let command = serde_json::from_value::<CreateData>(value.clone())
        .unwrap()
        .into_command(source)
        .unwrap();
    assert_eq!("caller-key", command.creation_key);
    assert_eq!(
        "https://example.test/terms",
        command.evidence[0].reference()
    );
    let import = command.import.unwrap();
    assert_eq!("profile-1", import.object_key.as_ref().unwrap());
    assert_eq!(
        import.acquired_at,
        import.observation.as_ref().unwrap().acquired_at
    );
    assert_eq!(
        value["import"],
        serde_json::to_value(ImportData::from_command(&import).unwrap()).unwrap()
    );
    for (path, replacement) in [
        ("/import/sequence", json!(0)),
        ("/import/sequence", json!(u64::MAX)),
        ("/import/requestKey", json!("contains spaces")),
        ("/import/integration/method", json!("PARTNER_API")),
        ("/import/observation", json!({"acquiredAt":"ignored"})),
    ] {
        let mut invalid = value.clone();
        *invalid.pointer_mut(path).unwrap() = replacement;
        assert!(!validator::<CreateData>(false).is_valid(&invalid), "{path}");
        let result = serde_json::from_value::<CreateData>(invalid);
        assert!(result.is_err() || result.unwrap().into_command(source).is_err());
    }
    for mutation in ["omit", "uuid", "wrongPrefix"] {
        let mut invalid = value.clone();
        match mutation {
            "omit" => {
                invalid["content"]
                    .as_object_mut()
                    .unwrap()
                    .remove("attributedPartyId");
            }
            "uuid" => {
                invalid["content"]["attributedPartyId"] =
                    json!(PartyId::new().into_uuid().to_string())
            }
            _ => invalid["content"]["attributedPartyId"] = json!(source),
        }
        assert!(serde_json::from_value::<CreateData>(invalid.clone()).is_err());
        assert!(!validator::<CreateData>(false).is_valid(&invalid));
    }
    for revision in [json!(0), json!(-1), json!("1"), json!(null)] {
        let invalid = json!({"expectedRevision":revision});
        assert!(serde_json::from_value::<LifecycleData>(invalid.clone()).is_err());
        assert!(!validator::<LifecycleData>(false).is_valid(&invalid));
    }
    let lifecycle = json!({"expectedRevision":u64::MAX});
    assert!(serde_json::from_value::<LifecycleData>(lifecycle.clone()).is_ok());
    assert!(validator::<LifecycleData>(false).is_valid(&lifecycle));
    let mut invalid = value;
    invalid["evidence"][0]["assertion"] = json!("\n");
    assert!(
        serde_json::from_value::<CreateData>(invalid)
            .unwrap()
            .into_command(source)
            .is_err()
    );
}
#[test]
fn should_describe_every_terms_branch_and_keep_public_references_distinct_from_storage() {
    let initial = terms::encode(&gallery_terms(8000)).unwrap();
    let applicability = json!({"class":"painting","variants":["packed"],"quantity":{"minimum":1,"maximum":3},"threshold":{"minimum":{"amount_minor":1000,"currency":"EUR"},"scope":"SELLER_ORDER"}});
    let mut full = initial.clone();
    full["shipping"]["coverage"]["applicability"] = applicability;
    full["shipping"]["coverage"]["exclusions"] = json!([
        {"kind":"DOMESTIC"},{"kind":"SUBDIVISION","code":"DE-BE","reference_release":"iso-codes-4.20.1"},
        {"kind":"POSTAL","version":"POSTAL_RULE_V1","country":"DE","reference_release":"iso-codes-4.20.1","form":{"kind":"EXACT","code":"10115"}},
        {"kind":"POSTAL","version":"POSTAL_RULE_V1","country":"DE","reference_release":"iso-codes-4.20.1","form":{"kind":"GERMAN_PREFIX","prefix":"10"}},
        {"kind":"POSTAL","version":"POSTAL_RULE_V1","country":"DE","reference_release":"iso-codes-4.20.1","form":{"kind":"GERMAN_RANGE","first":"01000","last":"01999"}},
        {"kind":"UNRESOLVED","assertion":"remote islands"}
    ]);
    let location = PartyLocationId::new();
    let party = PartyId::new();
    full["shipping"]["coverage"]["dispatch_origin"] =
        json!({"kind":"PARTY_LOCATION","party_location_id":location});
    full["collection"]["site"] = json!({"kind":"ASSERTED","geography":{"reference_release":"iso-codes-4.20.1","address_text":"Gallery","country":"DE","subdivision":"DE-BE","postal_code":"10115"},"position":{"point":{"latitude_degrees":52.5,"longitude_degrees":13.4},"precision":"PREMISES","accuracy_metres":5.0}});
    let service = &mut full["shipping"]["coverage"]["zones"][0]["services"][0];
    service["arranger"] = json!({"kind":"PARTY","party_id":party});
    service["performer"] = json!({"kind":"BUYER"});
    service["features"] = json!([
        "TRACKING",
        "INSURED",
        "WHITE_GLOVE",
        "INSTALLATION",
        "PACKING"
    ]);
    service["rate"]["qualifying_scope"] = json!("ITEM");
    service["rate"]["components"] = json!({"TRANSPORT":"INCLUDED","PACKING":"EXCLUDED","INSURANCE":"INCLUDED","HANDLING":"INCLUDED","INSTALLATION":"EXCLUDED","IMPORT_DUTY":"EXCLUDED","TAX":"INCLUDED"});
    service["lead_times"]["handling"] =
        json!({"minimum":1,"maximum":2,"unit":"DAYS","basis":"BUSINESS","trigger":"PAYMENT"});
    service["lead_times"]["transit"] =
        json!({"minimum":12,"maximum":48,"unit":"HOURS","basis":"CALENDAR","trigger":"DISPATCH"});
    service["lead_times"]["total"] =
        json!({"minimum":1,"maximum":2,"unit":"WEEKS","basis":null,"trigger":"ORDER_CONFIRMATION"});
    full["collection"]["arrangement"] = json!({"kind":"WINDOW","duration":{"minimum":1,"maximum":2,"unit":"DAYS","basis":null,"trigger":"READY_FOR_COLLECTION"}});
    assert_terms(full.clone());
    for price in [
        json!({"kind":"FREE","currency":null}),
        json!({"kind":"FREE","currency":"JPY"}),
        json!({"kind":"FROM","amount":{"amount_minor":u64::MAX,"currency":"USD"}}),
        json!({"kind":"ESTIMATED_RANGE","minimum":{"amount_minor":10,"currency":"EUR"},"maximum":{"amount_minor":100,"currency":"EUR"}}),
        json!({"kind":"EXTERNALLY_CALCULATED","currency":"GBP"}),
        json!({"kind":"ON_REQUEST","currency":null}),
    ] {
        let mut fixture = full.clone();
        fixture["shipping"]["coverage"]["zones"][0]["services"][0]["rate"]["price"] = price;
        assert_terms(fixture);
    }
    for (path, replacement) in [
        (
            "/shipping/coverage/dispatch_origin/party_location_id",
            json!(location.into_uuid().to_string()),
        ),
        (
            "/shipping/coverage/zones/0/services/0/arranger/party_id",
            json!(party.into_uuid().to_string()),
        ),
        (
            "/shipping/coverage/zones/0/services/0/rate/price/amount/currency",
            json!("ZZZ"),
        ),
        (
            "/collection/site/position/point/latitude_degrees",
            json!(91),
        ),
        ("/collection/site/geography/country", json!("ZZ")),
    ] {
        let mut invalid = full.clone();
        *invalid.pointer_mut(path).unwrap() = replacement;
        let value = json!({"creationKey":"bad","content":{"name":"Bad","attributedPartyId":null,"terms":invalid},"evidence":[]});
        assert!(!validator::<CreateData>(false).is_valid(&value), "{path}");
        assert!(
            serde_json::from_value::<CreateData>(value)
                .unwrap()
                .into_command(ListingSourceId::new())
                .is_err(),
            "{path}"
        );
    }
    assert_terms(
        json!({"schema":"SHIP_TERMS_V1","shipping":{"kind":"PROHIBITED"},"collection":null}),
    );
    assert_terms(json!({"schema":"SHIP_TERMS_V1","shipping":null,"collection":null}));
    let mut anywhere = initial.clone();
    anywhere["shipping"]["coverage"]["zones"][0]["areas"] = json!([{"kind":"ANYWHERE"}]);
    anywhere["shipping"]["coverage"]["zones"][0]["services"][0]["arranger"] =
        json!({"kind":"LISTING_SOURCE"});
    anywhere["collection"]["arrangement"] = json!({"kind":"CONTACT_TO_ARRANGE"});
    assert_terms(anywhere);
}
#[test]
fn should_preserve_output_revisions_lifecycle_and_default_eligibility() {
    use shipping_core::profile::ShippingProfile;
    use shipping_service::use_cases::{DefaultEligibility, DefaultView, ProfileView};
    let source = ListingSourceId::new();
    let id = shipping_core::profile::ShippingProfileId::new();
    let command = serde_json::from_value::<CreateData>(
        json!({"creationKey":"one","content":content(),"evidence":[]}),
    )
    .unwrap()
    .into_command(source)
    .unwrap();
    let profile = ShippingProfile::create(id, source, command.content).unwrap();
    let output = serde_json::to_value(
        ProfileData::try_from(ProfileView {
            id,
            source_id: source,
            management_revision: 7u64.try_into().unwrap(),
            revision: profile.revision(),
            lifecycle: profile.lifecycle(),
            content: profile.content().clone(),
        })
        .unwrap(),
    )
    .unwrap();
    for (parent, field) in [
        ("/content", "attributedPartyId"),
        ("/content/terms", "shipping"),
        ("/content/terms", "collection"),
    ] {
        let mut invalid = output.clone();
        invalid
            .pointer_mut(parent)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(
            !validator::<ProfileData>(true).is_valid(&invalid),
            "{parent}/{field}"
        );
    }
    assert_eq!(json!(7), output["managementRevision"]);
    assert_eq!(json!(1), output["termsRevision"]);
    assert!(validator::<ProfileData>(true).is_valid(&output));
    for eligibility in [
        None,
        Some(DefaultEligibility::Active),
        Some(DefaultEligibility::Retired),
        Some(DefaultEligibility::Missing),
    ] {
        let output = serde_json::to_value(DefaultData::from(DefaultView {
            source_id: source,
            revision: 3u64.try_into().unwrap(),
            profile_id: eligibility.map(|_| id),
            eligibility,
        }))
        .unwrap();
        assert!(validator::<DefaultData>(true).is_valid(&output));
        assert_eq!(json!(3), output["revision"]);
    }
}

#[test]
fn should_encode_the_same_terms_with_adapter_owned_public_and_persisted_references() {
    use application::error::{BoxError, box_error};
    use shipping_json::ShippingTermsReferenceCodec;
    struct StoredReferences;
    impl ShippingTermsReferenceCodec for StoredReferences {
        fn decode_party(value: &str) -> Result<PartyId, BoxError> {
            PartyId::try_from(value.parse::<uuid::Uuid>()?).map_err(box_error)
        }
        fn encode_party(value: PartyId) -> String {
            value.into_uuid().to_string()
        }
        fn decode_location(value: &str) -> Result<PartyLocationId, BoxError> {
            PartyLocationId::try_from(value.parse::<uuid::Uuid>()?).map_err(box_error)
        }
        fn encode_location(value: PartyLocationId) -> String {
            value.into_uuid().to_string()
        }
    }
    let source = ListingSourceId::new();
    let party = PartyId::new();
    let location = PartyLocationId::new();
    let mut public = terms::encode(&gallery_terms(8000)).unwrap();
    public["shipping"]["coverage"]["zones"][0]["services"][0]["arranger"] =
        json!({"kind":"PARTY","party_id":party});
    public["collection"]["site"] = json!({"kind":"PARTY_LOCATION","party_location_id":location});
    let domain = terms::decode(public.clone(), source).unwrap();
    let persisted = shipping_json::encode::<StoredReferences>(&domain).unwrap();
    assert_eq!(
        json!(party.into_uuid().to_string()),
        persisted["shipping"]["coverage"]["zones"][0]["services"][0]["arranger"]["party_id"]
    );
    assert_eq!(
        json!(location.into_uuid().to_string()),
        persisted["collection"]["site"]["party_location_id"]
    );
    assert_eq!(
        domain,
        shipping_json::decode::<StoredReferences>(persisted.clone(), source).unwrap()
    );
    assert_eq!(public, terms::encode(&domain).unwrap());
    assert!(terms::decode(persisted.clone(), source).is_err());
    let mut invalid_output = profile_output(source, domain.clone());
    invalid_output["content"]["terms"] = persisted.clone();
    assert!(!validator::<ProfileData>(true).is_valid(&invalid_output));

    assert!(shipping_json::decode::<StoredReferences>(public.clone(), source).is_err());
    let invalid = json!({"creationKey":"wrong-references","content":{"name":"Test","attributedPartyId":null,"terms":persisted},"evidence":[]});
    assert!(!validator::<CreateData>(false).is_valid(&invalid));
    let mut missing = public;
    missing.as_object_mut().unwrap().remove("collection");
    let invalid = json!({"creationKey":"missing-null","content":{"name":"Test","attributedPartyId":null,"terms":missing},"evidence":[]});
    assert!(!validator::<CreateData>(false).is_valid(&invalid));
    assert!(
        serde_json::from_value::<CreateData>(invalid)
            .unwrap()
            .into_command(source)
            .is_err()
    );
}
