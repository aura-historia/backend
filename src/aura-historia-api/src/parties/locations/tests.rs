use super::*;
use application::patch_field::PatchField;
use aura_historia_contracts::geography::PositionData;
use std::collections::BTreeSet;
#[test]
fn creates_unresolved_sites_without_any_geocoder() {
    for body in [
        r#"{"label":"Store"}"#,
        r#"{"label":"Store","geography":{"referenceRelease":"iso-codes-4.20.1","country":"DE"}}"#,
        r#"{"label":"Store","geography":{"referenceRelease":"iso-codes-4.20.1","addressText":"Unknown road"}}"#,
    ] {
        let c = create_command(PartyId::new(), body).unwrap();
        assert!(c.content.position.is_none());
    }
    for body in [
        r#"{"label":"Store","disclosure":null}"#,
        r#"{"label":"Store","trust":"ADMIN"}"#,
        r#"{"label":"Store","evidence":{"reference":"x","scope":"LISTING_INVENTORY"}}"#,
        r#"{"label":"Store","geography":{"referenceRelease":"iso-codes-4.20.1","country":"de"}}"#,
        r#"{"label":"Store","roles":["WAREHOUSE","WAREHOUSE"]}"#,
    ] {
        assert!(create_command(PartyId::new(), body).is_err());
    }
}
#[test]
fn maps_omitted_set_clear_and_requires_same_site_assertion() {
    let party = PartyId::new();
    let id = PartyLocationId::new();
    let c = update_command(party,id,r#"{"expectedRevision":1,"correction":"SAME_SITE","geography":null,"position":null,"evidence":null,"roles":[]}"#).unwrap();
    assert_eq!(PatchField::Unchanged, c.label);
    assert_eq!(PatchField::Clear, c.geography);
    assert_eq!(PatchField::Clear, c.position);
    assert_eq!(PatchField::Clear, c.evidence);
    assert_eq!(PatchField::Set(BTreeSet::new()), c.roles);
    let c = update_command(
        party,
        id,
        r#"{"expectedRevision":1,"correction":"SAME_SITE","label":"Corrected"}"#,
    )
    .unwrap();
    assert!(matches!(c.label, PatchField::Set(_)));
    for body in [
        r#"{}"#,
        r#"{"expectedRevision":0,"correction":"SAME_SITE"}"#,
        r#"{"expectedRevision":1,"correction":"RELOCATION"}"#,
        r#"{"expectedRevision":1,"correction":"SAME_SITE","label":null}"#,
        r#"{"expectedRevision":1,"correction":"SAME_SITE","roles":null}"#,
        r#"{"expectedRevision":1,"correction":"SAME_SITE","disclosure":null}"#,
    ] {
        assert!(update_command(party, id, body).is_err());
    }
}
#[test]
fn ids_and_positions_use_validated_constructors() {
    let id = PartyLocationId::new();
    assert_eq!(
        id,
        serde_json::from_value::<PartyLocationId>(serde_json::json!(id)).unwrap()
    );
    assert!(serde_json::from_value::<PartyLocationId>(serde_json::json!(id.as_uuid())).is_err());
    for latitude in [91.0, f64::INFINITY, f64::NAN] {
        assert!(
            PositionData {
                latitude_degrees: latitude,
                longitude_degrees: 0.0,
                precision: "PREMISES".to_owned(),
                accuracy_metres: None
            }
            .into_core()
            .is_err()
        );
    }
}
