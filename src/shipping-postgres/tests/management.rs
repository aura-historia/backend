use application::operation_context::{CredentialCapability, OperationContext, Principal};
use listing_source_core::ListingSourceId;
use listing_source_postgres::SqlxSourceLocationAccessFactory;
use party_core::{party_id::PartyId, party_location_id::PartyLocationId};
use platform_postgres::SqlxUnitOfWork;
use shipping_core::{fixtures::gallery_terms, inputs::Applicability, profile::*, terms::*};
use shipping_postgres::{SqlxShippingReaderFactory, SqlxShippingRepositoryFactory};
use shipping_service::{ShippingError, ports::*, use_cases::*};
use sqlx::PgPool;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
const SCHEMA: Postgres = Postgres::new("migrations");
struct Manager {
    create: std::sync::Arc<dyn CreateShippingProfileUseCase>,
    replace: std::sync::Arc<dyn ReplaceShippingProfileUseCase>,
    retire: std::sync::Arc<dyn RetireShippingProfileUseCase>,
    restore: std::sync::Arc<dyn RestoreShippingProfileUseCase>,
    assign_default: std::sync::Arc<dyn AssignShippingDefaultUseCase>,
    clear_default: std::sync::Arc<dyn ClearShippingDefaultUseCase>,
    get: std::sync::Arc<dyn GetShippingProfileUseCase>,
    list: std::sync::Arc<dyn ListShippingProfilesUseCase>,
    get_default: std::sync::Arc<dyn GetShippingDefaultUseCase>,
}
fn manager(p: &PgPool) -> Manager {
    Manager {
        create: std::sync::Arc::new(CreateShippingProfileHandler::new(
            SqlxUnitOfWork::new(p.clone()),
            SqlxShippingRepositoryFactory,
            SqlxSourceLocationAccessFactory,
        )),
        replace: std::sync::Arc::new(ReplaceShippingProfileHandler::new(
            SqlxUnitOfWork::new(p.clone()),
            SqlxShippingRepositoryFactory,
            SqlxSourceLocationAccessFactory,
        )),
        retire: std::sync::Arc::new(RetireShippingProfileHandler::new(
            SqlxUnitOfWork::new(p.clone()),
            SqlxShippingRepositoryFactory,
            SqlxSourceLocationAccessFactory,
        )),
        restore: std::sync::Arc::new(RestoreShippingProfileHandler::new(
            SqlxUnitOfWork::new(p.clone()),
            SqlxShippingRepositoryFactory,
            SqlxSourceLocationAccessFactory,
        )),
        assign_default: std::sync::Arc::new(AssignShippingDefaultHandler::new(
            SqlxUnitOfWork::new(p.clone()),
            SqlxShippingRepositoryFactory,
            SqlxSourceLocationAccessFactory,
        )),
        clear_default: std::sync::Arc::new(ClearShippingDefaultHandler::new(
            SqlxUnitOfWork::new(p.clone()),
            SqlxShippingRepositoryFactory,
            SqlxSourceLocationAccessFactory,
        )),
        get: std::sync::Arc::new(GetShippingProfileHandler::new(
            SqlxUnitOfWork::new(p.clone()),
            SqlxShippingReaderFactory,
            SqlxSourceLocationAccessFactory,
        )),
        list: std::sync::Arc::new(ListShippingProfilesHandler::new(
            SqlxUnitOfWork::new(p.clone()),
            SqlxShippingReaderFactory,
            SqlxSourceLocationAccessFactory,
        )),
        get_default: std::sync::Arc::new(GetShippingDefaultHandler::new(
            SqlxUnitOfWork::new(p.clone()),
            SqlxShippingReaderFactory,
            SqlxSourceLocationAccessFactory,
        )),
    }
}
fn context(principal: Principal) -> OperationContext {
    OperationContext {
        principal,
        request_id: "shipping-test".into(),
        correlation_id: "shipping-test".into(),
    }
}
fn trusted() -> OperationContext {
    context(Principal::Service("shipping-test".into()))
}
async fn source(p: &PgPool) -> (PartyId, ListingSourceId) {
    let owner = PartyId::new();
    let source = ListingSourceId::new();
    sqlx::query("INSERT INTO parties(party_id,party_slug_id,name) VALUES($1,$2,'Gallery')")
        .bind(owner.into_uuid())
        .bind(format!("gallery-{}", owner.as_uuid().simple()))
        .execute(p)
        .await
        .unwrap();
    sqlx::query("INSERT INTO listing_sources(listing_source_id,listing_source_slug_id,name,operator_party_id) VALUES($1,$2,'Gallery',$3)").bind(source.into_uuid()).bind(format!("gallery-{}",source.as_uuid().simple())).bind(owner.into_uuid()).execute(p).await.unwrap();
    (owner, source)
}
fn content(amount: u64) -> ShippingProfileContent {
    ShippingProfileContent {
        name: ShippingProfileName::new("Gallery advertised terms".into()).unwrap(),
        attributed_party: None,
        terms: gallery_terms(amount),
    }
}
fn evidence() -> Vec<ShippingEvidence> {
    vec![
        ShippingEvidence::new(
            "https://example.test/protected-policy".into(),
            "EXPLICIT_ADVERTISED_TERMS".into(),
        )
        .unwrap(),
    ]
}
fn command(source: ListingSourceId) -> CreateProfileCommand {
    CreateProfileCommand {
        source_id: source,
        creation_key: "gallery-profile".into(),
        content: content(8000),
        evidence: evidence(),
    }
}
fn replacement(v: &ProfileView, amount: u64) -> ReplaceProfileCommand {
    ReplaceProfileCommand {
        source_id: v.source_id,
        profile_id: v.id,
        expected_revision: v.management_revision,
        content: content(amount),
        evidence: evidence(),
    }
}
fn lifecycle(v: &ProfileView) -> ProfileLifecycleCommand {
    ProfileLifecycleCommand {
        source_id: v.source_id,
        profile_id: v.id,
        expected_revision: v.management_revision,
    }
}
async fn signals(p: &PgPool, source: ListingSourceId) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM shipping_changes WHERE listing_source_id=$1")
        .bind(source.into_uuid())
        .fetch_one(p)
        .await
        .unwrap()
}

#[aura_integration_test(services=[SCHEMA])]
async fn gallery_retries_evidence_and_noops_preserve_independent_revisions() {
    let p = get_postgres_client().await;
    let (_, source) = source(&p).await;
    let m = manager(&p);
    let original = m.create.execute(&trusted(), command(source)).await.unwrap();
    assert_eq!(
        original,
        m.create.execute(&trusted(), command(source)).await.unwrap()
    );
    assert_eq!(1, signals(&p, source).await);
    let mut conflicting = command(source);
    conflicting.content = content(9500);
    assert!(matches!(
        m.create.execute(&trusted(), conflicting).await,
        Err(ShippingError::CreationKeyConflict)
    ));
    assert_eq!(
        original,
        m.replace
            .execute(&trusted(), replacement(&original, 8000))
            .await
            .unwrap()
    );
    let selected = m
        .assign_default
        .execute(
            &trusted(),
            AssignDefaultCommand {
                source_id: source,
                profile_id: original.id,
                expected_revision: ShippingDefaultRevision::INITIAL,
            },
        )
        .await
        .unwrap();
    assert_eq!(2, selected.revision.into_inner());
    let updated = m
        .replace
        .execute(&trusted(), replacement(&original, 9500))
        .await
        .unwrap();
    assert_eq!(2, updated.revision.into_inner());
    assert_eq!(2, updated.management_revision.into_inner());
    assert_eq!(
        updated,
        m.get
            .execute(&trusted(), source, original.id)
            .await
            .unwrap()
    );
    assert_eq!(
        selected,
        m.get_default.execute(&trusted(), source).await.unwrap()
    );
    assert_eq!(
        Some(shipping_core::time::CollectionArrangement::ByAppointment),
        updated.content.terms.collection().unwrap().arrangement()
    );
    // Creation retries compare original immutable semantics even after correction.
    assert_eq!(
        updated,
        m.create.execute(&trusted(), command(source)).await.unwrap()
    );
    let mut correction = replacement(&updated, 9500);
    correction.evidence.clear();
    let corrected = m.replace.execute(&trusted(), correction).await.unwrap();
    assert_eq!(2, corrected.revision.into_inner());
    assert_eq!(3, corrected.management_revision.into_inner());
    let kind: String = sqlx::query_scalar(
        "SELECT change_kind FROM shipping_changes WHERE subject_id=$1 AND revision=3",
    )
    .bind(original.id.into_uuid())
    .fetch_one(&p)
    .await
    .unwrap();
    assert_eq!("EVIDENCE_CORRECTED", kind);
    assert_eq!(4, signals(&p, source).await);
    let mut no_op = replacement(&corrected, 9500);
    no_op.evidence.clear();
    assert_eq!(
        corrected,
        m.replace.execute(&trusted(), no_op).await.unwrap()
    );
    assert!(matches!(
        m.replace
            .execute(&trusted(), replacement(&original, 8000))
            .await,
        Err(ShippingError::StaleRevision)
    ));
    let list = m.list.execute(&trusted(), source, None, 1).await.unwrap();
    assert_eq!(1, list.len());
    assert_eq!(original.id, list[0].id);
    assert!(
        m.list
            .execute(&trusted(), source, Some(original.id), 100)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        m.list.execute(&trusted(), source, None, 101).await,
        Err(ShippingError::InvalidInput)
    ));
    assert_eq!(4, signals(&p, source).await);
    let history: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM shipping_profile_history WHERE shipping_profile_id=$1",
    )
    .bind(original.id.into_uuid())
    .fetch_one(&p)
    .await
    .unwrap();
    assert_eq!(3, history);
}
#[aura_integration_test(services=[SCHEMA])]
async fn concurrent_creations_and_edits_cas_retirement_and_default_tombstones() {
    let p = get_postgres_client().await;
    let (_, source) = source(&p).await;
    let m = manager(&p);
    let ctx = trusted();
    let (a, b) = tokio::join!(
        m.create.execute(&ctx, command(source)),
        m.create.execute(&ctx, command(source))
    );
    let a = a.unwrap();
    assert_eq!(a, b.unwrap());
    assert_eq!(1, signals(&p, source).await);
    let mut other = command(source);
    other.creation_key = "other-profile".into();
    let b = m.create.execute(&ctx, other).await.unwrap();
    assert_ne!(a.id, b.id);
    let (one, two) = tokio::join!(
        m.replace.execute(&ctx, replacement(&a, 9500)),
        m.replace.execute(&ctx, replacement(&a, 9900))
    );
    assert!(matches!(
        (&one, &two),
        (Ok(_), Err(ShippingError::StaleRevision)) | (Err(ShippingError::StaleRevision), Ok(_))
    ));
    let current = m.get.execute(&ctx, source, a.id).await.unwrap();
    let assigned = m
        .assign_default
        .execute(
            &ctx,
            AssignDefaultCommand {
                source_id: source,
                profile_id: a.id,
                expected_revision: ShippingDefaultRevision::INITIAL,
            },
        )
        .await
        .unwrap();
    let retired = m.retire.execute(&ctx, lifecycle(&current)).await.unwrap();
    assert_eq!(ShippingProfileLifecycle::Retired, retired.lifecycle);
    let default = m.get_default.execute(&ctx, source).await.unwrap();
    assert_eq!(Some(a.id), default.profile_id);
    assert_eq!(Some(DefaultEligibility::Retired), default.eligibility);
    assert!(matches!(
        m.assign_default
            .execute(
                &ctx,
                AssignDefaultCommand {
                    source_id: source,
                    profile_id: a.id,
                    expected_revision: assigned.revision
                }
            )
            .await,
        Err(ShippingError::InvalidTerms(
            shipping_core::InvalidShippingValue::RetiredProfile
        ))
    ));
    assert_eq!(
        retired,
        m.retire.execute(&ctx, lifecycle(&retired)).await.unwrap()
    );
    assert!(matches!(
        m.retire.execute(&ctx, lifecycle(&current)).await,
        Err(ShippingError::StaleRevision)
    ));
    let cleared = m
        .clear_default
        .execute(
            &ctx,
            ClearDefaultCommand {
                source_id: source,
                expected_revision: assigned.revision,
            },
        )
        .await
        .unwrap();
    assert!(cleared.profile_id.is_none());
    assert_eq!(3, cleared.revision.into_inner());
    assert_eq!(
        cleared,
        m.clear_default
            .execute(
                &ctx,
                ClearDefaultCommand {
                    source_id: source,
                    expected_revision: cleared.revision
                }
            )
            .await
            .unwrap()
    );
    assert!(matches!(
        m.clear_default
            .execute(
                &ctx,
                ClearDefaultCommand {
                    source_id: source,
                    expected_revision: ShippingDefaultRevision::INITIAL
                }
            )
            .await,
        Err(ShippingError::StaleRevision)
    ));
    let restored = m.restore.execute(&ctx, lifecycle(&retired)).await.unwrap();
    assert_eq!(ShippingProfileLifecycle::Active, restored.lifecycle);
    assert_eq!(cleared, m.get_default.execute(&ctx, source).await.unwrap());
}
async fn member(p: &PgPool, owner: PartyId, source: ListingSourceId) -> Principal {
    let user = uuid::Uuid::now_v7();
    let partnership = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO users(user_id,email,tier,role) VALUES($1,$2,'FREE','USER')")
        .bind(user)
        .bind(format!("shipping-{}@example.test", user.simple()))
        .execute(p)
        .await
        .unwrap();
    sqlx::query("INSERT INTO partnerships(partnership_id,party_id) VALUES($1,$2)")
        .bind(partnership)
        .bind(owner.into_uuid())
        .execute(p)
        .await
        .unwrap();
    sqlx::query("INSERT INTO partnership_members(partnership_id,user_id) VALUES($1,$2)")
        .bind(partnership)
        .bind(user)
        .execute(p)
        .await
        .unwrap();
    sqlx::query("INSERT INTO partnership_listing_source_grants(partnership_id,listing_source_id) VALUES($1,$2)").bind(partnership).bind(source.into_uuid()).execute(p).await.unwrap();
    Principal::DelegatedUser {
        user_id: user.try_into().unwrap(),
        capabilities: [
            CredentialCapability::ListingSourcesRead,
            CredentialCapability::ListingSourcesWrite,
        ]
        .into(),
    }
}
#[aura_integration_test(services=[SCHEMA])]
async fn source_isolation_current_grants_suspension_and_caller_scoped_retry_keys() {
    let p = get_postgres_client().await;
    let (owner, source) = source(&p).await;
    let (_, foreign) = self::source(&p).await;
    let m = manager(&p);
    let principal = member(&p, owner, source).await;
    let ctx = context(principal.clone());
    assert!(matches!(
        m.create
            .execute(&context(Principal::Anonymous), command(source))
            .await,
        Err(ShippingError::AuthenticationRequired)
    ));
    assert!(matches!(
        m.create.execute(&ctx, command(foreign)).await,
        Err(ShippingError::SourceAccessDenied)
    ));
    let v = m.create.execute(&ctx, command(source)).await.unwrap();
    let alternate = m.create.execute(&trusted(), command(source)).await.unwrap();
    assert_ne!(v.id, alternate.id);
    assert!(matches!(
        m.get.execute(&trusted(), foreign, v.id).await,
        Err(ShippingError::ProfileNotFound)
    ));
    assert!(matches!(
        m.assign_default
            .execute(
                &trusted(),
                AssignDefaultCommand {
                    source_id: foreign,
                    profile_id: v.id,
                    expected_revision: ShippingDefaultRevision::INITIAL
                }
            )
            .await,
        Err(ShippingError::ProfileNotFound)
    ));
    let Principal::DelegatedUser { user_id, .. } = principal else {
        unreachable!()
    };
    // Cognito and delegated tokens identify the same caller.
    assert_eq!(
        v,
        m.create
            .execute(&context(Principal::User(user_id)), command(source))
            .await
            .unwrap()
    );
    let read = context(Principal::DelegatedUser {
        user_id,
        capabilities: [CredentialCapability::ListingSourcesRead].into(),
    });
    assert_eq!(v, m.get.execute(&read, source, v.id).await.unwrap());
    assert!(matches!(
        m.retire.execute(&read, lifecycle(&v)).await,
        Err(ShippingError::SourceAccessDenied)
    ));
    let write = context(Principal::DelegatedUser {
        user_id,
        capabilities: [CredentialCapability::ListingSourcesWrite].into(),
    });
    assert!(matches!(
        m.get.execute(&write, source, v.id).await,
        Err(ShippingError::SourceAccessDenied)
    ));
    sqlx::query("DELETE FROM partnership_listing_source_grants WHERE listing_source_id=$1")
        .bind(source.into_uuid())
        .execute(&p)
        .await
        .unwrap();
    assert!(matches!(
        m.replace.execute(&ctx, replacement(&v, 8000)).await,
        Err(ShippingError::SourceAccessDenied)
    ));
    sqlx::query("UPDATE users SET role='ADMIN' WHERE user_id=$1")
        .bind(user_id.into_uuid())
        .execute(&p)
        .await
        .unwrap();
    assert_eq!(v, m.get.execute(&ctx, source, v.id).await.unwrap());
    sqlx::query("UPDATE users SET suspended=true WHERE user_id=$1")
        .bind(user_id.into_uuid())
        .execute(&p)
        .await
        .unwrap();
    assert!(matches!(
        m.create.execute(&ctx, command(source)).await,
        Err(ShippingError::SourceAccessDenied)
    ));
    assert_eq!(2, signals(&p, source).await);
}
#[aura_integration_test(services=[SCHEMA])]
async fn source_deletion_retains_profiles_receipts_and_monotonic_default_fence() {
    let p = get_postgres_client().await;
    let (_, source) = source(&p).await;
    let m = manager(&p);
    let v = m.create.execute(&trusted(), command(source)).await.unwrap();
    let selected = m
        .assign_default
        .execute(
            &trusted(),
            AssignDefaultCommand {
                source_id: source,
                profile_id: v.id,
                expected_revision: ShippingDefaultRevision::INITIAL,
            },
        )
        .await
        .unwrap();
    sqlx::query("DELETE FROM listing_sources WHERE listing_source_id=$1")
        .bind(source.into_uuid())
        .execute(&p)
        .await
        .unwrap();
    assert!(matches!(
        m.create.execute(&trusted(), command(source)).await,
        Err(ShippingError::SourceNotFound)
    ));
    let fence:(i64,bool,Option<uuid::Uuid>)=sqlx::query_as("SELECT revision,source_deleted,shipping_profile_id FROM shipping_source_defaults WHERE listing_source_id=$1").bind(source.into_uuid()).fetch_one(&p).await.unwrap();
    assert_eq!(
        (selected.revision.into_inner() as i64 + 1, true, None),
        fence
    );
    let retained:(i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM shipping_profiles WHERE shipping_profile_id=$1),(SELECT count(*) FROM shipping_profile_creation_receipts WHERE shipping_profile_id=$1)").bind(v.id.into_uuid()).fetch_one(&p).await.unwrap();
    assert_eq!((1, 1), retained);
    assert_eq!(3, signals(&p, source).await);
    assert!(
        sqlx::query("DELETE FROM shipping_changes WHERE listing_source_id=$1")
            .bind(source.into_uuid())
            .execute(&p)
            .await
            .is_err()
    );
}
#[aura_integration_test(services=[SCHEMA])]
async fn corruption_and_exhaustion_fail_without_effects_including_noop_cas() {
    let p = get_postgres_client().await;
    let (_, source) = source(&p).await;
    let m = manager(&p);
    let v = m.create.execute(&trusted(), command(source)).await.unwrap();
    let original: serde_json::Value =
        sqlx::query_scalar("SELECT terms FROM shipping_profiles WHERE shipping_profile_id=$1")
            .bind(v.id.into_uuid())
            .fetch_one(&p)
            .await
            .unwrap();
    for corrupt in [
        serde_json::json!({"schema":"SHIP_TERMS_V1","shipping":null}),
        serde_json::json!({"schema":"SHIP_TERMS_V2","shipping":null,"collection":null}),
        serde_json::json!({"schema":"SHIP_TERMS_V1","shipping":{"kind":"PROHIBITED","extra":true},"collection":null}),
    ] {
        sqlx::query("UPDATE shipping_profiles SET terms=$2 WHERE shipping_profile_id=$1")
            .bind(v.id.into_uuid())
            .bind(corrupt)
            .execute(&p)
            .await
            .unwrap();
        assert!(matches!(
            m.get.execute(&trusted(), source, v.id).await,
            Err(ShippingError::CorruptShippingState { .. })
        ));
        assert!(matches!(
            m.retire.execute(&trusted(), lifecycle(&v)).await,
            Err(ShippingError::CorruptShippingState { .. })
        ));
    }
    sqlx::query(
        "UPDATE shipping_profiles SET terms=$2,management_revision=$3 WHERE shipping_profile_id=$1",
    )
    .bind(v.id.into_uuid())
    .bind(original)
    .bind(i64::MAX)
    .execute(&p)
    .await
    .unwrap();
    // Simulate a valid state at the signed storage limit with matching evidence.
    sqlx::query("INSERT INTO shipping_profile_evidence(shipping_profile_id,management_revision,evidence,actor) SELECT shipping_profile_id,$2,evidence,'test' FROM shipping_profile_evidence WHERE shipping_profile_id=$1 AND management_revision=1").bind(v.id.into_uuid()).bind(i64::MAX).execute(&p).await.unwrap();
    let max = m.get.execute(&trusted(), source, v.id).await.unwrap();
    assert_eq!(
        max,
        m.replace
            .execute(&trusted(), replacement(&max, 8000))
            .await
            .unwrap()
    );
    assert!(matches!(
        m.replace.execute(&trusted(), replacement(&max, 9500)).await,
        Err(ShippingError::RevisionExhausted)
    ));
    assert!(matches!(
        m.retire.execute(&trusted(), lifecycle(&max)).await,
        Err(ShippingError::RevisionExhausted)
    ));
    assert_eq!(1, signals(&p, source).await);
}
#[aura_integration_test(services=[SCHEMA])]
async fn explicit_location_applicability_checks_owner_lifecycle_and_noop_assignment() {
    let p = get_postgres_client().await;
    let (owner, source) = source(&p).await;
    let (other, _) = self::source(&p).await;
    let m = manager(&p);
    let location = PartyLocationId::new();
    sqlx::query("INSERT INTO party_locations(party_location_id,party_id,label,lifecycle,created_actor,updated_actor) VALUES($1,$2,'Warehouse','ACTIVE','test','test')").bind(location.into_uuid()).bind(other.into_uuid()).execute(&p).await.unwrap();
    let mut cmd = command(source);
    cmd.content.terms = DeliveryTerms::new(
        None,
        Some(CollectionPolicy::new(
            Applicability::default(),
            CollectionDependency::Independent,
            Some(ShippingEligibility::Offered),
            Some(DeliverySite::PartyLocation(location)),
            None,
        )),
    );
    assert!(matches!(
        m.create.execute(&trusted(), cmd.clone()).await,
        Err(ShippingError::LocationNotFound)
    ));
    sqlx::query("UPDATE party_locations SET party_id=$2 WHERE party_location_id=$1")
        .bind(location.into_uuid())
        .bind(owner.into_uuid())
        .execute(&p)
        .await
        .unwrap();
    let v = m.create.execute(&trusted(), cmd).await.unwrap();
    let assigned = m
        .assign_default
        .execute(
            &trusted(),
            AssignDefaultCommand {
                source_id: source,
                profile_id: v.id,
                expected_revision: ShippingDefaultRevision::INITIAL,
            },
        )
        .await
        .unwrap();
    let refs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM shipping_profile_location_references WHERE party_location_id=$1",
    )
    .bind(location.into_uuid())
    .fetch_one(&p)
    .await
    .unwrap();
    assert_eq!(1, refs);
    sqlx::query("UPDATE party_locations SET lifecycle='RETIRED' WHERE party_location_id=$1")
        .bind(location.into_uuid())
        .execute(&p)
        .await
        .unwrap();
    assert!(matches!(
        m.assign_default
            .execute(
                &trusted(),
                AssignDefaultCommand {
                    source_id: source,
                    profile_id: v.id,
                    expected_revision: assigned.revision
                }
            )
            .await,
        Err(ShippingError::LocationRetired)
    ));
    assert!(matches!(
        m.replace
            .execute(
                &trusted(),
                ReplaceProfileCommand {
                    source_id: source,
                    profile_id: v.id,
                    expected_revision: v.management_revision,
                    content: v.content.clone(),
                    evidence: evidence()
                }
            )
            .await,
        Err(ShippingError::LocationRetired)
    ));
    // Retirement/clear remain possible when a dependency has become ineligible.
    m.retire.execute(&trusted(), lifecycle(&v)).await.unwrap();
    m.clear_default
        .execute(
            &trusted(),
            ClearDefaultCommand {
                source_id: source,
                expected_revision: assigned.revision,
            },
        )
        .await
        .unwrap();
}

#[aura_integration_test(services=[SCHEMA])]
async fn signal_failure_rolls_back_state_history_evidence_and_creation_receipt() {
    let p = get_postgres_client().await;
    let (_, source) = source(&p).await;
    let m = manager(&p);
    let v = m.create.execute(&trusted(), command(source)).await.unwrap();
    sqlx::raw_sql("CREATE FUNCTION reject_test_shipping_signal() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'synthetic signal persistence failure'; END $$; CREATE TRIGGER reject_test_shipping_signal BEFORE INSERT ON shipping_changes FOR EACH ROW EXECUTE FUNCTION reject_test_shipping_signal();").execute(&p).await.unwrap();
    let error = m
        .replace
        .execute(&trusted(), replacement(&v, 9500))
        .await
        .unwrap_err();
    assert!(std::error::Error::source(&error).is_some());
    let mut second = command(source);
    second.creation_key = "failed-profile".into();
    assert!(m.create.execute(&trusted(), second.clone()).await.is_err());
    sqlx::raw_sql("DROP TRIGGER reject_test_shipping_signal ON shipping_changes; DROP FUNCTION reject_test_shipping_signal();").execute(&p).await.unwrap();
    assert_eq!(v, m.get.execute(&trusted(), source, v.id).await.unwrap());
    assert_eq!(1, signals(&p, source).await);
    let counts:(i64,i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM shipping_profiles),(SELECT count(*) FROM shipping_profile_history),(SELECT count(*) FROM shipping_profile_evidence),(SELECT count(*) FROM shipping_profile_creation_receipts)").fetch_one(&p).await.unwrap();
    assert_eq!((1, 1, 1, 1), counts);
    m.create.execute(&trusted(), second).await.unwrap();
    assert_eq!(2, signals(&p, source).await);
}

#[tokio::test]
async fn invalid_expected_revisions_fail_before_opening_transactions() {
    let p = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://postgres:postgres@127.0.0.1:1/postgres")
        .unwrap();
    p.close().await;
    let m = manager(&p);
    let source_id = ListingSourceId::new();
    let profile_id = ShippingProfileId::new();
    let expected_revision = ShippingManagementRevision::try_from(i64::MAX as u64 + 1).unwrap();
    let lifecycle = ProfileLifecycleCommand {
        source_id,
        profile_id,
        expected_revision,
    };
    assert!(matches!(
        m.retire.execute(&trusted(), lifecycle).await,
        Err(ShippingError::InvalidRevision)
    ));
    assert!(matches!(
        m.restore.execute(&trusted(), lifecycle).await,
        Err(ShippingError::InvalidRevision)
    ));
    assert!(matches!(
        m.replace
            .execute(
                &trusted(),
                ReplaceProfileCommand {
                    source_id,
                    profile_id,
                    expected_revision,
                    content: content(8000),
                    evidence: evidence()
                }
            )
            .await,
        Err(ShippingError::InvalidRevision)
    ));
    let expected_revision = ShippingDefaultRevision::try_from(i64::MAX as u64 + 1).unwrap();
    assert!(matches!(
        m.assign_default
            .execute(
                &trusted(),
                AssignDefaultCommand {
                    source_id,
                    profile_id,
                    expected_revision
                }
            )
            .await,
        Err(ShippingError::InvalidRevision)
    ));
    assert!(matches!(
        m.clear_default
            .execute(
                &trusted(),
                ClearDefaultCommand {
                    source_id,
                    expected_revision
                }
            )
            .await,
        Err(ShippingError::InvalidRevision)
    ));
}

#[aura_integration_test(services=[SCHEMA])]
async fn retired_profiles_require_restoration_before_evidence_correction() {
    let p = get_postgres_client().await;
    let (_, source) = source(&p).await;
    let m = manager(&p);
    let created = m.create.execute(&trusted(), command(source)).await.unwrap();
    let retired = m
        .retire
        .execute(&trusted(), lifecycle(&created))
        .await
        .unwrap();
    let mut correction = replacement(&retired, 8000);
    correction.evidence.clear();
    assert!(matches!(
        m.replace.execute(&trusted(), correction.clone()).await,
        Err(ShippingError::Profile(ShippingProfileError::Retired))
    ));
    assert_eq!(2, signals(&p, source).await);
    assert_eq!(
        retired,
        m.replace
            .execute(&trusted(), replacement(&retired, 8000))
            .await
            .unwrap()
    );
    let restored = m
        .restore
        .execute(&trusted(), lifecycle(&retired))
        .await
        .unwrap();
    correction.expected_revision = restored.management_revision;
    let corrected = m.replace.execute(&trusted(), correction).await.unwrap();
    assert_eq!(restored.revision, corrected.revision);
    assert_eq!(
        restored.management_revision.next(),
        corrected.management_revision
    );
    assert_eq!(4, signals(&p, source).await);
}

#[aura_integration_test(services=[SCHEMA])]
async fn default_exhaustion_preserves_noops_and_rolls_back_source_deletion() {
    let p = get_postgres_client().await;
    let (_, source) = source(&p).await;
    let m = manager(&p);
    let profile = m.create.execute(&trusted(), command(source)).await.unwrap();
    let never_assigned = m.get_default.execute(&trusted(), source).await.unwrap();
    assert_eq!(
        never_assigned,
        m.clear_default
            .execute(
                &trusted(),
                ClearDefaultCommand {
                    source_id: source,
                    expected_revision: never_assigned.revision,
                }
            )
            .await
            .unwrap()
    );
    assert_eq!(1, signals(&p, source).await);
    m.assign_default
        .execute(
            &trusted(),
            AssignDefaultCommand {
                source_id: source,
                profile_id: profile.id,
                expected_revision: never_assigned.revision,
            },
        )
        .await
        .unwrap();
    sqlx::query("UPDATE shipping_source_defaults SET revision=$2 WHERE listing_source_id=$1")
        .bind(source.into_uuid())
        .bind(i64::MAX)
        .execute(&p)
        .await
        .unwrap();
    let max = m.get_default.execute(&trusted(), source).await.unwrap();
    assert_eq!(
        max,
        m.assign_default
            .execute(
                &trusted(),
                AssignDefaultCommand {
                    source_id: source,
                    profile_id: profile.id,
                    expected_revision: max.revision,
                }
            )
            .await
            .unwrap()
    );
    assert!(matches!(
        m.clear_default
            .execute(
                &trusted(),
                ClearDefaultCommand {
                    source_id: source,
                    expected_revision: max.revision,
                }
            )
            .await,
        Err(ShippingError::RevisionExhausted)
    ));
    assert!(
        sqlx::query("DELETE FROM listing_sources WHERE listing_source_id=$1")
            .bind(source.into_uuid())
            .execute(&p)
            .await
            .is_err()
    );
    assert_eq!(
        max,
        m.get_default.execute(&trusted(), source).await.unwrap()
    );
    assert_eq!(2, signals(&p, source).await);
}

#[aura_integration_test(services=[SCHEMA])]
async fn source_deletion_without_selection_still_creates_a_retained_fence() {
    let p = get_postgres_client().await;
    let (_, source) = source(&p).await;
    sqlx::query("DELETE FROM listing_sources WHERE listing_source_id=$1")
        .bind(source.into_uuid())
        .execute(&p)
        .await
        .unwrap();
    let fence: (i64, bool, Option<uuid::Uuid>) = sqlx::query_as(
        "SELECT revision,source_deleted,current_source_id FROM shipping_source_defaults WHERE listing_source_id=$1"
    ).bind(source.into_uuid()).fetch_one(&p).await.unwrap();
    assert_eq!((2, true, None), fence);
    assert_eq!(1, signals(&p, source).await);
}
