use super::*;
use application::error::static_error;
use party_service::use_cases::party_locations::PartyLocationError;
use serde_json::json;
use std::error::Error;

#[tokio::test]
async fn should_expose_only_the_five_location_operations_with_truthful_scope_and_retry_annotations()
{
    let runtime = aura_historia_runtime::Runtime::from_pool(
        sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://fixture:fixture@localhost:9/fixture")
            .unwrap(),
    );
    let operations = operations(runtime.party_locations().clone()).unwrap();
    assert_eq!(
        vec![
            "createPartyLocation",
            "listPartyLocations",
            "getPartyLocation",
            "correctPartyLocation",
            "setPartyLocationLifecycle"
        ],
        operations.iter().map(|op| op.name()).collect::<Vec<_>>()
    );
    for op in operations {
        let read = matches!(op.name(), "listPartyLocations" | "getPartyLocation");
        let annotations = op.tool().annotations.as_ref().unwrap();
        assert_eq!(Some(read), annotations.read_only_hint);
        assert_eq!(Some(!read), annotations.destructive_hint);
        assert_eq!(
            Some(op.name() != "createPartyLocation"),
            annotations.idempotent_hint
        );
        assert_eq!(Some(false), annotations.open_world_hint);
        assert_eq!(
            vec![if read {
                CredentialCapability::PartiesRead
            } else {
                CredentialCapability::PartiesWrite
            }],
            op.policy().required_scopes
        );
        assert_eq!(AccessClass::Protected, op.policy().access);
    }
}
#[test]
fn should_describe_combined_path_and_body_arguments_without_dropping_business_fields() {
    let party = PartyId::new();
    let id = PartyLocationId::new();
    let v = json!({"partyId":party,"label":"Store","roles":["WAREHOUSE"],"geography":null,"position":null,"evidence":null,"relocates":{"id":id,"revision":2}});
    let schema = serde_json::to_value(schemars::schema_for!(PartyArguments<CreateData>)).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    assert!(validator.is_valid(&v));
    let input: PartyArguments<CreateData> = serde_json::from_value(v).unwrap();
    assert_eq!(
        id,
        input
            .data
            .into_command(input.party_id)
            .unwrap()
            .relocates
            .unwrap()
            .id
    );
    for value in [
        json!({"partyId":party,"label":"Store","granted":true}),
        json!({"partyId":party,"label":"Store","userId":"actor"}),
        json!({"partyId":party.as_uuid(),"label":"Store"}),
    ] {
        assert!(!validator.is_valid(&value));
        assert!(serde_json::from_value::<PartyArguments<CreateData>>(value).is_err());
    }
    let value = json!({"partyId":party,"locationId":id,"expectedRevision":3,"correction":"SAME_SITE","geography":null});
    let schema =
        serde_json::to_value(schemars::schema_for!(LocationArguments<UpdateData>)).unwrap();
    assert!(jsonschema::validator_for(&schema).unwrap().is_valid(&value));
    let input: LocationArguments<UpdateData> = serde_json::from_value(value).unwrap();
    let c = input
        .data
        .into_command(input.party_id, input.location_id)
        .unwrap();
    assert_eq!(3, c.expected.revision.into_inner());
}
#[test]
fn should_keep_failure_causes_and_classify_safe_location_errors() {
    assert!(matches!(
        errors::location(PartyLocationError::Forbidden),
        OperationError::Forbidden
    ));
    assert!(matches!(
        errors::location(PartyLocationError::ConcurrencyConflict),
        OperationError::Conflict
    ));
    let error = errors::location(PartyLocationError::TemporarilyUnavailable {
        source: static_error("private cause"),
    });
    assert!(matches!(error, OperationError::Unavailable(_)));
    assert_eq!("private cause", error.source().unwrap().to_string());
    let error = errors::location(PartyLocationError::InvalidPersistedState {
        source: static_error("invalid state"),
    });
    assert!(matches!(error, OperationError::Internal(_)));
    assert_eq!("invalid state", error.source().unwrap().to_string());
}
