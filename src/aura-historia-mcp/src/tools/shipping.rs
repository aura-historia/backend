//! Shipping profile/default operations call the existing authorized use cases in process.
pub(super) mod errors;
use crate::{AccessClass, OperationDefinition, OperationError, OperationPolicy, RegistryError};
use application::operation_context::CredentialCapability;
use aura_historia_contracts::shipping::management::{
    self, AssignData, ClearData, CreateData, DefaultData, LifecycleData, ListQuery, ProfileData,
    ReplaceData, SummaryData,
};
use aura_historia_runtime::ShippingManagement;
use listing_source_core::ListingSourceId;
use schemars::JsonSchema;
use serde::Deserialize;
use shipping_core::profile::ShippingProfileId;
use shipping_service::use_cases::{
    AssignShippingDefaultUseCase, ClearShippingDefaultUseCase, CreateShippingProfileUseCase,
    GetShippingDefaultUseCase, GetShippingProfileUseCase, ListShippingProfilesUseCase,
    ReplaceShippingProfileUseCase, RestoreShippingProfileUseCase, RetireShippingProfileUseCase,
};
use std::sync::Arc;

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SourceArguments {
    #[schemars(schema_with = "management::source_id")]
    listing_source_id: ListingSourceId,
}
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProfileArguments {
    #[schemars(schema_with = "management::source_id")]
    listing_source_id: ListingSourceId,
    #[schemars(schema_with = "management::profile_id")]
    shipping_profile_id: ShippingProfileId,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateArguments {
    #[schemars(schema_with = "management::source_id")]
    listing_source_id: ListingSourceId,
    #[serde(flatten)]
    data: CreateData,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReplaceArguments {
    #[schemars(schema_with = "management::source_id")]
    listing_source_id: ListingSourceId,
    #[schemars(schema_with = "management::profile_id")]
    shipping_profile_id: ShippingProfileId,
    #[serde(flatten)]
    data: ReplaceData,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LifecycleArguments {
    #[schemars(schema_with = "management::source_id")]
    listing_source_id: ListingSourceId,
    #[schemars(schema_with = "management::profile_id")]
    shipping_profile_id: ShippingProfileId,
    #[serde(flatten)]
    data: LifecycleData,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AssignArguments {
    #[schemars(schema_with = "management::source_id")]
    listing_source_id: ListingSourceId,
    #[serde(flatten)]
    data: AssignData,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ClearArguments {
    #[schemars(schema_with = "management::source_id")]
    listing_source_id: ListingSourceId,
    #[serde(flatten)]
    data: ClearData,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ListArguments {
    #[schemars(schema_with = "management::source_id")]
    listing_source_id: ListingSourceId,
    #[serde(flatten)]
    data: ListQuery,
}

fn read_policy() -> OperationPolicy {
    OperationPolicy {
        access: AccessClass::Protected,
        required_scopes: vec![CredentialCapability::ListingSourcesRead],
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
    }
}
fn write_policy(destructive: bool) -> OperationPolicy {
    OperationPolicy {
        access: AccessClass::Protected,
        required_scopes: vec![CredentialCapability::ListingSourcesWrite],
        read_only: false,
        destructive,
        idempotent: true,
        open_world: false,
    }
}
/// Shipping management family for explicit composition into the operation registry.
pub fn operations(services: ShippingManagement) -> Result<Vec<OperationDefinition>, RegistryError> {
    Ok(vec![
        list(services.list)?,
        create(services.create)?,
        get(services.get)?,
        replace(services.replace)?,
        retire(services.retire)?,
        restore(services.restore)?,
        get_default(services.get_default)?,
        assign(services.assign_default)?,
        clear(services.clear_default)?,
    ])
}
fn list(
    service: Arc<dyn ListShippingProfilesUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "listShippingProfiles",
        "List shipping profiles, including retired profiles. objectKey and optional integrationMethod resolve the caller's import identity. after continues by shippingProfileId; size defaults to 21 and must be 1 through 100.",
        read_policy(),
        move |input: ListArguments, invocation| {
            let service = service.clone();
            async move {
                let after = input.data.after;
                let size = input.data.size.unwrap_or(21);
                let identity = input
                    .data
                    .into_identity()
                    .map_err(OperationError::InvalidArguments)?;
                let result = invocation
                    .execute(service.execute(
                        &invocation.operation,
                        input.listing_source_id,
                        after,
                        size,
                        identity,
                    ))
                    .await?
                    .map_err(errors::shipping)?;
                Ok(result
                    .into_iter()
                    .map(SummaryData::from)
                    .collect::<Vec<_>>())
            }
        },
    )
}
fn get(service: Arc<dyn GetShippingProfileUseCase>) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "getShippingProfile",
        "Read complete shipping profile terms, managementRevision, termsRevision and lifecycle for an authorized source.",
        read_policy(),
        move |input: ProfileArguments, invocation| {
            let service = service.clone();
            async move {
                let result = invocation
                    .execute(service.execute(
                        &invocation.operation,
                        input.listing_source_id,
                        input.shipping_profile_id,
                    ))
                    .await?
                    .map_err(errors::shipping)?;
                ProfileData::try_from(result).map_err(OperationError::Internal)
            }
        },
    )
}
fn get_default(
    service: Arc<dyn GetShippingDefaultUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "getShippingDefault",
        "Read the source's default shipping profile and its ACTIVE, RETIRED or MISSING eligibility. A cleared default has null profile and eligibility.",
        read_policy(),
        move |input: SourceArguments, invocation| {
            let service = service.clone();
            async move {
                let result = invocation
                    .execute(service.execute(&invocation.operation, input.listing_source_id))
                    .await?
                    .map_err(errors::shipping)?;
                Ok(DefaultData::from(result))
            }
        },
    )
}

fn create(
    service: Arc<dyn CreateShippingProfileUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "createShippingProfile",
        "Create a complete shipping profile. creationKey is caller/source scoped: retry identical creation content and evidence with the same key; different content conflicts. Include import provenance for typed imports. attributedPartyId is required and may be null.",
        write_policy(false),
        move |input: CreateArguments, invocation| {
            let service = service.clone();
            async move {
                let command = input
                    .data
                    .into_command(input.listing_source_id)
                    .map_err(OperationError::InvalidArguments)?;
                let result = invocation
                    .execute(service.execute(&invocation.operation, command))
                    .await?
                    .map_err(errors::shipping)?;
                ProfileData::try_from(result).map_err(OperationError::Internal)
            }
        },
    )
}

fn replace(
    service: Arc<dyn ReplaceShippingProfileUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "replaceShippingProfile",
        "Replace complete shipping profile content and evidence with expectedRevision. managementRevision and termsRevision are distinct. After an ambiguous result retry identical content, keys and revision; conflicts require refetch and reconciliation.",
        write_policy(true),
        move |input: ReplaceArguments, invocation| {
            let service = service.clone();
            async move {
                let command = input
                    .data
                    .into_command(input.listing_source_id, input.shipping_profile_id)
                    .map_err(OperationError::InvalidArguments)?;
                let result = invocation
                    .execute(service.execute(&invocation.operation, command))
                    .await?
                    .map_err(errors::shipping)?;
                ProfileData::try_from(result).map_err(OperationError::Internal)
            }
        },
    )
}

fn retire(
    service: Arc<dyn RetireShippingProfileUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "retireShippingProfile",
        "Retire a shipping profile using expectedRevision and optional import provenance. Existing defaults retain their reference and report RETIRED eligibility. A stale retry conflicts; refetch and reconcile.",
        write_policy(true),
        move |input: LifecycleArguments, invocation| {
            let service = service.clone();
            async move {
                let command = input
                    .data
                    .into_command(input.listing_source_id, input.shipping_profile_id)
                    .map_err(OperationError::InvalidArguments)?;
                let result = invocation
                    .execute(service.execute(&invocation.operation, command))
                    .await?
                    .map_err(errors::shipping)?;
                ProfileData::try_from(result).map_err(OperationError::Internal)
            }
        },
    )
}

fn restore(
    service: Arc<dyn RestoreShippingProfileUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "restoreShippingProfile",
        "Restore a retired shipping profile using expectedRevision and optional import provenance. A stale retry conflicts; refetch and reconcile.",
        write_policy(false),
        move |input: LifecycleArguments, invocation| {
            let service = service.clone();
            async move {
                let command = input
                    .data
                    .into_command(input.listing_source_id, input.shipping_profile_id)
                    .map_err(OperationError::InvalidArguments)?;
                let result = invocation
                    .execute(service.execute(&invocation.operation, command))
                    .await?
                    .map_err(errors::shipping)?;
                ProfileData::try_from(result).map_err(OperationError::Internal)
            }
        },
    )
}

fn assign(
    service: Arc<dyn AssignShippingDefaultUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "assignShippingDefault",
        "Assign an active profile as source default using its shippingProfileId, the default expectedRevision and optional import provenance. Equal assignments still validate current source/profile/location authority.",
        write_policy(true),
        move |input: AssignArguments, invocation| {
            let service = service.clone();
            async move {
                let command = input
                    .data
                    .into_command(input.listing_source_id)
                    .map_err(OperationError::InvalidArguments)?;
                let result = invocation
                    .execute(service.execute(&invocation.operation, command))
                    .await?
                    .map_err(errors::shipping)?;
                Ok(DefaultData::from(result))
            }
        },
    )
}

fn clear(
    service: Arc<dyn ClearShippingDefaultUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "clearShippingDefault",
        "Clear the source shipping default using expectedRevision and optional import provenance. The retained revision fences stale assignments; a manual clear protects the assertion from later imports.",
        write_policy(true),
        move |input: ClearArguments, invocation| {
            let service = service.clone();
            async move {
                let command = input
                    .data
                    .into_command(input.listing_source_id)
                    .map_err(OperationError::InvalidArguments)?;
                let result = invocation
                    .execute(service.execute(&invocation.operation, command))
                    .await?
                    .map_err(errors::shipping)?;
                Ok(DefaultData::from(result))
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{InvocationContext, MAX_RESPONSE_BYTES, Registry};
    use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
    use aura_historia_runtime::Runtime;
    use serde_json::{Value, json};
    use tokio_util::sync::CancellationToken;
    use user_core::user_id::UserId;
    fn context(
        user: UserId,
        scopes: impl IntoIterator<Item = CredentialCapability>,
    ) -> InvocationContext {
        InvocationContext {
            operation: OperationContext {
                principal: Principal::DelegatedUser {
                    user_id: user,
                    capabilities: scopes.into_iter().collect(),
                },
                request_id: RequestId::new("shipping-test"),
                correlation_id: CorrelationId::new("shipping-test"),
            },
            deadline: tokio::time::Instant::now() + std::time::Duration::from_secs(30),
            cancellation: CancellationToken::new(),
            max_response_bytes: MAX_RESPONSE_BYTES,
        }
    }
    fn both(user: UserId) -> InvocationContext {
        context(
            user,
            [
                CredentialCapability::ListingSourcesRead,
                CredentialCapability::ListingSourcesWrite,
            ],
        )
    }
    fn registry(runtime: &Runtime) -> Registry {
        Registry::new(operations(runtime.shipping_management().clone()).unwrap()).unwrap()
    }
    fn content(amount: u64) -> Value {
        json!({"name":"Shipping MCP terms","attributedPartyId":null,"terms":aura_historia_contracts::shipping::terms::encode(&shipping_core::fixtures::gallery_terms(amount)).unwrap()})
    }
    fn creation(source: ListingSourceId, key: &str) -> Value {
        json!({"listingSourceId":source,"creationKey":key,"content":content(8000),"evidence":[{"reference":"https://example.test/protected","assertion":"ADVERTISED"}]})
    }
    async fn invoke(
        registry: &Registry,
        name: &str,
        input: Value,
        ctx: &InvocationContext,
    ) -> Value {
        let op = registry.get(name).unwrap();
        let validator =
            jsonschema::validator_for(&serde_json::to_value(&op.tool().input_schema).unwrap())
                .unwrap();
        assert!(
            validator.is_valid(&input),
            "{name}: {input}: {:?}",
            validator
                .iter_errors(&input)
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
        );
        let result = op
            .call(input, ctx.clone())
            .await
            .unwrap_or_else(|e| panic!("{name}: {e:?}"));
        let output = result.structured_content.unwrap();
        let validator =
            jsonschema::validator_for(&serde_json::to_value(&op.tool().output_schema).unwrap())
                .unwrap();
        assert!(
            validator.is_valid(&output),
            "{name}: {output}: {:?}",
            validator
                .iter_errors(&output)
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
        );
        output
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
    #[tokio::test]
    async fn should_publish_all_nine_operations_with_matching_schemas_scopes_and_retry_annotations()
    {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://fixture:fixture@127.0.0.1:9/fixture")
            .unwrap();
        let runtime = Runtime::from_pool(pool);
        assert!(Arc::ptr_eq(
            &runtime.shipping_management().create,
            &runtime.shipping_management().create
        ));
        let registry = registry(&runtime);
        let manifest: Value =
            serde_json::from_str(include_str!("../../partner-operations.json")).unwrap();
        let entries: Vec<_> = manifest["operations"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["family"] == "shipping")
            .collect();
        assert_eq!(9, entries.len());
        let source = ListingSourceId::new();
        let profile = ShippingProfileId::new();
        let user = UserId::new();
        for entry in entries {
            let name = entry["operationId"].as_str().unwrap();
            let op = registry.get(name).unwrap();
            assert_eq!(entry["name"], op.name());
            assert_eq!(AccessClass::Protected, op.policy().access);
            assert_eq!(
                entry["annotations"],
                serde_json::to_value(op.tool().annotations.as_ref().unwrap()).unwrap()
            );
            assert_eq!(
                entry["requiredScopes"],
                json!(
                    op.policy()
                        .required_scopes
                        .iter()
                        .map(|v| v.as_str())
                        .collect::<Vec<_>>()
                )
            );
            let mut input = match name {
                "createShippingProfile" => creation(source, "create"),
                "replaceShippingProfile" => {
                    json!({"listingSourceId":source,"shippingProfileId":profile,"expectedRevision":1,"content":content(8000),"evidence":[]})
                }
                "retireShippingProfile" | "restoreShippingProfile" => {
                    json!({"listingSourceId":source,"shippingProfileId":profile,"expectedRevision":1})
                }
                "getShippingProfile" => {
                    json!({"listingSourceId":source,"shippingProfileId":profile})
                }
                "assignShippingDefault" => {
                    json!({"listingSourceId":source,"shippingProfileId":profile,"expectedRevision":1})
                }
                "clearShippingDefault" => json!({"listingSourceId":source,"expectedRevision":1}),
                _ => json!({"listingSourceId":source}),
            };
            let validator =
                jsonschema::validator_for(&serde_json::to_value(&op.tool().input_schema).unwrap())
                    .unwrap();
            assert!(validator.is_valid(&input), "{name}: {input}");
            // Scope admission is also enforced by the actual use case before touching PostgreSQL.
            assert_error(&registry, name, input.clone(), &context(user, []), |e| {
                matches!(e, OperationError::Forbidden)
            })
            .await;
            input["listingSourceId"] = json!(source.into_uuid().to_string());
            assert!(!validator.is_valid(&input));
            assert_error(&registry, name, input, &both(user), |e| {
                matches!(e, OperationError::InvalidArguments(_))
            })
            .await;
        }
        let mut invalid = creation(source, "missing-attribution");
        invalid["content"]
            .as_object_mut()
            .unwrap()
            .remove("attributedPartyId");
        assert_error(
            &registry,
            "createShippingProfile",
            invalid,
            &both(user),
            |e| matches!(e, OperationError::InvalidArguments(_)),
        )
        .await;
    }
    #[test]
    fn should_preserve_technical_error_causes_and_safe_protocol_classifications() {
        for (unavailable, error) in [
            (
                true,
                shipping_service::ShippingError::PersistenceUnavailable {
                    source: application::error::static_error("private persistence failure"),
                },
            ),
            (
                false,
                shipping_service::ShippingError::CorruptShippingState {
                    source: application::error::static_error("private corruption"),
                },
            ),
        ] {
            let mapped = errors::shipping(error);
            assert!(std::error::Error::source(&mapped).is_some());
            assert!(!mapped.to_string().contains("private"));
            assert_eq!(
                unavailable,
                matches!(mapped, OperationError::Unavailable(_))
            );
            let protocol = format!("{:?}", mapped.into_protocol());
            assert!(!protocol.contains("private"));
        }
    }

    mod postgres {
        use super::*;
        use party_core::party_id::PartyId;
        use test_api::{
            IntegrationTestService, Postgres, aura_integration_test, get_postgres_client,
        };
        const SCHEMA: Postgres = Postgres::new("migrations");
        async fn seed(pool: &sqlx::PgPool) -> (PartyId, ListingSourceId, UserId) {
            let owner = PartyId::new();
            let source = ListingSourceId::new();
            let user = UserId::new();
            let partnership = uuid::Uuid::now_v7();
            sqlx::query("INSERT INTO users(user_id,email,tier,role) VALUES($1,$2,'FREE','USER')")
                .bind(user.into_uuid())
                .bind(format!("{user}@example.test"))
                .execute(pool)
                .await
                .unwrap();
            sqlx::query(
                "INSERT INTO parties(party_id,party_slug_id,name) VALUES($1,$2,'MCP Gallery')",
            )
            .bind(owner.into_uuid())
            .bind(format!("gallery-{}", owner.as_uuid().simple()))
            .execute(pool)
            .await
            .unwrap();
            sqlx::query("INSERT INTO listing_sources(listing_source_id,listing_source_slug_id,name,operator_party_id) VALUES($1,$2,'MCP Gallery',$3)").bind(source.into_uuid()).bind(format!("source-{}", source.as_uuid().simple())).bind(owner.into_uuid()).execute(pool).await.unwrap();
            sqlx::query("INSERT INTO partnerships(partnership_id,party_id) VALUES($1,$2)")
                .bind(partnership)
                .bind(owner.into_uuid())
                .execute(pool)
                .await
                .unwrap();
            sqlx::query("INSERT INTO partnership_members(partnership_id,user_id) VALUES($1,$2)")
                .bind(partnership)
                .bind(user.into_uuid())
                .execute(pool)
                .await
                .unwrap();
            sqlx::query("INSERT INTO partnership_listing_source_grants(partnership_id,listing_source_id) VALUES($1,$2)").bind(partnership).bind(source.into_uuid()).execute(pool).await.unwrap();
            (owner, source, user)
        }
        #[aura_integration_test(services=[SCHEMA],isolated=true)]
        async fn should_share_shipping_revision_creation_retry_lifecycle_evidence_and_paging_behavior()
         {
            let pool = get_postgres_client().await;
            let (_, source, user) = seed(&pool).await;
            let runtime = Runtime::from_pool(pool.clone());
            let registry = registry(&runtime);
            let ctx = both(user);
            let first = invoke(
                &registry,
                "createShippingProfile",
                creation(source, "first"),
                &ctx,
            )
            .await;
            assert_eq!(
                first,
                invoke(
                    &registry,
                    "createShippingProfile",
                    creation(source, "first"),
                    &ctx
                )
                .await
            );
            let id = first["shippingProfileId"].clone();
            let mut conflict = creation(source, "first");
            conflict["content"] = content(9500);
            assert_error(&registry, "createShippingProfile", conflict, &ctx, |e| {
                matches!(e, OperationError::Conflict(None))
            })
            .await;
            let details = json!({"listingSourceId":source,"shippingProfileId":id});
            assert_eq!(
                first,
                invoke(&registry, "getShippingProfile", details.clone(), &ctx).await
            );
            // Evidence changes advance management independently of semantic terms revisions.
            let replacement = json!({"listingSourceId":source,"shippingProfileId":id,"expectedRevision":1,"content":content(8000),"evidence":[{"reference":"https://example.test/new-evidence","assertion":"CORRECTED"}]});
            let updated = invoke(
                &registry,
                "replaceShippingProfile",
                replacement.clone(),
                &ctx,
            )
            .await;
            assert_eq!(json!(2), updated["managementRevision"]);
            assert_eq!(json!(1), updated["termsRevision"]);
            assert_error(
                &registry,
                "replaceShippingProfile",
                replacement,
                &ctx,
                |e| matches!(e, OperationError::Conflict(None)),
            )
            .await;
            let observed:String=sqlx::query_scalar("SELECT evidence->0->>'reference' FROM shipping_profile_evidence WHERE shipping_profile_id=$1 AND management_revision=2").bind(id.as_str().unwrap().parse::<ShippingProfileId>().unwrap().into_uuid()).fetch_one(&pool).await.unwrap();
            assert_eq!("https://example.test/new-evidence", observed);
            let default = invoke(
                &registry,
                "getShippingDefault",
                json!({"listingSourceId":source}),
                &ctx,
            )
            .await;
            assert!(default["shippingProfileId"].is_null());
            let assigned=invoke(&registry,"assignShippingDefault",json!({"listingSourceId":source,"shippingProfileId":id,"expectedRevision":default["revision"]}),&ctx).await;
            assert_eq!(json!("ACTIVE"), assigned["eligibility"]);
            let retired = invoke(
                &registry,
                "retireShippingProfile",
                json!({"listingSourceId":source,"shippingProfileId":id,"expectedRevision":2}),
                &ctx,
            )
            .await;
            assert_eq!(json!("RETIRED"), retired["lifecycle"]);
            assert_eq!(
                json!("RETIRED"),
                invoke(
                    &registry,
                    "getShippingDefault",
                    json!({"listingSourceId":source}),
                    &ctx
                )
                .await["eligibility"]
            );
            assert_error(&registry,"assignShippingDefault",json!({"listingSourceId":source,"shippingProfileId":id,"expectedRevision":assigned["revision"]}),&ctx,|e|matches!(e,OperationError::Conflict(None))).await;
            let restored=invoke(&registry,"restoreShippingProfile",json!({"listingSourceId":source,"shippingProfileId":id,"expectedRevision":retired["managementRevision"]}),&ctx).await;
            assert_eq!(json!("ACTIVE"), restored["lifecycle"]);
            let cleared = invoke(
                &registry,
                "clearShippingDefault",
                json!({"listingSourceId":source,"expectedRevision":assigned["revision"]}),
                &ctx,
            )
            .await;
            assert!(cleared["shippingProfileId"].is_null());
            let second = invoke(
                &registry,
                "createShippingProfile",
                creation(source, "second"),
                &ctx,
            )
            .await;
            let page = invoke(
                &registry,
                "listShippingProfiles",
                json!({"listingSourceId":source,"size":1}),
                &ctx,
            )
            .await;
            assert_eq!(1, page["result"].as_array().unwrap().len());
            let next = invoke(
        &registry,
        "listShippingProfiles",
        json!({"listingSourceId":source,"after":page["result"][0]["shippingProfileId"],"size":100}),
        &ctx,
    )
    .await;
            assert_eq!(1, next["result"].as_array().unwrap().len());
            assert_ne!(
                page["result"][0]["shippingProfileId"],
                next["result"][0]["shippingProfileId"]
            );
            assert_eq!(json!(1), second["termsRevision"]);
        }
        #[aura_integration_test(services=[SCHEMA],isolated=true)]
        async fn should_recheck_delegated_scopes_operator_grants_account_status_and_attributed_party()
         {
            let pool = get_postgres_client().await;
            let (_, source, user) = seed(&pool).await;
            let (_, foreign, _) = seed(&pool).await;
            let runtime = Runtime::from_pool(pool.clone());
            let registry = registry(&runtime);
            let ctx = both(user);
            assert_error(
                &registry,
                "createShippingProfile",
                creation(foreign, "foreign"),
                &ctx,
                |e| matches!(e, OperationError::Forbidden),
            )
            .await;
            let mut attributed = creation(source, "unknown-party");
            attributed["content"]["attributedPartyId"] = json!(PartyId::new());
            assert_error(&registry, "createShippingProfile", attributed, &ctx, |e| {
                matches!(e, OperationError::NotFound)
            })
            .await;
            assert_error(
                &registry,
                "createShippingProfile",
                creation(source, "read-only"),
                &context(user, [CredentialCapability::ListingSourcesRead]),
                |e| matches!(e, OperationError::Forbidden),
            )
            .await;
            let result = invoke(
                &registry,
                "createShippingProfile",
                creation(source, "authorized"),
                &ctx,
            )
            .await;
            let input =
                json!({"listingSourceId":source,"shippingProfileId":result["shippingProfileId"]});
            assert_error(
                &registry,
                "getShippingProfile",
                input.clone(),
                &context(user, [CredentialCapability::ListingSourcesWrite]),
                |e| matches!(e, OperationError::Forbidden),
            )
            .await;
            let mut anonymous = ctx.clone();
            anonymous.operation.principal = Principal::Anonymous;
            assert_error(
                &registry,
                "getShippingProfile",
                input.clone(),
                &anonymous,
                |e| matches!(e, OperationError::Forbidden),
            )
            .await;
            sqlx::query("DELETE FROM partnership_listing_source_grants WHERE listing_source_id=$1")
                .bind(source.into_uuid())
                .execute(&pool)
                .await
                .unwrap();
            assert_error(&registry, "getShippingProfile", input.clone(), &ctx, |e| {
                matches!(e, OperationError::Forbidden)
            })
            .await;
            sqlx::query("UPDATE users SET role='ADMIN' WHERE user_id=$1")
                .bind(user.into_uuid())
                .execute(&pool)
                .await
                .unwrap();
            assert_eq!(
                result,
                invoke(&registry, "getShippingProfile", input.clone(), &ctx).await
            );
            sqlx::query("UPDATE users SET suspended=true WHERE user_id=$1")
                .bind(user.into_uuid())
                .execute(&pool)
                .await
                .unwrap();
            assert_error(&registry, "getShippingProfile", input, &ctx, |e| {
                matches!(e, OperationError::Forbidden)
            })
            .await;
        }
        #[aura_integration_test(services=[SCHEMA],isolated=true)]
        async fn should_preserve_import_identity_exact_replay_provenance_and_current_authority() {
            let pool = get_postgres_client().await;
            let (_, source, user) = seed(&pool).await;
            let runtime = Runtime::from_pool(pool.clone());
            let registry = registry(&runtime);
            let ctx = both(user);
            // Acquire after the source/membership authority boundary.
            let acquired: time::OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
                .fetch_one(&pool)
                .await
                .unwrap();
            let mut input = creation(source, "import-create");
            input["import"] = json!({"integration":{"kind":"SOURCE"},"sequence":1,"acquiredAt":acquired.format(&time::format_description::well_known::Rfc3339).unwrap(),"requestKey":"request-1","objectKey":"policy-1","observation":{}});
            let result = invoke(&registry, "createShippingProfile", input.clone(), &ctx).await;
            assert_eq!(
                result,
                invoke(&registry, "createShippingProfile", input.clone(), &ctx).await
            );
            let found = invoke(
                &registry,
                "listShippingProfiles",
                json!({"listingSourceId":source,"objectKey":"policy-1","size":100}),
                &ctx,
            )
            .await;
            assert_eq!(
                result["shippingProfileId"],
                found["result"][0]["shippingProfileId"]
            );
            let mut changed = input.clone();
            changed["content"] = content(9500);
            assert_error(&registry, "createShippingProfile", changed, &ctx, |e| {
                matches!(e, OperationError::Conflict(None))
            })
            .await;
            assert_error(
                &registry,
                "listShippingProfiles",
                json!({"listingSourceId":source,"integrationMethod":"SHOPIFY"}),
                &ctx,
                |e| matches!(e, OperationError::InvalidArguments(_)),
            )
            .await;
            sqlx::query("DELETE FROM partnership_listing_source_grants WHERE listing_source_id=$1")
                .bind(source.into_uuid())
                .execute(&pool)
                .await
                .unwrap();
            assert_error(&registry, "createShippingProfile", input, &ctx, |e| {
                matches!(e, OperationError::Forbidden)
            })
            .await;
        }
    }
}
