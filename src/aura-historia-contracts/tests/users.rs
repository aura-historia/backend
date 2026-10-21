use application::patch_field::PatchField;
use aura_historia_contracts::users::{
    account::{OwnUserData, PatchOwnUserData},
    addresses::{AddressData, CreateData, PageData, PageQuery, UpdateData},
    destination::{DestinationData, DestinationResponse, SetDestinationData},
};
use geo::{CountryCode, DerivedGeography, GeoPoint, SpatialPosition, SpatialPrecision};
use localization::Language;
use money::Currency;
use schemars::{JsonSchema, generate::SchemaSettings};
use serde_json::{Value, json};
use user_core::{
    measurement_unit::MeasurementUnit,
    role::UserRole,
    tier::UserTier,
    user_address::{BrowsingDestination, UserAddressId},
    user_id::UserId,
};
use user_service::{
    address_book::{
        DestinationView, UserAddressAttribution, UserAddressPage, UserAddressResolution,
        UserAddressResolutionStatus, UserAddressView,
    },
    ports::UserDetailsView,
};

fn input_schema<T: JsonSchema>() -> jsonschema::Validator {
    let schema = SchemaSettings::default()
        .for_deserialize()
        .into_generator()
        .into_root_schema_for::<T>();
    jsonschema::validator_for(&serde_json::to_value(schema).unwrap()).unwrap()
}
fn assert_output<T: JsonSchema + serde::Serialize>(value: T) -> Value {
    let schema = SchemaSettings::default()
        .for_serialize()
        .into_generator()
        .into_root_schema_for::<T>();
    let output = serde_json::to_value(value).unwrap();
    let validator = jsonschema::validator_for(&serde_json::to_value(schema).unwrap()).unwrap();
    assert!(
        validator.is_valid(&output),
        "{output}: {:?}",
        validator
            .iter_errors(&output)
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
    );
    output
}

#[test]
fn should_preserve_every_profile_patch_state_and_non_nullable_boolean() {
    let id = UserId::new();
    let schema = input_schema::<PatchOwnUserData>();
    let omitted = json!({});
    assert!(schema.is_valid(&omitted));
    let command = serde_json::from_value::<PatchOwnUserData>(omitted)
        .unwrap()
        .into_command(id)
        .unwrap();
    assert!(command.is_empty());
    let cleared = json!({"firstName":null,"lastName":null,"language":null,"currency":null,"measurementUnit":null});
    assert!(schema.is_valid(&cleared));
    let command = serde_json::from_value::<PatchOwnUserData>(cleared)
        .unwrap()
        .into_command(id)
        .unwrap();
    assert_eq!(PatchField::Clear, command.first_name);
    assert_eq!(PatchField::Clear, command.last_name);
    assert_eq!(PatchField::Clear, command.language);
    assert_eq!(PatchField::Clear, command.currency);
    assert_eq!(PatchField::Clear, command.measurement_unit);
    assert_eq!(
        PatchField::Unchanged,
        command.show_unassessed_or_sensitive_content
    );
    let input = json!({"firstName":"é".repeat(80),"lastName":"Lovelace","language":"de-CH","currency":"ARS","measurementUnit":"IMPERIAL","showUnassessedOrSensitiveContent":false,"userId":UserId::new(),"role":"ADMIN"});
    assert!(schema.is_valid(&input));
    let command = serde_json::from_value::<PatchOwnUserData>(input)
        .unwrap()
        .into_command(id)
        .unwrap();
    assert_eq!(id, command.user_id);
    assert_eq!(PatchField::Set("é".repeat(64).into()), command.first_name);
    assert_eq!(PatchField::Set("Lovelace".into()), command.last_name);
    assert_eq!(PatchField::Set(Language::De), command.language);
    assert_eq!(PatchField::Set(Currency::Ars), command.currency);
    assert_eq!(
        PatchField::Set(MeasurementUnit::Imperial),
        command.measurement_unit
    );
    assert_eq!(
        PatchField::Set(false),
        command.show_unassessed_or_sensitive_content
    );
    let null_boolean = json!({"showUnassessedOrSensitiveContent":null});
    assert!(!schema.is_valid(&null_boolean));
    assert!(
        serde_json::from_value::<PatchOwnUserData>(null_boolean)
            .unwrap()
            .into_command(id)
            .is_err()
    );
    for invalid in [
        json!({"language":"invalid"}),
        json!({"currency":"eur"}),
        json!({"measurementUnit":"metric"}),
        json!({"firstName":1}),
    ] {
        assert!(!schema.is_valid(&invalid), "{invalid}");
        assert!(serde_json::from_value::<PatchOwnUserData>(invalid).is_err());
    }
}

#[test]
fn should_serialize_the_complete_existing_own_account_contract() {
    let id = UserId::new();
    let output = assert_output(OwnUserData::from(UserDetailsView {
        user_id: id,
        email: "ada@example.test".parse().unwrap(),
        first_name: Some("Ada".into()),
        last_name: Some("Lovelace".into()),
        language: Some(Language::De),
        currency: Some(Currency::Eur),
        measurement_unit: Some(MeasurementUnit::Metric),
        show_unassessed_or_sensitive_content: true,
        marketing_email_consent: true,
        tier: UserTier::Pro,
        role: UserRole::User,
        stripe_customer_id: Some("cus_fixture".into()),
    }));
    assert_eq!(
        json!({"userId":id,"email":"ada@example.test","firstName":"Ada","lastName":"Lovelace","language":"de","currency":"EUR","measurementUnit":"METRIC","showUnassessedOrSensitiveContent":true,"marketingEmailConsent":true,"tier":"PRO","role":"USER","stripeCustomerId":"cus_fixture"}),
        output
    );
}

fn geography() -> Value {
    json!({"referenceRelease":"iso-codes-4.20.1","addressText":"Synthetic private home","country":"DE","subdivision":"DE-BE","postalCode":"10115"})
}
#[test]
fn should_preserve_strict_full_address_input_and_constructor_validation() {
    let data: CreateData =
        serde_json::from_value(json!({"label":"Home","geography":geography()})).unwrap();
    let command = data.into_command().unwrap();
    assert_eq!("Home", command.label.as_str());
    assert_eq!(Some(CountryCode::DEU), command.geography.country());
    assert_eq!(
        Some("DE-BE"),
        command.geography.subdivision().map(|v| v.as_str())
    );
    assert_eq!(
        Some("10115"),
        command.geography.postal_code().map(|v| v.as_str())
    );
    let id = UserAddressId::new();
    let update: UpdateData = serde_json::from_value(
        json!({"expectedVersion":7,"label":"Replacement","geography":geography()}),
    )
    .unwrap();
    let update = update.into_command(id).unwrap();
    assert_eq!(id, update.id);
    assert_eq!(7, update.expected_version);
    for field in [
        "owner",
        "userId",
        "addressId",
        "preferred",
        "version",
        "inputRevision",
    ] {
        let mut input = json!({"label":"Home","geography":geography()});
        input[field] = json!(id);
        assert!(!input_schema::<CreateData>().is_valid(&input));
        assert!(serde_json::from_value::<CreateData>(input).is_err());
    }
    for (field, value) in [
        ("referenceRelease", json!("current")),
        ("country", json!("XX")),
        ("subdivision", json!("FR-IDF")),
        ("postalCode", json!("\n")),
        ("addressText", json!("")),
    ] {
        let mut input = json!({"label":"Home","geography":geography()});
        input["geography"][field] = value;
        assert!(
            serde_json::from_value::<CreateData>(input)
                .unwrap()
                .into_command()
                .is_err(),
            "{field}"
        );
    }
    for label in [
        "".to_owned(),
        " ".to_owned(),
        "a".repeat(129),
        "home\n".to_owned(),
    ] {
        assert!(
            serde_json::from_value::<CreateData>(json!({"label":label,"geography":geography()}))
                .unwrap()
                .into_command()
                .is_err()
        );
    }
}

#[test]
fn should_retain_complete_resolution_attribution_and_page_cursor_output() {
    let submitted =
        serde_json::from_value::<CreateData>(json!({"label":"Home","geography":geography()}))
            .unwrap()
            .into_command()
            .unwrap();
    let id = UserAddressId::new();
    let next = UserAddressId::new();
    let view = UserAddressView {
        id,
        label: submitted.label,
        geography: submitted.geography.clone(),
        input_revision: 3,
        version: 7,
        preferred: true,
        resolution: UserAddressResolution {
            status: UserAddressResolutionStatus::Accepted,
            derived: DerivedGeography::new(
                Some(submitted.geography),
                Some(
                    SpatialPosition::new(
                        GeoPoint::new(52.5, 13.4).unwrap(),
                        SpatialPrecision::Premises,
                        Some(10.0),
                    )
                    .unwrap(),
                ),
            ),
            attribution: Some(UserAddressAttribution {
                provider: "TEST".into(),
                attribution_url: "https://example.test/attribution".into(),
            }),
        },
    };
    let address = assert_output(AddressData::from(view.clone()));
    assert_eq!(geography(), address["geography"]);
    assert_eq!(geography(), address["resolution"]["derived"]["geography"]);
    assert_eq!(
        json!({"latitudeDegrees":52.5,"longitudeDegrees":13.4,"precision":"PREMISES","accuracyMetres":10.0}),
        address["resolution"]["derived"]["position"]
    );
    assert_eq!(
        json!({"provider":"TEST","attributionUrl":"https://example.test/attribution"}),
        address["resolution"]["attribution"]
    );
    let page = assert_output(PageData::from(UserAddressPage {
        items: vec![view],
        next: Some(next),
    }));
    assert_eq!(json!(next), page["next"]);
    assert_eq!(address, page["items"][0]);
    let input = json!({"after":next,"limit":100});
    assert!(input_schema::<PageQuery>().is_valid(&input));
    let query: PageQuery = serde_json::from_value(input).unwrap();
    assert_eq!(Some(next), query.after);
    assert_eq!(Some(100), query.limit);
    assert_eq!(
        json!(null),
        assert_output(PageData::from(UserAddressPage {
            items: vec![],
            next: None
        }))["next"]
    );
}

#[test]
fn should_preserve_all_destination_variants_and_strict_identity() {
    let id = UserAddressId::new();
    for destination in [
        json!({"kind":"NONE"}),
        json!({"kind":"ADDRESS","addressId":id}),
        json!({"kind":"COUNTRY_POSTAL","country":"DE","postalCode":"10115"}),
        json!({"kind":"COUNTRY_POSTAL","country":"FR"}),
    ] {
        let input = json!({"expectedVersion":8,"destination":destination});
        assert!(input_schema::<SetDestinationData>().is_valid(&input));
        let data: SetDestinationData = serde_json::from_value(input).unwrap();
        assert_eq!(8, data.expected_version);
        let output = assert_output(DestinationResponse::from(DestinationView {
            version: 9,
            destination: data.destination.into_core().unwrap(),
        }));
        assert_eq!(json!(9), output["version"]);
        assert!(input_schema::<DestinationData>().is_valid(&output["destination"]));
    }
    for invalid in [
        json!({"kind":"NONE","country":"FR"}),
        json!({"kind":"ADDRESS","addressId":id,"postalCode":"10115"}),
        json!({"kind":"COUNTRY_POSTAL","country":"FR","addressId":id}),
        json!({"kind":"ADDRESS","addressId":id.as_uuid()}),
    ] {
        assert!(!input_schema::<DestinationData>().is_valid(&invalid));
        assert!(serde_json::from_value::<DestinationData>(invalid).is_err());
    }
    for invalid in [
        json!({"kind":"COUNTRY_POSTAL","country":"XX"}),
        json!({"kind":"COUNTRY_POSTAL","country":"DE","postalCode":"\n"}),
    ] {
        assert!(
            serde_json::from_value::<DestinationData>(invalid)
                .unwrap()
                .into_core()
                .is_err()
        );
    }
    assert!(matches!(
        serde_json::from_value::<DestinationData>(json!({"kind":"NONE"}))
            .unwrap()
            .into_core()
            .unwrap(),
        BrowsingDestination::None
    ));
}
