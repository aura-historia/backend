//! Current-user profile, private addresses and browsing-destination operations.
mod errors;

use crate::{AccessClass, OperationDefinition, OperationError, OperationPolicy, RegistryError};
use application::operation_context::{CredentialCapability, OperationContext, Principal};
use aura_historia_contracts::users::{
    account::{OwnUserData, PatchOwnUserData},
    addresses::{AddressData, CreateData, ExpectedData, PageData, PageQuery, UpdateData},
    destination::{DestinationResponse, SetDestinationData},
};
use aura_historia_runtime::{PrivateAddresses, UserProfile};
use schemars::JsonSchema;
use serde::Deserialize;
use std::sync::Arc;
use user_core::{user_address::UserAddressId, user_id::UserId};
use user_service::{
    address_book::{AddressBookCommands, AddressBookQueries},
    use_cases::{
        commands::update_user_profile::UpdateUserProfileUseCase,
        queries::get_own_user::{GetOwnUserRequest, GetOwnUserUseCase},
    },
};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct OwnAccountArguments {}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AddressArguments {
    #[schemars(schema_with = "aura_historia_contracts::wire::schema::user_address_id")]
    address_id: UserAddressId,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateAddressArguments {
    #[schemars(schema_with = "aura_historia_contracts::wire::schema::user_address_id")]
    address_id: UserAddressId,
    #[serde(flatten)]
    data: UpdateData,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeleteAddressArguments {
    #[schemars(schema_with = "aura_historia_contracts::wire::schema::user_address_id")]
    address_id: UserAddressId,
    #[serde(flatten)]
    data: ExpectedData,
}

fn read_policy() -> OperationPolicy {
    OperationPolicy {
        access: AccessClass::Protected,
        required_scopes: vec![CredentialCapability::UsersRead],
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
    }
}

fn replacement_policy() -> OperationPolicy {
    OperationPolicy {
        access: AccessClass::Protected,
        required_scopes: vec![CredentialCapability::UsersWrite],
        read_only: false,
        destructive: true,
        idempotent: true,
        open_world: false,
    }
}

fn creation_policy() -> OperationPolicy {
    OperationPolicy {
        destructive: false,
        idempotent: false,
        ..replacement_policy()
    }
}

fn acting_user(context: &OperationContext) -> Result<UserId, OperationError> {
    match context.principal {
        Principal::User(id) | Principal::DelegatedUser { user_id: id, .. } => Ok(id),
        _ => Err(OperationError::Forbidden),
    }
}

/// One family in the shared native/Lambda registry; services own authorization and CAS.
pub fn operations(
    profile: UserProfile,
    addresses: PrivateAddresses,
) -> Result<Vec<OperationDefinition>, RegistryError> {
    Ok(vec![
        get_account(profile.get)?,
        update_account(profile.update)?,
        create_address(addresses.commands.clone())?,
        list_addresses(addresses.queries.clone())?,
        get_address(addresses.queries.clone())?,
        update_address(addresses.commands.clone())?,
        delete_address(addresses.commands.clone())?,
        get_destination(addresses.queries)?,
        set_destination(addresses.commands)?,
    ])
}

fn get_account(get: Arc<dyn GetOwnUserUseCase>) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "getUserAccount",
        "Read your complete Aura account and profile preferences.",
        read_policy(),
        move |_: OwnAccountArguments, invocation| {
            let get = get.clone();
            async move {
                let result = invocation
                    .execute(get.execute(&invocation.operation, GetOwnUserRequest))
                    .await?
                    .map_err(errors::get_profile)?;
                Ok(OwnUserData::from(result))
            }
        },
    )
}

fn update_account(
    update: Arc<dyn UpdateUserProfileUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "updateUserAccount",
        "Update your profile and preferences. Omitted members are unchanged; null clears optional values. The sensitive-content boolean cannot be null.",
        replacement_policy(),
        move |input: PatchOwnUserData, invocation| {
            let update = update.clone();
            async move {
                let command = input
                    .into_command(acting_user(&invocation.operation)?)
                    .map_err(|error| OperationError::InvalidArguments(Box::new(error)))?;
                let result = invocation
                    .execute(update.execute(&invocation.operation, command))
                    .await?
                    .map_err(errors::update_profile)?;
                Ok(OwnUserData::from(result.view))
            }
        },
    )
}

fn create_address(
    create: Arc<dyn AddressBookCommands>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "createUserAddress",
        "Create a private address bookmark with full submitted geography. Every accepted call creates a new identity; after an unknown outcome, reconcile your address book before retrying.",
        creation_policy(),
        move |input: CreateData, invocation| {
            let create = create.clone();
            async move {
                let command = input
                    .into_command()
                    .map_err(|error| OperationError::InvalidArguments(Box::new(error)))?;
                let result = invocation
                    .execute(create.create(&invocation.operation, command))
                    .await?
                    .map_err(errors::address)?;
                Ok(AddressData::from(result))
            }
        },
    )
}

fn list_addresses(list: Arc<dyn AddressBookQueries>) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "listUserAddresses",
        "List your private addresses, resolution and source attribution. Pass next as after to continue; limit defaults to 25 and accepts 1 through 100.",
        read_policy(),
        move |input: PageQuery, invocation| {
            let list = list.clone();
            async move {
                let result = invocation
                    .execute(list.list(
                        &invocation.operation,
                        input.after,
                        input.limit.unwrap_or(25),
                    ))
                    .await?
                    .map_err(errors::address)?;
                Ok(PageData::from(result))
            }
        },
    )
}

fn get_address(get: Arc<dyn AddressBookQueries>) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "getUserAddress",
        "Read an owned private address, complete resolution geography and source attribution.",
        read_policy(),
        move |input: AddressArguments, invocation| {
            let get = get.clone();
            async move {
                let result = invocation
                    .execute(get.get(&invocation.operation, input.address_id))
                    .await?
                    .map_err(errors::address)?;
                Ok(AddressData::from(result))
            }
        },
    )
}

fn update_address(
    update: Arc<dyn AddressBookCommands>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "updateUserAddress",
        "Replace the label and submitted geography of an owned address using its expectedVersion. The address identity and owner are immutable.",
        replacement_policy(),
        move |input: UpdateAddressArguments, invocation| {
            let update = update.clone();
            async move {
                let command = input
                    .data
                    .into_command(input.address_id)
                    .map_err(|error| OperationError::InvalidArguments(Box::new(error)))?;
                let result = invocation
                    .execute(update.update(&invocation.operation, command))
                    .await?
                    .map_err(errors::address)?;
                Ok(AddressData::from(result))
            }
        },
    )
}

fn delete_address(
    delete: Arc<dyn AddressBookCommands>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "deleteUserAddress",
        "Delete the selected owned address using its expectedVersion. If selected for browsing, deletion clears that destination. Returns result:null on success.",
        replacement_policy(),
        move |input: DeleteAddressArguments, invocation| {
            let delete = delete.clone();
            async move {
                invocation
                    .execute(delete.delete(
                        &invocation.operation,
                        input.address_id,
                        input.data.expected_version,
                    ))
                    .await?
                    .map_err(errors::address)
            }
        },
    )
}

fn get_destination(get: Arc<dyn AddressBookQueries>) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "getBrowsingDestination",
        "Read your versioned browsing destination: NONE, an owned ADDRESS, or COUNTRY_POSTAL.",
        read_policy(),
        move |_: OwnAccountArguments, invocation| {
            let get = get.clone();
            async move {
                let result = invocation
                    .execute(get.destination(&invocation.operation))
                    .await?
                    .map_err(errors::address)?;
                Ok(DestinationResponse::from(result))
            }
        },
    )
}

fn set_destination(
    set: Arc<dyn AddressBookCommands>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "setBrowsingDestination",
        "Set or clear your browsing destination using its expectedVersion. NONE clears it; ADDRESS must belong to you; COUNTRY_POSTAL accepts a country and optional postalCode.",
        replacement_policy(),
        move |input: SetDestinationData, invocation| {
            let set = set.clone();
            async move {
                let destination = input
                    .destination
                    .into_core()
                    .map_err(|error| OperationError::InvalidArguments(Box::new(error)))?;
                let result = invocation
                    .execute(set.set_destination(
                        &invocation.operation,
                        destination,
                        input.expected_version,
                    ))
                    .await?
                    .map_err(errors::address)?;
                Ok(DestinationResponse::from(result))
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{InvocationContext, MAX_RESPONSE_BYTES, Registry};
    use application::operation_context::{CorrelationId, RequestId};
    use aura_historia_runtime::Runtime;
    use serde_json::{Value, json};
    use std::{collections::BTreeSet, sync::Arc};
    use tokio_util::sync::CancellationToken;

    fn context(
        owner: UserId,
        capabilities: impl IntoIterator<Item = CredentialCapability>,
    ) -> InvocationContext {
        InvocationContext {
            operation: OperationContext {
                principal: Principal::DelegatedUser {
                    user_id: owner,
                    capabilities: capabilities.into_iter().collect(),
                },
                request_id: RequestId::new("account-request"),
                correlation_id: CorrelationId::new("account-correlation"),
            },
            deadline: tokio::time::Instant::now() + std::time::Duration::from_secs(20),
            cancellation: CancellationToken::new(),
            max_response_bytes: MAX_RESPONSE_BYTES,
        }
    }
    fn registry(runtime: &Runtime) -> Registry {
        Registry::new(
            operations(
                runtime.user_profile().clone(),
                runtime.private_addresses().clone(),
            )
            .unwrap(),
        )
        .unwrap()
    }
    fn input() -> Value {
        json!({"label":"Home","geography":{"referenceRelease":"iso-codes-4.20.1","addressText":"Synthetic private home","country":"DE","subdivision":"DE-BE","postalCode":"10115"}})
    }
    fn requests(id: UserAddressId) -> Vec<(&'static str, Value)> {
        vec![
            ("getUserAccount", json!({})),
            ("updateUserAccount", json!({"firstName":"Ada"})),
            ("createUserAddress", input()),
            ("listUserAddresses", json!({})),
            ("getUserAddress", json!({"addressId":id})),
            (
                "updateUserAddress",
                json!({"addressId":id,"expectedVersion":1,"label":"Home","geography":input()["geography"]}),
            ),
            (
                "deleteUserAddress",
                json!({"addressId":id,"expectedVersion":1}),
            ),
            ("getBrowsingDestination", json!({})),
            (
                "setBrowsingDestination",
                json!({"expectedVersion":1,"destination":{"kind":"NONE"}}),
            ),
        ]
    }
    async fn invoke(
        registry: &Registry,
        name: &str,
        input: Value,
        context: &InvocationContext,
    ) -> Value {
        let operation = registry.get(name).unwrap();
        let result = operation
            .call(input, context.clone())
            .await
            .unwrap_or_else(|error| panic!("{name} failed: {error:?}"));
        assert_ne!(Some(true), result.is_error, "{result:?}");
        let output = result.structured_content.unwrap();
        let schema =
            serde_json::to_value(operation.tool().output_schema.as_ref().unwrap()).unwrap();
        let validator = jsonschema::validator_for(&schema).unwrap();
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

    #[tokio::test]
    async fn should_publish_exact_family_manifest_schemas_and_truthful_effects() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://fixture:fixture@localhost:9/fixture")
            .unwrap();
        let runtime = Runtime::from_pool(pool);
        let registry = registry(&runtime);
        let manifest: Value =
            serde_json::from_str(include_str!("../../authenticated-operations.json")).unwrap();
        let entries = manifest["operations"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["family"] == "account")
            .collect::<Vec<_>>();
        assert_eq!(9, entries.len());
        let (tools, next) = registry.page(None).unwrap();
        assert!(next.is_none());
        assert_eq!(9, tools.len());
        assert_eq!(
            entries
                .iter()
                .map(|e| e["name"].as_str().unwrap())
                .collect::<BTreeSet<_>>(),
            tools.iter().map(|t| t.name.as_ref()).collect()
        );
        for entry in entries {
            let operation = registry.get(entry["name"].as_str().unwrap()).unwrap();
            let policy = operation.policy();
            assert_eq!(AccessClass::Protected, policy.access);
            assert_eq!(entry["operationId"], entry["name"]);
            assert_eq!(entry["access"], "protected");
            assert_eq!(
                entry["requiredScopes"],
                json!(
                    policy
                        .required_scopes
                        .iter()
                        .map(|s| s.as_scope_str())
                        .collect::<Vec<_>>()
                )
            );
            let annotations = operation.tool().annotations.as_ref().unwrap();
            assert_eq!(
                entry["annotations"],
                serde_json::to_value(annotations).unwrap()
            );
            assert!(
                !serde_json::to_string(&operation.tool().input_schema)
                    .unwrap()
                    .contains("userId")
            );
        }
        let id = UserAddressId::new();
        for (name, input) in requests(id) {
            let schema =
                serde_json::to_value(registry.get(name).unwrap().tool().input_schema.as_ref())
                    .unwrap();
            assert!(
                jsonschema::validator_for(&schema).unwrap().is_valid(&input),
                "{name}: {input}"
            );
        }
        for (name, input) in [
            (
                "updateUserAddress",
                json!({"addressId":id,"expectedVersion":1,"label":"Home","geography":input()["geography"],"owner":UserId::new()}),
            ),
            (
                "deleteUserAddress",
                json!({"addressId":id,"expectedVersion":1,"userId":UserId::new()}),
            ),
            (
                "createUserAddress",
                json!({"label":"Home","geography":{"referenceRelease":"iso-codes-4.20.1","addressText":"Home","owner":"foreign"}}),
            ),
            (
                "createUserAddress",
                json!({"label":"Home","geography":{"referenceRelease":"iso-codes-4.20.1","country":"DE"}}),
            ),
            (
                "createUserAddress",
                json!({"label":"Home","geography":{"referenceRelease":"iso-codes-4.20.1","addressText":null}}),
            ),
            (
                "updateUserAddress",
                json!({"addressId":id,"expectedVersion":1,"label":"Home","geography":{"referenceRelease":"iso-codes-4.20.1","country":"DE"}}),
            ),
            (
                "updateUserAddress",
                json!({"addressId":id,"expectedVersion":1,"label":"Home","geography":{"referenceRelease":"iso-codes-4.20.1","addressText":null}}),
            ),
            (
                "setBrowsingDestination",
                json!({"expectedVersion":1,"destination":{"kind":"NONE","country":"DE"}}),
            ),
            ("getUserAddress", json!({"addressId":id.as_uuid()})),
            ("listUserAddresses", json!({"limit":0})),
        ] {
            let schema =
                serde_json::to_value(registry.get(name).unwrap().tool().input_schema.as_ref())
                    .unwrap();
            assert!(
                !jsonschema::validator_for(&schema).unwrap().is_valid(&input),
                "{name}: {input}"
            );
        }
        for excluded in [
            "deleteUser",
            "deleteUserAccount",
            "createAccessToken",
            "updateAccessToken",
            "createCheckoutSession",
        ] {
            assert!(registry.get(excluded).is_none());
        }
        assert!(Arc::ptr_eq(
            &runtime.user_profile().get,
            &runtime.user_profile().get
        ));
        assert!(Arc::ptr_eq(
            &runtime.user_profile().update,
            &runtime.user_profile().update
        ));
        assert!(Arc::ptr_eq(
            &runtime.private_addresses().commands,
            &runtime.private_addresses().commands
        ));
        assert!(Arc::ptr_eq(
            &runtime.private_addresses().queries,
            &runtime.private_addresses().queries
        ));
        assert_eq!(0, runtime.postgres_pool().size());
    }

    #[test]
    fn should_preserve_technical_error_causes_while_classifying_private_failures() {
        use application::error::static_error;
        use application::transaction::TransactionError;
        use geo_service::resolution::ResolutionError;
        use std::error::Error;
        use user_service::address_book::AddressBookError;
        use user_service::use_cases::{
            commands::update_user_profile::UpdateUserProfileError,
            queries::get_own_user::GetOwnUserError,
        };
        let errors = [
            errors::get_profile(GetOwnUserError::CommitTransactionFailed(static_error(
                "private nested cause",
            ))),
            errors::update_profile(UpdateUserProfileError::BeginTransactionFailed(
                static_error("private nested cause"),
            )),
            errors::address(AddressBookError::Resolution(ResolutionError::Storage(
                static_error("private nested cause"),
            ))),
            errors::address(AddressBookError::Transaction(
                TransactionError::CommitFailed(static_error("private nested cause")),
            )),
            errors::address(AddressBookError::Resolution(ResolutionError::Transaction(
                TransactionError::BeginFailed(static_error("private nested cause")),
            ))),
        ];
        for error in errors {
            assert!(matches!(error, OperationError::Unavailable(_)));
            let mut cause = error.source().unwrap();
            while let Some(source) = cause.source() {
                cause = source;
            }
            assert_eq!("private nested cause", cause.to_string());
            assert!(
                !serde_json::to_string(&error.into_protocol().unwrap())
                    .unwrap()
                    .contains("private")
            );
        }
        assert!(matches!(
            errors::address(AddressBookError::Conflict),
            OperationError::Conflict(None)
        ));
        assert!(matches!(
            errors::address(AddressBookError::NotFound),
            OperationError::NotFound
        ));
        assert!(matches!(
            errors::address(AddressBookError::InvalidState(static_error(
                "private invalid state"
            ))),
            OperationError::Internal(_)
        ));
        assert!(matches!(
            errors::get_profile(GetOwnUserError::Forbidden),
            OperationError::Forbidden
        ));
    }

    #[test]
    fn should_keep_internal_causes_private_for_each_account_service() {
        use application::error::static_error;
        use geo_service::resolution::ResolutionError;
        use std::error::Error;
        use user_service::{
            address_book::AddressBookError,
            use_cases::{
                commands::update_user_profile::UpdateUserProfileError,
                queries::get_own_user::GetOwnUserError,
            },
        };
        for error in [
            errors::get_profile(GetOwnUserError::InvalidReadModel {
                source: static_error("private nested cause"),
            }),
            errors::update_profile(UpdateUserProfileError::InvalidPersistedState {
                source: static_error("private nested cause"),
            }),
            errors::address(AddressBookError::Resolution(ResolutionError::InvalidState(
                static_error("private nested cause"),
            ))),
        ] {
            assert!(matches!(error, OperationError::Internal(_)));
            let mut cause = error.source().unwrap();
            while let Some(source) = cause.source() {
                cause = source;
            }
            assert_eq!("private nested cause", cause.to_string());
            let protocol = error.into_protocol().unwrap_err();
            assert_eq!(rmcp::model::ErrorCode::INTERNAL_ERROR, protocol.code);
            assert!(
                !serde_json::to_string(&protocol)
                    .unwrap()
                    .contains("private")
            );
        }
    }

    mod postgres {
        use super::*;
        use crate::{AuraTokenGate, TransportConfig};
        use axum::{
            body::{Body, to_bytes},
            http::{Request, StatusCode},
        };
        use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};
        use tower::ServiceExt;
        use user_core::access_token::{RawAccessToken, Scope};
        use user_service::use_cases::{
            AuthenticateAccessTokenError, AuthenticateAccessTokenRequest,
            AuthenticateAccessTokenResult, AuthenticateAccessTokenUseCase,
        };

        const SCHEMA: test_api::Postgres = test_api::Postgres::new("migrations");
        async fn seed(pool: &sqlx::PgPool) -> UserId {
            let id = UserId::new();
            sqlx::query("INSERT INTO users(user_id,email,tier,role) VALUES($1,$2,'FREE','USER')")
                .bind(id.into_uuid())
                .bind(format!("{id}@example.test"))
                .execute(pool)
                .await
                .unwrap();
            id
        }
        fn both(owner: UserId) -> InvocationContext {
            context(
                owner,
                [
                    CredentialCapability::UsersRead,
                    CredentialCapability::UsersWrite,
                ],
            )
        }
        async fn assert_error(
            registry: &Registry,
            name: &str,
            input: Value,
            ctx: &InvocationContext,
            expected: fn(&OperationError) -> bool,
        ) {
            let result = registry.get(name).unwrap().call(input, ctx.clone()).await;
            assert!(result.as_ref().is_err_and(expected), "{name}: {result:?}");
        }

        #[aura_integration_test(services = [SCHEMA], isolated = true)]
        async fn should_apply_profile_patches_only_to_the_delegated_owner_and_preserve_scope_checks()
         {
            let pool = get_postgres_client().await;
            let owner = seed(&pool).await;
            let foreign = seed(&pool).await;
            let runtime = Runtime::from_pool(pool);
            let registry = registry(&runtime);
            let ctx = both(owner);
            let initial = invoke(&registry, "getUserAccount", json!({}), &ctx).await;
            assert_eq!(json!(owner), initial["userId"]);
            assert!(initial.get("firstName").is_none());
            let read = context(owner, [CredentialCapability::UsersRead]);
            let update = json!({"firstName":"Ada","lastName":"Lovelace","language":"de-CH","currency":"ARS","measurementUnit":"IMPERIAL","showUnassessedOrSensitiveContent":true,"userId":foreign,"role":"ADMIN","marketingEmailConsent":true});
            assert_error(&registry, "updateUserAccount", update.clone(), &read, |e| {
                matches!(e, OperationError::Forbidden)
            })
            .await;
            let result = invoke(&registry, "updateUserAccount", update, &ctx).await;
            assert_eq!(json!(owner), result["userId"]);
            assert_eq!(json!("Ada"), result["firstName"]);
            assert_eq!(json!("Lovelace"), result["lastName"]);
            assert_eq!(json!("de"), result["language"]);
            assert_eq!(json!("ARS"), result["currency"]);
            assert_eq!(json!("IMPERIAL"), result["measurementUnit"]);
            assert_eq!(json!(true), result["showUnassessedOrSensitiveContent"]);
            assert_eq!(json!(false), result["marketingEmailConsent"]);
            assert_eq!(json!("USER"), result["role"]);
            let foreign_result =
                invoke(&registry, "getUserAccount", json!({}), &both(foreign)).await;
            assert!(foreign_result.get("firstName").is_none());
            assert_eq!(
                result,
                invoke(&registry, "updateUserAccount", json!({}), &ctx).await
            );
            let cleared=invoke(&registry,"updateUserAccount",json!({"firstName":null,"lastName":null,"language":null,"currency":null,"measurementUnit":null}),&ctx).await;
            for field in [
                "firstName",
                "lastName",
                "language",
                "currency",
                "measurementUnit",
            ] {
                assert!(cleared.get(field).is_none());
            }
            assert_eq!(json!(true), cleared["showUnassessedOrSensitiveContent"]);
            assert_error(
                &registry,
                "updateUserAccount",
                json!({"showUnassessedOrSensitiveContent":null}),
                &ctx,
                |e| matches!(e, OperationError::InvalidArguments(_)),
            )
            .await;
            assert_eq!(
                json!(false),
                invoke(
                    &registry,
                    "updateUserAccount",
                    json!({"showUnassessedOrSensitiveContent":false}),
                    &ctx
                )
                .await["showUnassessedOrSensitiveContent"]
            );
            assert_error(
                &registry,
                "getUserAccount",
                json!({}),
                &context(owner, [CredentialCapability::UsersWrite]),
                |e| matches!(e, OperationError::Forbidden),
            )
            .await;
        }

        #[aura_integration_test(services = [SCHEMA], isolated = true)]
        async fn should_preserve_owned_address_pagination_cas_destinations_and_http_scope_challenges()
         {
            let pool = get_postgres_client().await;
            sqlx::query("UPDATE geo_resolution_provider_controls SET is_default=false")
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("INSERT INTO geo_resolution_provider_controls(profile,is_default) VALUES('MCP_ACCOUNT_TEST',true)").execute(&pool).await.unwrap();
            let owner = seed(&pool).await;
            let foreign = seed(&pool).await;
            let runtime = Runtime::from_pool(pool);
            let registry = registry(&runtime);
            let ctx = both(owner);
            let first = invoke(&registry, "createUserAddress", input(), &ctx).await;
            let duplicate = invoke(&registry, "createUserAddress", input(), &ctx).await;
            let foreign_address =
                invoke(&registry, "createUserAddress", input(), &both(foreign)).await;
            assert_ne!(first["addressId"], duplicate["addressId"]);
            let id: UserAddressId = serde_json::from_value(first["addressId"].clone()).unwrap();
            let read = context(owner, [CredentialCapability::UsersRead]);
            let address = invoke(&registry, "getUserAddress", json!({"addressId":id}), &read).await;
            assert_eq!(input()["geography"], address["geography"]);
            assert_eq!(json!(1), address["version"]);
            assert_eq!(
                json!({"status":"PENDING","derived":null,"attribution":null}),
                address["resolution"]
            );
            let mut seen = BTreeSet::new();
            let mut after = Value::Null;
            loop {
                let page = invoke(
                    &registry,
                    "listUserAddresses",
                    json!({"limit":1,"after":after}),
                    &read,
                )
                .await;
                for item in page["items"].as_array().unwrap() {
                    assert!(seen.insert(item["addressId"].as_str().unwrap().to_owned()));
                }
                after = page["next"].clone();
                if after.is_null() {
                    break;
                }
            }
            assert_eq!(
                BTreeSet::from([
                    first["addressId"].as_str().unwrap().to_owned(),
                    duplicate["addressId"].as_str().unwrap().to_owned()
                ]),
                seen
            );
            assert_error(
                &registry,
                "getUserAddress",
                json!({"addressId":id}),
                &both(foreign),
                |e| matches!(e, OperationError::NotFound),
            )
            .await;
            for (name, input) in requests(id)
                .into_iter()
                .filter(|(name, _)| matches!(*name, "updateUserAddress" | "deleteUserAddress"))
            {
                assert_error(&registry, name, input, &both(foreign), |e| {
                    matches!(e, OperationError::NotFound)
                })
                .await;
            }
            assert_eq!(
                address,
                invoke(&registry, "getUserAddress", json!({"addressId":id}), &read).await
            );
            let write_only = context(owner, [CredentialCapability::UsersWrite]);
            for (name, input) in requests(id)
                .into_iter()
                .filter(|(name, _)| registry.get(name).unwrap().policy().read_only)
            {
                assert_error(&registry, name, input, &write_only, |e| {
                    matches!(e, OperationError::Forbidden)
                })
                .await;
            }
            for (name, input) in requests(id)
                .into_iter()
                .filter(|(name, _)| !registry.get(name).unwrap().policy().read_only)
            {
                assert_error(&registry, name, input, &read, |e| {
                    matches!(e, OperationError::Forbidden)
                })
                .await;
            }
            let mut replacement = input();
            replacement["label"] = json!("Replacement");
            replacement["geography"]["addressText"] = json!("Replacement private home");
            replacement["addressId"] = json!(id);
            replacement["expectedVersion"] = json!(1);
            let updated = invoke(&registry, "updateUserAddress", replacement.clone(), &ctx).await;
            assert_eq!(json!(2), updated["version"]);
            assert_eq!(json!(2), updated["inputRevision"]);
            assert_error(&registry, "updateUserAddress", replacement, &ctx, |e| {
                matches!(e, OperationError::Conflict(None))
            })
            .await;
            assert_error(
                &registry,
                "deleteUserAddress",
                json!({"addressId":id,"expectedVersion":1}),
                &ctx,
                |e| matches!(e, OperationError::Conflict(None)),
            )
            .await;
            exercise_destinations(&registry, &ctx, id, foreign_address["addressId"].clone()).await;
            exercise_invalid_inputs(&registry, &ctx, id).await;
            exercise_http_scopes(&runtime, owner, id).await;
            let selected = invoke(
                &registry,
                "setBrowsingDestination",
                json!({"expectedVersion":5,"destination":{"kind":"ADDRESS","addressId":id}}),
                &ctx,
            )
            .await;
            assert_eq!(json!(6), selected["version"]);
            assert_eq!(
                json!(true),
                invoke(&registry, "getUserAddress", json!({"addressId":id}), &ctx).await["preferred"]
            );
            assert_eq!(
                json!({"result":null}),
                invoke(
                    &registry,
                    "deleteUserAddress",
                    json!({"addressId":id,"expectedVersion":2}),
                    &ctx
                )
                .await
            );
            assert_eq!(
                json!({"version":7,"destination":{"kind":"NONE"}}),
                invoke(&registry, "getBrowsingDestination", json!({}), &ctx).await
            );
            assert_error(
                &registry,
                "getUserAddress",
                json!({"addressId":id}),
                &ctx,
                |e| matches!(e, OperationError::NotFound),
            )
            .await;
            assert_eq!(
                1,
                invoke(&registry, "listUserAddresses", json!({}), &ctx).await["items"]
                    .as_array()
                    .unwrap()
                    .len()
            );
            exercise_page_bounds(&registry, &ctx).await;
        }

        async fn exercise_page_bounds(registry: &Registry, ctx: &InvocationContext) {
            for _ in 0..25 {
                invoke(registry, "createUserAddress", input(), ctx).await;
            }
            let default_page = invoke(registry, "listUserAddresses", json!({}), ctx).await;
            assert_eq!(25, default_page["items"].as_array().unwrap().len());
            assert!(default_page["next"].is_string());
            assert_eq!(
                default_page,
                invoke(registry, "listUserAddresses", json!({"limit":null}), ctx).await
            );
            let last = invoke(
                registry,
                "listUserAddresses",
                json!({"after":default_page["next"]}),
                ctx,
            )
            .await;
            assert_eq!(1, last["items"].as_array().unwrap().len());
            assert!(last["next"].is_null());
            let complete = invoke(registry, "listUserAddresses", json!({"limit":100}), ctx).await;
            assert_eq!(26, complete["items"].as_array().unwrap().len());
            assert!(complete["next"].is_null());
            for limit in [101, u32::MAX] {
                assert_error(
                    registry,
                    "listUserAddresses",
                    json!({"limit":limit}),
                    ctx,
                    |e| matches!(e, OperationError::InvalidInput),
                )
                .await;
            }
        }

        async fn exercise_destinations(
            registry: &Registry,
            ctx: &InvocationContext,
            id: UserAddressId,
            foreign: Value,
        ) {
            assert_eq!(
                json!({"version":1,"destination":{"kind":"NONE"}}),
                invoke(registry, "getBrowsingDestination", json!({}), ctx).await
            );
            assert_error(
                registry,
                "setBrowsingDestination",
                json!({"expectedVersion":1,"destination":{"kind":"ADDRESS","addressId":foreign}}),
                ctx,
                |e| matches!(e, OperationError::NotFound),
            )
            .await;
            for (version, destination) in [
                (1, json!({"kind":"ADDRESS","addressId":id})),
                (
                    2,
                    json!({"kind":"COUNTRY_POSTAL","country":"DE","postalCode":"10115"}),
                ),
                (3, json!({"kind":"COUNTRY_POSTAL","country":"FR"})),
                (4, json!({"kind":"NONE"})),
            ] {
                let result = invoke(
                    registry,
                    "setBrowsingDestination",
                    json!({"expectedVersion":version,"destination":destination}),
                    ctx,
                )
                .await;
                assert_eq!(json!(version + 1), result["version"]);
                assert_eq!(destination["kind"], result["destination"]["kind"]);
                if version == 2 {
                    assert_eq!(destination, result["destination"]);
                }
            }
            assert_error(
                registry,
                "setBrowsingDestination",
                json!({"expectedVersion":1,"destination":{"kind":"NONE"}}),
                ctx,
                |e| matches!(e, OperationError::Conflict(None)),
            )
            .await;
        }
        async fn exercise_invalid_inputs(
            registry: &Registry,
            ctx: &InvocationContext,
            id: UserAddressId,
        ) {
            let mut country_only = input();
            country_only["geography"]
                .as_object_mut()
                .unwrap()
                .remove("addressText");
            assert_error(registry, "createUserAddress", country_only, ctx, |e| {
                matches!(e, OperationError::InvalidInput)
            })
            .await;
            let mut changed_owner = input();
            changed_owner["owner"] = json!(UserId::new());
            assert_error(registry, "createUserAddress", changed_owner, ctx, |e| {
                matches!(e, OperationError::InvalidArguments(_))
            })
            .await;
            for (name, input) in [
                ("getUserAddress", json!({"addressId":id.as_uuid()})),
                (
                    "updateUserAddress",
                    json!({"addressId":id,"expectedVersion":2,"label":"Home","geography":input()["geography"],"owner":UserId::new()}),
                ),
                (
                    "deleteUserAddress",
                    json!({"addressId":id,"expectedVersion":2,"userId":UserId::new()}),
                ),
                (
                    "setBrowsingDestination",
                    json!({"expectedVersion":5,"destination":{"kind":"NONE","country":"DE"}}),
                ),
                (
                    "setBrowsingDestination",
                    json!({"expectedVersion":5,"destination":{"kind":"COUNTRY_POSTAL","country":"XX"}}),
                ),
            ] {
                assert_error(registry, name, input, ctx, |e| {
                    matches!(e, OperationError::InvalidArguments(_))
                })
                .await;
            }
            assert_error(
                registry,
                "listUserAddresses",
                json!({"limit":0}),
                ctx,
                |e| matches!(e, OperationError::InvalidInput),
            )
            .await;
            for version in [0, i64::MAX as u64 + 1, u64::MAX] {
                for (name, input) in [
                    (
                        "updateUserAddress",
                        json!({"addressId":id,"expectedVersion":version,"label":"Home","geography":input()["geography"]}),
                    ),
                    (
                        "deleteUserAddress",
                        json!({"addressId":id,"expectedVersion":version}),
                    ),
                    (
                        "setBrowsingDestination",
                        json!({"expectedVersion":version,"destination":{"kind":"NONE"}}),
                    ),
                ] {
                    let schema = serde_json::to_value(
                        registry.get(name).unwrap().tool().input_schema.as_ref(),
                    )
                    .unwrap();
                    assert!(!jsonschema::validator_for(&schema).unwrap().is_valid(&input));
                    assert_error(registry, name, input, ctx, |e| {
                        matches!(e, OperationError::InvalidInput)
                    })
                    .await;
                }
            }
        }

        struct ReadToken(UserId);
        #[async_trait::async_trait]
        impl AuthenticateAccessTokenUseCase for ReadToken {
            async fn execute(
                &self,
                ctx: &OperationContext,
                _: AuthenticateAccessTokenRequest,
            ) -> Result<AuthenticateAccessTokenResult, AuthenticateAccessTokenError> {
                assert_eq!(Principal::Anonymous, ctx.principal);
                Ok(AuthenticateAccessTokenResult {
                    user_id: self.0,
                    scopes: [Scope::UsersRead].into(),
                })
            }
        }
        async fn exercise_http_scopes(runtime: &Runtime, owner: UserId, id: UserAddressId) {
            let config = TransportConfig {
                allowed_hosts: vec!["mcp.example.test".into()],
                ..Default::default()
            };
            let app = crate::router(
                config.clone(),
                registry(runtime),
                Arc::new(AuraTokenGate::new(
                    Arc::new(ReadToken(owner)),
                    config.resource_policy,
                )),
            )
            .unwrap();
            let token = String::from(RawAccessToken::new());
            for (name, input) in requests(id) {
                let mut params = json!({"name":name,"arguments":input});
                params["_meta"] = json!({"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientInfo":{"name":"account-fixture","version":"1"},"io.modelcontextprotocol/clientCapabilities":{}});
                let body = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":params});
                let request = || {
                    Request::builder()
                        .method("POST")
                        .uri("/v1")
                        .header("host", "mcp.example.test")
                        .header("content-type", "application/json")
                        .header("accept", "application/json,text/event-stream")
                        .header("mcp-protocol-version", "2026-07-28")
                        .header("mcp-method", "tools/call")
                        .header("mcp-name", name)
                };
                let missing = app
                    .clone()
                    .oneshot(request().body(Body::from(body.to_string())).unwrap())
                    .await
                    .unwrap();
                assert_eq!(StatusCode::UNAUTHORIZED, missing.status());
                let response = app
                    .clone()
                    .oneshot(
                        request()
                            .header("authorization", format!("Bearer {token}"))
                            .body(Body::from(body.to_string()))
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                assert_eq!("no-store", response.headers()["cache-control"]);
                let read = registry(runtime).get(name).unwrap().policy().read_only;
                if read {
                    assert_eq!(StatusCode::OK, response.status());
                    let result: Value = serde_json::from_slice(
                        &to_bytes(response.into_body(), MAX_RESPONSE_BYTES)
                            .await
                            .unwrap(),
                    )
                    .unwrap();
                    assert!(result.get("error").is_none(), "{result}");
                    assert_ne!(json!(true), result["result"]["isError"], "{result}");
                } else {
                    assert_eq!(StatusCode::FORBIDDEN, response.status());
                    assert!(
                        response.headers()["www-authenticate"]
                            .to_str()
                            .unwrap()
                            .contains("scope=\"users:write\"")
                    );
                }
            }
        }
    }
}
