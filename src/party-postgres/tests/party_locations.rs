use application::{
    operation_context::{CredentialCapability, OperationContext, Principal},
    patch_field::PatchField,
    transaction::{Transaction, UnitOfWork},
};
use geo::{
    AddressText, CountryCode, GeoPoint, GeographicDescription, SpatialPosition, SpatialPrecision,
};
use party_core::{
    party::{NewParty, Party, PartyContact},
    party_id::PartyId,
    party_location::{
        PartyLocationContent, PartyLocationDisclosure, PartyLocationLabel, PartyLocationLifecycle,
        PartyLocationRole,
    },
    party_location_id::PartyLocationId,
    party_name::PartyName,
};
use party_postgres::{
    SqlxPartyLocationAccessFactory, SqlxPartyLocationReaderFactory,
    SqlxPartyLocationRepositoryFactory, SqlxPartyRepositoryFactory,
};
use party_service::{
    ports::party_location::*,
    ports::{PartyRepository, PartyRepositoryFactory},
    use_cases::party_locations::*,
};
use platform_postgres::SqlxUnitOfWork;
use sqlx::PgPool;
use std::collections::BTreeSet;
use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};
use user_core::user_id::UserId;
use user_service::use_cases::queries::check_user_admin::{
    CheckUserAdminError, CheckUserAdminRequest, CheckUserAdminResult, CheckUserAdminUseCase,
};

const BUSINESS_SCHEMA: test_api::Postgres = test_api::Postgres::new("migrations");
struct NoAdmin;
#[async_trait::async_trait]
impl CheckUserAdminUseCase for NoAdmin {
    async fn execute(
        &self,
        _: &OperationContext,
        _: CheckUserAdminRequest,
    ) -> Result<CheckUserAdminResult, CheckUserAdminError> {
        Err(CheckUserAdminError::Forbidden)
    }
}
fn context(principal: Principal) -> OperationContext {
    OperationContext {
        principal,
        request_id: "test-request".into(),
        correlation_id: "test-correlation".into(),
    }
}
fn trusted() -> OperationContext {
    context(Principal::Service("location-test".to_owned()))
}
fn create(pool: &PgPool) -> impl CreatePartyLocationUseCase {
    CreatePartyLocationHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxPartyLocationRepositoryFactory,
        SqlxPartyLocationAccessFactory,
        NoAdmin,
    )
}
fn update(pool: &PgPool) -> impl UpdatePartyLocationUseCase {
    UpdatePartyLocationHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxPartyLocationRepositoryFactory,
        SqlxPartyLocationAccessFactory,
        NoAdmin,
    )
}
fn lifecycle(pool: &PgPool) -> impl SetPartyLocationLifecycleUseCase {
    SetPartyLocationLifecycleHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxPartyLocationRepositoryFactory,
        SqlxPartyLocationAccessFactory,
        NoAdmin,
    )
}
fn get(pool: &PgPool) -> impl GetPartyLocationUseCase {
    GetPartyLocationHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxPartyLocationReaderFactory,
        SqlxPartyLocationAccessFactory,
        NoAdmin,
    )
}
fn list(pool: &PgPool) -> impl ListPartyLocationsUseCase {
    ListPartyLocationsHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxPartyLocationReaderFactory,
        SqlxPartyLocationAccessFactory,
        NoAdmin,
    )
}
async fn party(pool: &PgPool) -> PartyId {
    let p = Party::create(NewParty {
        id: PartyId::new(),
        name: PartyName::try_from("Location test").unwrap(),
        contact: PartyContact::default(),
    });
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    SqlxPartyRepositoryFactory
        .in_transaction(&mut tx)
        .insert(&p)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    p.id()
}
fn command(party_id: PartyId, key: &str) -> CreatePartyLocationCommand {
    CreatePartyLocationCommand {
        party_id,
        idempotency_key: PartyLocationIdempotencyKey::new(key.to_owned()).unwrap(),
        content: PartyLocationContent {
            label: PartyLocationLabel::new("Test warehouse".to_owned()).unwrap(),
            roles: BTreeSet::from([PartyLocationRole::Warehouse]),
            geography: GeographicDescription::new(
                Some(AddressText::new("  123 Private Street\nBerlin  ".to_owned()).unwrap()),
                Some(CountryCode::DEU),
                None,
                None,
            )
            .unwrap(),
            position: Some(
                SpatialPosition::new(
                    GeoPoint::new(52.5, 13.4).unwrap(),
                    SpatialPrecision::Premises,
                    None,
                )
                .unwrap(),
            ),
            disclosure: PartyLocationDisclosure::Private,
        },
        evidence: Some(
            LocationEvidence::new(
                "https://private.example/evidence".to_owned(),
                None,
                LocationAssertionScope::SiteDescription,
            )
            .unwrap(),
        ),
        relocates: None,
    }
}
fn expected(v: &PartyLocationView) -> ExpectedPartyLocation {
    ExpectedPartyLocation {
        id: v.id,
        revision: v.revision.unwrap(),
    }
}
fn correction(party_id: PartyId, v: &PartyLocationView) -> UpdatePartyLocationCommand {
    UpdatePartyLocationCommand {
        party_id,
        expected: expected(v),
        label: PatchField::Unchanged,
        roles: PatchField::Unchanged,
        geography: PatchField::Unchanged,
        position: PatchField::Unchanged,
        disclosure: PatchField::Unchanged,
        evidence: PatchField::Unchanged,
    }
}
async fn signals(pool: &PgPool, id: PartyLocationId) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM party_location_changes WHERE party_location_id=$1")
        .bind(id.into_uuid())
        .fetch_one(pool)
        .await
        .unwrap()
}

#[aura_integration_test(services=[BUSINESS_SCHEMA])]
async fn concurrent_create_retries_converge_and_replay_original_result() {
    let pool = get_postgres_client().await;
    let party = party(&pool).await;
    let ctx = trusted();
    let c = command(party, "retry-key");
    let h1 = create(&pool);
    let h2 = create(&pool);
    let (a, b) = tokio::join!(h1.execute(&ctx, c.clone()), h2.execute(&ctx, c.clone()));
    let a = a.unwrap();
    assert_eq!(a, b.unwrap());
    assert_eq!(1, signals(&pool, a.id).await);
    let mut changed = correction(party, &a);
    changed.label = PatchField::Set(PartyLocationLabel::new("Corrected label".to_owned()).unwrap());
    let updated = update(&pool).execute(&ctx, changed).await.unwrap();
    assert_eq!(2, updated.revision.unwrap().into_inner());
    assert_eq!(a, create(&pool).execute(&ctx, c.clone()).await.unwrap());
    let mut conflict = c;
    conflict.content.disclosure = PartyLocationDisclosure::ExactPublic;
    assert!(matches!(
        create(&pool).execute(&ctx, conflict).await,
        Err(PartyLocationError::IdempotencyConflict)
    ));
    let other = create(&pool)
        .execute(&ctx, command(party, "independent-site"))
        .await
        .unwrap();
    assert_ne!(a.id, other.id);
    let page = list(&pool)
        .execute(
            &ctx,
            ListPartyLocationsRequest {
                party_id: party,
                after: None,
                limit: 1,
                private: true,
            },
        )
        .await
        .unwrap();
    assert_eq!(1, page.items.len());
    assert!(page.next.is_some());
    let second = list(&pool)
        .execute(
            &ctx,
            ListPartyLocationsRequest {
                party_id: party,
                after: page.next,
                limit: 1,
                private: true,
            },
        )
        .await
        .unwrap();
    assert_eq!(1, second.items.len());
    assert_ne!(page.items[0].id, second.items[0].id);
    assert!(second.next.is_none());
}

#[aura_integration_test(services=[BUSINESS_SCHEMA])]
async fn correction_noops_lifecycle_and_relocation_are_distinct() {
    let pool = get_postgres_client().await;
    let party = party(&pool).await;
    let ctx = trusted();
    let a = create(&pool)
        .execute(&ctx, command(party, "initial"))
        .await
        .unwrap();
    let noop = update(&pool)
        .execute(&ctx, correction(party, &a))
        .await
        .unwrap();
    assert_eq!(a, noop);
    assert_eq!(1, signals(&pool, a.id).await);
    let mut correct = correction(party, &a);
    correct.geography = PatchField::Set(
        GeographicDescription::new(
            Some(AddressText::new("123 Corrected Street".to_owned()).unwrap()),
            Some(CountryCode::DEU),
            None,
            None,
        )
        .unwrap()
        .unwrap(),
    );
    correct.position = PatchField::Clear;
    let corrected = update(&pool).execute(&ctx, correct).await.unwrap();
    assert_eq!(a.id, corrected.id);
    assert_eq!(Some(2), corrected.input_revision);
    assert!(corrected.position.is_none());
    assert!(matches!(
        update(&pool).execute(&ctx, correction(party, &a)).await,
        Err(PartyLocationError::ConcurrencyConflict)
    ));
    let mut relocation = command(party, "move");
    relocation.relocates = Some(expected(&corrected));
    let moved = create(&pool)
        .execute(&ctx, relocation.clone())
        .await
        .unwrap();
    assert_ne!(a.id, moved.id);
    assert_eq!(
        moved,
        create(&pool).execute(&ctx, relocation).await.unwrap()
    );
    let retired = get(&pool)
        .execute(
            &ctx,
            GetPartyLocationRequest {
                party_id: party,
                id: a.id,
                private: true,
            },
        )
        .await
        .unwrap();
    assert_eq!(PartyLocationLifecycle::Retired, retired.lifecycle);
    let noop_retire = lifecycle(&pool)
        .execute(
            &ctx,
            SetPartyLocationLifecycleCommand {
                party_id: party,
                expected: expected(&retired),
                lifecycle: PartyLocationLifecycle::Retired,
            },
        )
        .await
        .unwrap();
    assert_eq!(retired, noop_retire);
    assert_eq!(3, signals(&pool, a.id).await);
    let restored = lifecycle(&pool)
        .execute(
            &ctx,
            SetPartyLocationLifecycleCommand {
                party_id: party,
                expected: expected(&retired),
                lifecycle: PartyLocationLifecycle::Active,
            },
        )
        .await
        .unwrap();
    assert_eq!(a.id, restored.id);
    assert_eq!(Some(2), restored.input_revision);
    assert_eq!(4, signals(&pool, a.id).await);
    let deleted = sqlx::query("DELETE FROM parties WHERE party_id=$1")
        .bind(party.into_uuid())
        .execute(&pool)
        .await;
    assert!(deleted.is_err());
}

#[aura_integration_test(services=[BUSINESS_SCHEMA])]
async fn authorization_requires_scope_and_explicit_party_grant() {
    let pool = get_postgres_client().await;
    let party = party(&pool).await;
    let other_party = crate::party(&pool).await;
    let user = UserId::new();
    sqlx::query("INSERT INTO users(user_id,email,tier,role) VALUES ($1,$2,'FREE','USER')")
        .bind(user.into_uuid())
        .bind(format!("{user}@example.test"))
        .execute(&pool)
        .await
        .unwrap();
    let first_party = context(Principal::User(user));
    assert!(matches!(
        create(&pool)
            .execute(&first_party, command(party, "no-grant"))
            .await,
        Err(PartyLocationError::Forbidden)
    ));
    let grant = GrantPartyLocationManagementHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxPartyLocationAccessFactory,
        NoAdmin,
    );
    assert!(matches!(
        grant
            .execute(
                &first_party,
                GrantPartyLocationManagementCommand {
                    party_id: party,
                    user_id: user,
                    granted: true
                }
            )
            .await,
        Err(PartyLocationError::Forbidden)
    ));
    grant
        .execute(
            &trusted(),
            GrantPartyLocationManagementCommand {
                party_id: party,
                user_id: user,
                granted: true,
            },
        )
        .await
        .unwrap();
    let listing_only = context(Principal::DelegatedUser {
        user_id: user,
        capabilities: BTreeSet::from([CredentialCapability::ProductListingsWrite]),
    });
    assert!(matches!(
        create(&pool)
            .execute(&listing_only, command(party, "no-scope"))
            .await,
        Err(PartyLocationError::Forbidden)
    ));
    let scoped = context(Principal::DelegatedUser {
        user_id: user,
        capabilities: BTreeSet::from([
            CredentialCapability::PartiesWrite,
            CredentialCapability::PartiesRead,
        ]),
    });
    let a = create(&pool)
        .execute(&scoped, command(party, "granted"))
        .await
        .unwrap();
    assert!(matches!(
        create(&pool)
            .execute(&scoped, command(other_party, "wrong-party"))
            .await,
        Err(PartyLocationError::Forbidden)
    ));
    assert!(matches!(
        get(&pool)
            .execute(
                &listing_only,
                GetPartyLocationRequest {
                    party_id: party,
                    id: a.id,
                    private: true
                }
            )
            .await,
        Err(PartyLocationError::Forbidden)
    ));
    assert!(matches!(
        get(&pool)
            .execute(
                &scoped,
                GetPartyLocationRequest {
                    party_id: other_party,
                    id: a.id,
                    private: true
                }
            )
            .await,
        Err(PartyLocationError::Forbidden)
    ));
    assert!(matches!(
        get(&pool)
            .execute(
                &trusted(),
                GetPartyLocationRequest {
                    party_id: other_party,
                    id: a.id,
                    private: true
                }
            )
            .await,
        Err(PartyLocationError::NotFound)
    ));
    assert!(matches!(
        create(&pool)
            .execute(&context(Principal::Anonymous), command(party, "anon"))
            .await,
        Err(PartyLocationError::AuthenticationRequired)
    ));
    grant
        .execute(
            &trusted(),
            GrantPartyLocationManagementCommand {
                party_id: party,
                user_id: user,
                granted: false,
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        get(&pool)
            .execute(
                &scoped,
                GetPartyLocationRequest {
                    party_id: party,
                    id: a.id,
                    private: true
                }
            )
            .await,
        Err(PartyLocationError::Forbidden)
    ));
}

#[aura_integration_test(services=[BUSINESS_SCHEMA])]
async fn public_readers_redact_private_evidence_and_precise_geography() {
    let pool = get_postgres_client().await;
    let party = party(&pool).await;
    let ctx = trusted();
    let anonymous = context(Principal::Anonymous);
    let private = create(&pool)
        .execute(&ctx, command(party, "private"))
        .await
        .unwrap();
    assert!(matches!(
        get(&pool)
            .execute(
                &anonymous,
                GetPartyLocationRequest {
                    party_id: party,
                    id: private.id,
                    private: false
                }
            )
            .await,
        Err(PartyLocationError::NotFound)
    ));
    let mut c = command(party, "coarse");
    c.content.disclosure = PartyLocationDisclosure::CoarsePublic;
    let coarse = create(&pool).execute(&ctx, c).await.unwrap();
    let v = get(&pool)
        .execute(
            &anonymous,
            GetPartyLocationRequest {
                party_id: party,
                id: coarse.id,
                private: false,
            },
        )
        .await
        .unwrap();
    assert!(v.evidence.is_none());
    assert!(v.label.is_none());
    assert!(v.position.is_none());
    assert!(v.revision.is_none());
    assert!(v.input_revision.is_none());
    assert!(v.geography.as_ref().unwrap().address_text().is_none());
    assert_eq!(Some(CountryCode::DEU), v.geography.unwrap().country());
    let mut c = command(party, "exact");
    c.content.disclosure = PartyLocationDisclosure::ExactPublic;
    let exact = create(&pool).execute(&ctx, c).await.unwrap();
    let v = get(&pool)
        .execute(
            &anonymous,
            GetPartyLocationRequest {
                party_id: party,
                id: exact.id,
                private: false,
            },
        )
        .await
        .unwrap();
    assert!(v.evidence.is_none());
    assert!(v.position.is_some());
    assert!(v.geography.unwrap().address_text().is_some());
    let p = list(&pool)
        .execute(
            &anonymous,
            ListPartyLocationsRequest {
                party_id: party,
                after: None,
                limit: 100,
                private: false,
            },
        )
        .await
        .unwrap();
    assert_eq!(2, p.items.len());
    assert!(p.items.iter().all(|v| v.evidence.is_none()));
    lifecycle(&pool)
        .execute(
            &ctx,
            SetPartyLocationLifecycleCommand {
                party_id: party,
                expected: expected(&exact),
                lifecycle: PartyLocationLifecycle::Retired,
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        get(&pool)
            .execute(
                &anonymous,
                GetPartyLocationRequest {
                    party_id: party,
                    id: exact.id,
                    private: false
                }
            )
            .await,
        Err(PartyLocationError::NotFound)
    ));
}

#[aura_integration_test(services=[BUSINESS_SCHEMA])]
async fn independent_sessions_enforce_database_cas() {
    let pool = get_postgres_client().await;
    let party = party(&pool).await;
    let v = create(&pool)
        .execute(&trusted(), command(party, "cas"))
        .await
        .unwrap();
    let uow = SqlxUnitOfWork::new(pool.clone());
    let mut a = uow.begin().await.unwrap();
    let mut b = uow.begin().await.unwrap();
    let pid_a: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(a.connection())
        .await
        .unwrap();
    let pid_b: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(b.connection())
        .await
        .unwrap();
    assert_ne!(pid_a, pid_b);
    // Capture two independent stale snapshots without locking either owner or aggregate.
    let first = SqlxPartyLocationReaderFactory
        .in_transaction(&mut a)
        .get(party, v.id, true)
        .await
        .unwrap()
        .unwrap();
    let second = SqlxPartyLocationReaderFactory
        .in_transaction(&mut b)
        .get(party, v.id, true)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.revision, second.revision);
    let stored = SqlxPartyLocationRepositoryFactory
        .in_transaction(&mut a)
        .find_for_update(party, v.id)
        .await
        .unwrap()
        .unwrap();
    let mut change_a = stored.clone();
    let mut content = change_a.location.content().clone();
    content.label = PartyLocationLabel::new("Winner".to_owned()).unwrap();
    change_a.location.correct_same_site(content, false).unwrap();
    SqlxPartyLocationRepositoryFactory
        .in_transaction(&mut a)
        .update(&change_a, "test:a")
        .await
        .unwrap();
    let mut change_b = stored;
    let mut content = change_b.location.content().clone();
    content.label = PartyLocationLabel::new("Stale".to_owned()).unwrap();
    change_b.location.correct_same_site(content, false).unwrap();
    let (committed, result) = tokio::join!(a.commit(), async {
        SqlxPartyLocationRepositoryFactory
            .in_transaction(&mut b)
            .update(&change_b, "test:b")
            .await
    });
    committed.unwrap();
    assert!(matches!(
        result,
        Err(PartyLocationError::ConcurrencyConflict)
    ));
    drop(b);
    assert_eq!(2, signals(&pool, v.id).await);
}

#[aura_integration_test(services=[BUSINESS_SCHEMA])]
async fn service_updates_serialize_expected_revisions_and_rollback_failed_relocations() {
    let pool = get_postgres_client().await;
    let party = party(&pool).await;
    let ctx = trusted();
    let v = create(&pool)
        .execute(&ctx, command(party, "updates"))
        .await
        .unwrap();
    let mut c1 = correction(party, &v);
    c1.label = PatchField::Set(PartyLocationLabel::new("One".to_owned()).unwrap());
    let mut c2 = c1.clone();
    c2.label = PatchField::Set(PartyLocationLabel::new("Two".to_owned()).unwrap());
    let h1 = update(&pool);
    let h2 = update(&pool);
    let (a, b) = tokio::join!(h1.execute(&ctx, c1), h2.execute(&ctx, c2));
    assert_eq!(1, [a.is_ok(), b.is_ok()].into_iter().filter(|v| *v).count());
    assert!(
        matches!(a, Err(PartyLocationError::ConcurrencyConflict))
            || matches!(b, Err(PartyLocationError::ConcurrencyConflict))
    );
    let mut relocation = command(party, "failed-move");
    relocation.relocates = Some(expected(&v));
    assert!(matches!(
        create(&pool).execute(&ctx, relocation).await,
        Err(PartyLocationError::ConcurrencyConflict)
    ));
    let p = list(&pool)
        .execute(
            &ctx,
            ListPartyLocationsRequest {
                party_id: party,
                after: None,
                limit: 100,
                private: true,
            },
        )
        .await
        .unwrap();
    assert_eq!(1, p.items.len());
    assert_eq!(PartyLocationLifecycle::Active, p.items[0].lifecycle);
    assert_eq!(2, signals(&pool, v.id).await);
}

#[aura_integration_test(services=[BUSINESS_SCHEMA])]
async fn corrupt_persisted_geography_fails_closed() {
    let pool = get_postgres_client().await;
    let party = party(&pool).await;
    let v = create(&pool)
        .execute(&trusted(), command(party, "corrupt"))
        .await
        .unwrap();
    sqlx::query("UPDATE party_locations SET geography=jsonb_set(geography,'{country}','\"XX\"') WHERE party_location_id=$1").bind(v.id.into_uuid()).execute(&pool).await.unwrap();
    assert!(matches!(
        get(&pool)
            .execute(
                &trusted(),
                GetPartyLocationRequest {
                    party_id: party,
                    id: v.id,
                    private: true
                }
            )
            .await,
        Err(PartyLocationError::InvalidPersistedState { .. })
    ));
    assert!(matches!(
        update(&pool)
            .execute(&trusted(), correction(party, &v))
            .await,
        Err(PartyLocationError::InvalidPersistedState { .. })
    ));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn required_patch_fields_reject_clear_and_role_replacement_preserves_geographic_inputs() {
    let pool = get_postgres_client().await;
    let party = party(&pool).await;
    let ctx = trusted();
    let v = create(&pool)
        .execute(&ctx, command(party, "patch-contract"))
        .await
        .unwrap();
    for field in ["label", "roles", "disclosure"] {
        let mut c = correction(party, &v);
        match field {
            "label" => c.label = PatchField::Clear,
            "roles" => c.roles = PatchField::Clear,
            "disclosure" => c.disclosure = PatchField::Clear,
            _ => unreachable!(),
        }
        assert!(matches!(
            update(&pool).execute(&ctx, c).await,
            Err(PartyLocationError::InvalidInput)
        ));
    }
    assert_eq!(1, signals(&pool, v.id).await);
    let mut c = correction(party, &v);
    c.roles = PatchField::Set(BTreeSet::new());
    let updated = update(&pool).execute(&ctx, c).await.unwrap();
    assert!(updated.roles.is_empty());
    assert_eq!(v.input_revision, updated.input_revision);
    assert_eq!(2, updated.revision.unwrap().into_inner());
    let mut c = correction(party, &updated);
    c.roles = PatchField::Set(BTreeSet::new());
    assert_eq!(updated, update(&pool).execute(&ctx, c).await.unwrap());
    assert_eq!(2, signals(&pool, v.id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn invalid_stored_and_receipt_object_ids_fail_closed() {
    let pool = get_postgres_client().await;
    let party = party(&pool).await;
    let ctx = trusted();
    let c = command(party, "corrupt-identity");
    let v = create(&pool).execute(&ctx, c.clone()).await.unwrap();
    sqlx::query(
        "INSERT INTO party_locations(party_location_id,party_id,label,disclosure,lifecycle,created_actor,updated_actor) VALUES ($1,$2,'Invalid site','PRIVATE','ACTIVE','test','test')",
    )
    .bind(uuid::Uuid::nil())
    .bind(party.into_uuid())
    .execute(&pool)
    .await
    .unwrap();
    assert!(matches!(
        list(&pool)
            .execute(
                &ctx,
                ListPartyLocationsRequest {
                    party_id: party,
                    after: None,
                    limit: 100,
                    private: true,
                },
            )
            .await,
        Err(PartyLocationError::InvalidPersistedState { .. })
    ));
    sqlx::query("UPDATE party_location_create_receipts SET result=jsonb_set(result,'{id}',to_jsonb($1::text)) WHERE party_id=$2")
        .bind(uuid::Uuid::nil().to_string())
        .bind(party.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        create(&pool).execute(&ctx, c).await,
        Err(PartyLocationError::InvalidPersistedState { .. })
    ));
    assert_eq!(1, signals(&pool, v.id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn exhausted_revisions_allow_noops_and_reject_changes_without_signals() {
    let pool = get_postgres_client().await;
    let party = party(&pool).await;
    let ctx = trusted();
    let mut v = create(&pool)
        .execute(&ctx, command(party, "exhausted-revision"))
        .await
        .unwrap();
    sqlx::query("UPDATE party_locations SET revision=$1 WHERE party_location_id=$2")
        .bind(i64::MAX)
        .bind(v.id.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    v.revision = Some(PartyLocationRevision::try_from(i64::MAX).unwrap());
    assert_eq!(
        v,
        update(&pool)
            .execute(&ctx, correction(party, &v))
            .await
            .unwrap()
    );
    let mut c = correction(party, &v);
    c.label = PatchField::Set(PartyLocationLabel::new("Changed".to_owned()).unwrap());
    assert!(matches!(
        update(&pool).execute(&ctx, c).await,
        Err(PartyLocationError::RevisionExhausted)
    ));
    assert!(matches!(
        lifecycle(&pool)
            .execute(
                &ctx,
                SetPartyLocationLifecycleCommand {
                    party_id: party,
                    expected: expected(&v),
                    lifecycle: PartyLocationLifecycle::Retired,
                },
            )
            .await,
        Err(PartyLocationError::RevisionExhausted)
    ));
    assert_eq!(1, signals(&pool, v.id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn relocation_rolls_back_old_retirement_new_site_and_signals_on_receipt_failure() {
    let pool = get_postgres_client().await;
    let party = party(&pool).await;
    let ctx = trusted();
    let old = create(&pool)
        .execute(&ctx, command(party, "before-failure"))
        .await
        .unwrap();
    sqlx::query("ALTER TABLE party_location_create_receipts ADD CONSTRAINT test_receipt_failure CHECK (idempotency_key <> 'force-receipt-failure')").execute(&pool).await.unwrap();
    let mut relocation = command(party, "force-receipt-failure");
    relocation.relocates = Some(expected(&old));
    let result = create(&pool).execute(&ctx, relocation).await;
    sqlx::query("ALTER TABLE party_location_create_receipts DROP CONSTRAINT test_receipt_failure")
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(result, Err(PartyLocationError::Internal { .. })));
    assert_eq!(
        old,
        get(&pool)
            .execute(
                &ctx,
                GetPartyLocationRequest {
                    party_id: party,
                    id: old.id,
                    private: true
                }
            )
            .await
            .unwrap()
    );
    let page = list(&pool)
        .execute(
            &ctx,
            ListPartyLocationsRequest {
                party_id: party,
                after: None,
                limit: 100,
                private: true,
            },
        )
        .await
        .unwrap();
    assert_eq!(1, page.items.len());
    assert_eq!(1, signals(&pool, old.id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn country_only_free_text_and_unresolved_sites_persist_without_enrichment() {
    let pool = get_postgres_client().await;
    let party = party(&pool).await;
    let ctx = trusted();
    let descriptions = [
        None,
        GeographicDescription::new(None, Some(CountryCode::CAN), None, None).unwrap(),
        GeographicDescription::new(
            Some(AddressText::new("Unresolved location text".to_owned()).unwrap()),
            None,
            None,
            None,
        )
        .unwrap(),
    ];
    for (i, description) in descriptions.into_iter().enumerate() {
        let mut c = command(party, &format!("partial-{i}"));
        c.content.geography = description.clone();
        c.content.position = None;
        c.evidence = None;
        let v = create(&pool).execute(&ctx, c).await.unwrap();
        let read = get(&pool)
            .execute(
                &ctx,
                GetPartyLocationRequest {
                    party_id: party,
                    id: v.id,
                    private: true,
                },
            )
            .await
            .unwrap();
        assert_eq!(description, read.geography);
        assert!(read.position.is_none());
    }
}
