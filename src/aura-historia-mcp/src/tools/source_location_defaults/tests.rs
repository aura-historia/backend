use super::*;
use application::error::static_error;
use listing_source_service::use_cases::location_default::LocationDefaultError;
use party_core::party_location_id::PartyLocationId;
use serde_json::json;
use std::error::Error;
#[tokio::test]
async fn should_expose_only_four_default_operations_with_source_scopes_and_revision_annotations() {
    let runtime = aura_historia_runtime::Runtime::from_pool(
        sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://fixture:fixture@localhost:9/fixture")
            .unwrap(),
    );
    let operations = operations(runtime.source_location_defaults().clone()).unwrap();
    assert_eq!(
        vec![
            "getSourceLocationDefault",
            "assignSourceLocationDefault",
            "changeSourceLocationDefaultApplicability",
            "clearSourceLocationDefault"
        ],
        operations.iter().map(|op| op.name()).collect::<Vec<_>>()
    );
    for op in operations {
        let read = op.name() == "getSourceLocationDefault";
        let annotations = op.tool().annotations.as_ref().unwrap();
        assert_eq!(Some(read), annotations.read_only_hint);
        assert_eq!(Some(!read), annotations.destructive_hint);
        assert_eq!(Some(true), annotations.idempotent_hint);
        assert_eq!(Some(false), annotations.open_world_hint);
        assert_eq!(
            vec![if read {
                CredentialCapability::ListingSourcesRead
            } else {
                CredentialCapability::ListingSourcesWrite
            }],
            op.policy().required_scopes
        );
        assert_eq!(AccessClass::Protected, op.policy().access);
    }
}
#[test]
fn should_describe_combined_source_arguments_preserving_null_applicability_and_strict_ids() {
    let source = ListingSourceId::new();
    let id = PartyLocationId::new();
    let value = json!({"listingSourceId":source,"expectedRevision":2,"partyLocationId":id,"businessAssociation":{"reference":"x","scope":"ASSOCIATED_BUSINESS"},"advertisedInventory":null});
    let schema = serde_json::to_value(schemars::schema_for!(SourceArguments<AssignData>)).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    assert!(validator.is_valid(&value));
    let input: SourceArguments<AssignData> = serde_json::from_value(value).unwrap();
    assert_eq!(
        id,
        input
            .data
            .into_command(input.listing_source_id)
            .unwrap()
            .assignment
            .location_id()
    );
    for value in [
        json!({"listingSourceId":source,"expectedRevision":0,"partyLocationId":id}),
        json!({"listingSourceId":source,"expectedRevision":1,"partyLocationId":id,"granted":true}),
        json!({"listingSourceId":source.as_uuid(),"expectedRevision":1,"partyLocationId":id}),
    ] {
        assert!(!validator.is_valid(&value));
        assert!(serde_json::from_value::<SourceArguments<AssignData>>(value).is_err());
    }
    let value = json!({"listingSourceId":source,"expectedRevision":2,"advertisedInventory":null});
    let schema = serde_json::to_value(schemars::schema_for!(SourceArguments<ChangeData>)).unwrap();
    assert!(jsonschema::validator_for(&schema).unwrap().is_valid(&value));
    let input: SourceArguments<ChangeData> = serde_json::from_value(value).unwrap();
    let c = input.data.into_command(input.listing_source_id).unwrap();
    assert_eq!(
        application::patch_field::PatchField::Clear,
        c.advertised_inventory
    );
}
#[test]
fn should_keep_failure_causes_and_classify_safe_default_errors() {
    for error in [
        LocationDefaultError::ConcurrencyConflict,
        LocationDefaultError::LocationRetired,
        LocationDefaultError::DefaultNotAssigned,
    ] {
        assert!(matches!(
            errors::default(error),
            OperationError::Conflict(None)
        ));
    }
    assert!(matches!(
        errors::default(LocationDefaultError::LocationNotFound),
        OperationError::NotFound
    ));
    let error = errors::default(LocationDefaultError::TemporarilyUnavailable {
        source: static_error("private cause"),
    });
    assert!(matches!(error, OperationError::Unavailable(_)));
    assert_eq!("private cause", error.source().unwrap().to_string());
    let error = errors::default(LocationDefaultError::InvalidPersistedState {
        source: static_error("invalid state"),
    });
    assert!(matches!(error, OperationError::Internal(_)));
    assert_eq!("invalid state", error.source().unwrap().to_string());
}
