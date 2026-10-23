use application::patch_field::PatchField;
use aura_historia_contracts::{
    listing_sources::location_default as defaults, party_locations::management as locations,
};
use listing_source_core::ListingSourceId;
use party_core::{party_id::PartyId, party_location_id::PartyLocationId};
use schemars::{JsonSchema, generate::SchemaSettings};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

fn input<T: DeserializeOwned + JsonSchema>(value: Value) -> T {
    let schema = serde_json::to_value(
        SchemaSettings::default()
            .for_deserialize()
            .into_generator()
            .into_root_schema_for::<T>(),
    )
    .unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    assert!(validator.is_valid(&value), "{value}: {schema}");
    serde_json::from_value(value).unwrap()
}
fn output<T: serde::Serialize + JsonSchema>(data: T) -> Value {
    let value = serde_json::to_value(data).unwrap();
    let schema = serde_json::to_value(
        SchemaSettings::default()
            .for_serialize()
            .into_generator()
            .into_root_schema_for::<T>(),
    )
    .unwrap();
    assert!(
        jsonschema::validator_for(&schema).unwrap().is_valid(&value),
        "{value}: {schema}"
    );
    value
}
#[test]
fn should_preserve_all_roles_geography_position_evidence_and_relocation() {
    let party = PartyId::new();
    let old = PartyLocationId::new();
    let data: locations::CreateData = input(json!({
        "label":"  Store  ","roles":["BUSINESS_PREMISES","WAREHOUSE","REGISTERED_ADDRESS","CORRESPONDENCE"],
        "geography":{"referenceRelease":"iso-codes-4.20.1","addressText":"Straße 1","country":"DE","subdivision":"DE-BY","postalCode":"01234"},
        "position":{"latitudeDegrees":0.0,"longitudeDegrees":0.0,"precision":"PREMISES","accuracyMetres":0.0},
        "evidence":{"reference":"Private observation","observedAt":"2026-10-20T12:00:00+02:00","scope":"SITE_DESCRIPTION"},
        "relocates":{"id":old,"revision":7}
    }));
    let c = data.into_command(party).unwrap();
    assert_eq!(4, c.content.roles.len());
    assert_eq!("Store", c.content.label.as_str());
    assert_eq!(old, c.relocates.unwrap().id);
    assert_eq!(7, c.relocates.unwrap().revision.into_inner());
    let view = party_service::use_cases::party_locations::PartyLocationView {
        id: PartyLocationId::new(),
        party_id: party,
        label: c.content.label,
        roles: c.content.roles,
        geography: c.content.geography,
        position: c.content.position,
        lifecycle: party_core::party_location::PartyLocationLifecycle::Active,
        revision: party_service::ports::party_location::PartyLocationRevision::try_from(u64::MAX)
            .unwrap(),
        input_revision: u64::MAX,
        evidence: c.evidence,
    };
    let v = output(locations::LocationData::from(view));
    assert_eq!(json!(u64::MAX), v["revision"]);
    assert_eq!(json!(u64::MAX), v["inputRevision"]);
    assert_eq!("01234", v["geography"]["postalCode"]);
    assert_eq!("DE-BY", v["geography"]["subdivision"]);
    assert_eq!("2026-10-20T10:00:00Z", v["evidence"]["observedAt"]);
    assert_eq!(0.0, v["position"]["accuracyMetres"]);
}
#[test]
fn should_preserve_omitted_null_and_value_corrections_and_reject_required_clears() {
    let party = PartyId::new();
    let id = PartyLocationId::new();
    let omitted: locations::UpdateData =
        input(json!({"expectedRevision":1,"correction":"SAME_SITE"}));
    let c = omitted.into_command(party, id).unwrap();
    assert_eq!(PatchField::Unchanged, c.label);
    assert_eq!(PatchField::Unchanged, c.roles);
    assert_eq!(PatchField::Unchanged, c.geography);
    assert_eq!(PatchField::Unchanged, c.position);
    assert_eq!(PatchField::Unchanged, c.evidence);
    let clear: locations::UpdateData = input(
        json!({"expectedRevision":2,"correction":"SAME_SITE","label":"Corrected","roles":[],"geography":null,"position":null,"evidence":null}),
    );
    let c = clear.into_command(party, id).unwrap();
    assert_eq!(2, c.expected.revision.into_inner());
    assert_eq!(id, c.expected.id);
    assert!(matches!(c.label, PatchField::Set(_)));
    assert!(matches!(c.roles,PatchField::Set(v) if v.is_empty()));
    assert_eq!(PatchField::Clear, c.geography);
    assert_eq!(PatchField::Clear, c.position);
    assert_eq!(PatchField::Clear, c.evidence);
    for field in ["label", "roles"] {
        let mut value = json!({"expectedRevision":1,"correction":"SAME_SITE"});
        value[field] = Value::Null;
        let schema = serde_json::to_value(schemars::schema_for!(locations::UpdateData)).unwrap();
        assert!(!jsonschema::validator_for(&schema).unwrap().is_valid(&value));
        let data: locations::UpdateData = serde_json::from_value(value).unwrap();
        assert!(data.into_command(party, id).is_err());
    }
    for correction in ["RELOCATION", "same_site", ""] {
        let data: locations::UpdateData =
            serde_json::from_value(json!({"expectedRevision":1,"correction":correction})).unwrap();
        assert!(data.into_command(party, id).is_err());
    }
}
#[test]
fn should_validate_roles_coordinates_geographic_codes_and_opaque_evidence() {
    for extra in [
        json!({"roles":["WAREHOUSE","WAREHOUSE"]}),
        json!({"roles":["PICKUP"]}),
        json!({"geography":{"referenceRelease":"iso-codes-4.20.1","country":"de"}}),
        json!({"position":{"latitudeDegrees":91.0,"longitudeDegrees":0,"precision":"PREMISES"}}),
        json!({"position":{"latitudeDegrees":0,"longitudeDegrees":0,"precision":"PREMISES","accuracyMetres":-1}}),
        json!({"evidence":{"reference":"private\nvalue","scope":"SITE_DESCRIPTION"}}),
        json!({"evidence":{"reference":"x","scope":"ADVERTISED_INVENTORY"}}),
    ] {
        let mut value = json!({"label":"Store"});
        value
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let d: locations::CreateData = serde_json::from_value(value).unwrap();
        assert!(d.into_command(PartyId::new()).is_err());
    }
    for scope in [
        "SITE_DESCRIPTION",
        "REGISTERED_ADDRESS",
        "CORRESPONDENCE_ADDRESS",
    ] {
        let d: locations::CreateData =
            input(json!({"label":"Store","evidence":{"reference":"x","scope":scope}}));
        assert!(d.into_command(PartyId::new()).is_ok());
    }
}
#[test]
fn should_preserve_location_page_defaults_continuations_and_null_next() {
    let party = PartyId::new();
    let id = PartyLocationId::new();
    for value in [json!({}), json!({"limit":null,"after":null})] {
        let d: locations::ListData = input(value);
        let request = d.into_request(party).unwrap();
        assert_eq!(25, request.limit);
        assert_eq!(None, request.after);
    }
    for limit in [1, 100] {
        let d: locations::ListData = input(json!({"limit":limit,"after":id}));
        let r = d.into_request(party).unwrap();
        assert_eq!(Some(id), r.after);
        assert_eq!(limit, r.limit);
    }
    for limit in [0, 101] {
        let d: locations::ListData = serde_json::from_value(json!({"limit":limit})).unwrap();
        assert!(d.into_request(party).is_err());
    }
    let v = output(locations::PageData::from(
        party_service::use_cases::party_locations::PartyLocationsPage {
            items: vec![],
            next: None,
        },
    ));
    assert_eq!(json!({"items":[],"next":null}), v);
}
#[test]
fn should_preserve_default_assignment_applicability_and_explicit_clear_tombstone() {
    let source = ListingSourceId::new();
    let id = PartyLocationId::new();
    let data: defaults::AssignData = input(
        json!({"expectedRevision":1,"partyLocationId":id,"businessAssociation":{"reference":"business","scope":"ASSOCIATED_BUSINESS"},"advertisedInventory":{"reference":"inventory","scope":"ADVERTISED_INVENTORY"}}),
    );
    let c = data.into_command(source).unwrap();
    assert_eq!(source, c.source_id);
    assert_eq!(id, c.assignment.location_id());
    let assigned=output(defaults::DefaultData::from(listing_source_service::use_cases::location_default::LocationDefaultView { source_id:source,revision:c.expected_revision,assignment:Some(c.assignment),eligibility:Some(listing_source_service::use_cases::location_default::SelectedLocationEligibility::Retired) }));
    assert_eq!("RETIRED", assigned["eligibility"]);
    assert_eq!(
        "inventory",
        assigned["assignment"]["advertisedInventory"]["reference"]
    );
    let d: defaults::ChangeData = input(json!({"expectedRevision":2,"businessAssociation":null}));
    let c = d.into_command(source).unwrap();
    assert_eq!(PatchField::Clear, c.business_association);
    assert_eq!(PatchField::Unchanged, c.advertised_inventory);
    let d: defaults::ChangeData = input(
        json!({"expectedRevision":2,"advertisedInventory":{"reference":"updated","scope":"ADVERTISED_INVENTORY"}}),
    );
    assert!(matches!(
        d.into_command(source).unwrap().advertised_inventory,
        PatchField::Set(_)
    ));
    let clear: defaults::ClearData = input(json!({"expectedRevision":i64::MAX}));
    let c = clear.into_command(source);
    let cleared = output(defaults::DefaultData::from(
        listing_source_service::use_cases::location_default::LocationDefaultView {
            source_id: source,
            revision: c.expected_revision,
            assignment: None,
            eligibility: None,
        },
    ));
    assert_eq!(
        json!({"listingSourceId":source,"revision":i64::MAX,"assignment":null,"eligibility":null}),
        cleared
    );
}
#[test]
fn should_reject_invalid_revisions_ids_and_mismatched_source_meanings() {
    let id = PartyLocationId::new();
    for revision in [
        json!(0),
        json!(-1),
        json!(null),
        json!(i64::MAX as u64 + 1),
        json!(1.5),
    ] {
        let v = json!({"expectedRevision":revision});
        assert!(serde_json::from_value::<defaults::ClearData>(v.clone()).is_err());
        let schema = serde_json::to_value(schemars::schema_for!(defaults::ClearData)).unwrap();
        assert!(!jsonschema::validator_for(&schema).unwrap().is_valid(&v));
    }
    for scope in ["ADVERTISED_INVENTORY", "SITE_DESCRIPTION"] {
        let d:defaults::AssignData=serde_json::from_value(json!({"expectedRevision":1,"partyLocationId":id,"businessAssociation":{"reference":"x","scope":scope}})).unwrap();
        assert!(d.into_command(ListingSourceId::new()).is_err());
    }
    for id in [json!(id.as_uuid()), json!("wrong"), json!(null)] {
        assert!(
            serde_json::from_value::<defaults::AssignData>(
                json!({"expectedRevision":1,"partyLocationId":id})
            )
            .is_err()
        );
    }
    // Selecting a site without claiming either applicability is supported by REST.
    let d: defaults::AssignData = input(json!({"expectedRevision":1,"partyLocationId":id}));
    assert!(d.into_command(ListingSourceId::new()).is_ok());
}
