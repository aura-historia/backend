use application::{
    operation_context::{OperationContext, Principal},
    patch_field::PatchField,
    transaction::{Transaction, UnitOfWork},
};
use geo::{
    AddressText, CountryCode, DerivedGeography, GeoPoint, GeographicDescription, SpatialPosition,
    SpatialPrecision,
};
use geo_postgres::SqlxResolutionRepositoryFactory;
use geo_service::{geocoding::*, resolution::*};
use party_core::{
    party::{NewParty, Party, PartyContact},
    party_id::PartyId,
    party_location::{PartyLocationContent, PartyLocationLabel, PartyLocationLifecycle},
    party_name::PartyName,
};
use party_postgres::{
    SqlxPartyLocationAccessFactory, SqlxPartyLocationRepositoryFactory, SqlxPartyRepositoryFactory,
};
use party_service::{
    ports::{PartyRepository, PartyRepositoryFactory, party_location::*},
    use_cases::party_locations::*,
};
use platform_postgres::SqlxUnitOfWork;
use sqlx::PgPool;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};
use uuid::Uuid;
const SCHEMA: test_api::Postgres = test_api::Postgres::new("migrations");
const PROFILE: &str = "TEST_V1";
fn context() -> OperationContext {
    OperationContext {
        principal: Principal::Service("geo-test".into()),
        request_id: "test".into(),
        correlation_id: "test".into(),
    }
}
struct NoAdmin;
#[async_trait::async_trait]
impl user_service::use_cases::queries::check_user_admin::CheckUserAdminUseCase for NoAdmin {
    async fn execute(
        &self,
        _: &OperationContext,
        _: user_service::use_cases::queries::check_user_admin::CheckUserAdminRequest,
    ) -> Result<
        user_service::use_cases::queries::check_user_admin::CheckUserAdminResult,
        user_service::use_cases::queries::check_user_admin::CheckUserAdminError,
    > {
        Err(user_service::use_cases::queries::check_user_admin::CheckUserAdminError::Forbidden)
    }
}
async fn configure(pool: &PgPool) -> String {
    sqlx::query("UPDATE geo_resolution_provider_controls SET is_default=false")
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO geo_resolution_provider_controls(profile,is_default,enabled,storage_permitted,daily_limit,scope_daily_limit,concurrency_limit,scope_concurrency_limit) VALUES($1,true,true,true,100,20,2,1)").bind(PROFILE).execute(pool).await.unwrap();
    PROFILE.into()
}
async fn default_off(pool: &PgPool) {
    sqlx::query("UPDATE geo_resolution_provider_controls SET is_default=false")
        .execute(pool)
        .await
        .unwrap();
    // The fixture truncates controls between tests; seed no data beyond each test.
    sqlx::query("INSERT INTO geo_resolution_provider_controls(profile,is_default) VALUES($1,true)")
        .bind(PROFILE)
        .execute(pool)
        .await
        .unwrap();
}
fn input(owner: Uuid, query: &str) -> ResolutionInput {
    ResolutionInput::new(
        GeocodingRequest::new(AddressText::new(query).unwrap()),
        ResolutionScope::new(ScopeKind::Party, Some(owner)).unwrap(),
        RetentionPurpose::PartyLocation,
    )
    .unwrap()
}
fn binding() -> ResolutionBinding {
    ResolutionBinding {
        subject: ResolutionSubject::new(RetentionPurpose::PartyLocation, Uuid::now_v7()).unwrap(),
        input_revision: 1,
        guard: 1,
    }
}
async fn bind(pool: &PgPool, input: &ResolutionInput, b: ResolutionBinding) -> ResolutionWork {
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    let id = SqlxResolutionRepositoryFactory
        .resolutions(&mut tx)
        .bind(input, b)
        .await
        .unwrap();
    let view = SqlxResolutionRepositoryFactory
        .resolution_reader(&mut tx)
        .inspect(b.subject)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(id, view.work.id);
    tx.commit().await.unwrap();
    view.work
}
async fn claim(pool: &PgPool, w: ResolutionWork) -> ClaimOutcome {
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    let result = SqlxResolutionRepositoryFactory
        .resolutions(&mut tx)
        .claim(w, PROFILE)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    result
}
async fn inspect(pool: &PgPool, b: ResolutionBinding) -> Option<ResolutionView> {
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    let view = SqlxResolutionRepositoryFactory
        .resolution_reader(&mut tx)
        .inspect(b.subject)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    view
}
fn candidate(country: CountryCode) -> GeocodingCandidate {
    GeocodingCandidate::new(
        DerivedGeography::new(
            GeographicDescription::new(None, Some(country), None, None).unwrap(),
            Some(
                SpatialPosition::new(
                    GeoPoint::new(52.5, 13.4).unwrap(),
                    SpatialPrecision::Premises,
                    None,
                )
                .unwrap(),
            ),
        ),
        vec![],
        None,
        GeocodingProvenance {
            provider: "TEST",
            product: "TEST",
            data_source: "TEST",
            region: "TEST".into(),
            interpretation_profile: PROFILE,
            external_reference: "test-place".into(),
            attribution_url: "https://example.test/attribution",
        },
    )
}
fn result() -> GeocodingResult {
    GeocodingResult {
        outcome: GeocodingOutcome::Candidates(
            CandidateSet::new(vec![candidate(CountryCode::DEU)], 5).unwrap(),
        ),
        retention: ResultRetention::Storage,
    }
}
struct Provider {
    calls: AtomicUsize,
    pool: PgPool,
    result: GeocodingResult,
}
#[async_trait::async_trait]
impl GeocodingProvider for Provider {
    async fn geocode(&self, _: &GeocodingRequest) -> Result<GeocodingResult, GeocodingError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        // With a single-connection pool this would time out if the handler retained
        // an open transaction or connection over provider I/O.
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            sqlx::query("SELECT 1").execute(&self.pool),
        )
        .await
        .unwrap()
        .unwrap();
        Ok(self.result.clone())
    }
}
async fn provider(pool: &PgPool, result: GeocodingResult) -> Arc<Provider> {
    let isolated = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with(pool.connect_options().as_ref().clone())
        .await
        .unwrap();
    test_api::TestContext::current()
        .unwrap()
        .register_postgres_pool(isolated.clone())
        .unwrap();
    Arc::new(Provider {
        calls: AtomicUsize::new(0),
        pool: isolated,
        result,
    })
}
async fn complete(pool: &PgPool, c: &ResolutionClaim, e: ResolutionCompletion) -> bool {
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    let done = SqlxResolutionRepositoryFactory
        .resolutions(&mut tx)
        .complete(c, e)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    done
}

#[aura_integration_test(services=[SCHEMA], isolated = true)]
async fn input_failures_regression_do_not_open_the_shared_provider_circuit() {
    let pool = get_postgres_client().await;
    configure(&pool).await;
    for failure in [
        GeocodingFailure::UnsupportedInput,
        GeocodingFailure::StorageNotPermitted,
    ] {
        for index in 0..3 {
            let w = bind(
                &pool,
                &input(
                    Uuid::now_v7(),
                    &format!("Local rejection {failure:?} {index}"),
                ),
                binding(),
            )
            .await;
            let ClaimOutcome::Claimed(c) = claim(&pool, w).await else {
                panic!("claim required")
            };
            assert!(!complete(&pool, &c, ResolutionCompletion::Failure(failure)).await);
        }
        let state: (i32, Option<time::OffsetDateTime>) = sqlx::query_as(
            "SELECT consecutive_failures,open_until FROM geo_resolution_provider_controls WHERE profile=$1"
        ).bind(PROFILE).fetch_one(&pool).await.unwrap();
        assert_eq!((0, None), state);
    }
    let valid = bind(
        &pool,
        &input(Uuid::now_v7(), "Other owner's valid address"),
        binding(),
    )
    .await;
    assert!(matches!(
        claim(&pool, valid).await,
        ClaimOutcome::Claimed(_)
    ));
    let calls: i32 =
        sqlx::query_scalar("SELECT calls FROM geo_resolution_provider_controls WHERE profile=$1")
            .bind(PROFILE)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        7, calls,
        "input refusals still consume their conservative reservations"
    );
}

async fn mutation_before_inspection_lock_order(operation: &'static str) {
    let pool = get_postgres_client().await;
    configure(&pool).await;
    let owner = Uuid::now_v7();
    let mut source = binding();
    let input = if operation == "erasure" {
        source.subject =
            ResolutionSubject::new(RetentionPurpose::UserLocation, source.subject.id()).unwrap();
        sqlx::query("INSERT INTO users(user_id,email,tier,role) VALUES($1,$2,'FREE','USER')")
            .bind(owner)
            .bind(format!("{owner}@example.test"))
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO user_addresses(address_id,user_id,label,geography) VALUES($1,$2,'Home',$3)")
            .bind(source.subject.id()).bind(owner)
            .bind(serde_json::json!({"reference_release":"iso-codes-4.20.1","address_text":"Original private address"}))
            .execute(&pool).await.unwrap();
        ResolutionInput::new(
            GeocodingRequest::new(AddressText::new("Original private address").unwrap()),
            ResolutionScope::new(ScopeKind::User, Some(owner)).unwrap(),
            RetentionPurpose::UserLocation,
        )
        .unwrap()
    } else {
        input(owner, "Existing reusable address")
    };
    let work = bind(&pool, &input, source).await;
    let ClaimOutcome::Claimed(c) = claim(&pool, work).await else {
        panic!("claim required")
    };
    assert!(
        complete(
            &pool,
            &c,
            ResolutionCompletion::Evidence(evaluate(&input, result(), PROFILE).unwrap())
        )
        .await
    );

    // The worker takes its existing provider lock, then waits for this test to
    // start a mutation. The mutation must wait before taking the resolution lock.
    let mut worker = pool.begin().await.unwrap();
    sqlx::query(
        "SELECT profile FROM geo_resolution_provider_controls WHERE profile=$1 FOR NO KEY UPDATE",
    )
    .bind(PROFILE)
    .fetch_one(&mut *worker)
    .await
    .unwrap();
    let mut writer = pool.begin().await.unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *writer)
        .await
        .unwrap();
    let writing = test_api::spawn_owned(async move {
        match operation {
            "bind" => {
                SqlxResolutionRepositoryFactory::on_connection(&mut writer)
                    .bind(&input, ResolutionBinding { guard: 2, ..source })
                    .await
                    .unwrap();
            }
            "select" => {
                SqlxResolutionRepositoryFactory::on_connection(&mut writer)
                    .select(source, work, 0, "test-reviewer")
                    .await
                    .unwrap();
            }
            "erasure" => {
                sqlx::query("UPDATE user_addresses SET geography=$2,version=2,input_revision=2 WHERE address_id=$1")
                    .bind(source.subject.id())
                    .bind(serde_json::json!({"reference_release":"iso-codes-4.20.1","address_text":"Corrected private address"}))
                    .execute(&mut *writer).await.unwrap();
                let next = ResolutionInput::new(
                    GeocodingRequest::new(AddressText::new("Corrected private address").unwrap()),
                    input.scope(),
                    input.purpose(),
                )
                .unwrap();
                SqlxResolutionRepositoryFactory::on_connection(&mut writer)
                    .bind(
                        &next,
                        ResolutionBinding {
                            input_revision: 2,
                            guard: 2,
                            ..source
                        },
                    )
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let view = SqlxResolutionRepositoryFactory::on_connection(&mut writer)
            .inspect(source.subject)
            .await
            .unwrap()
            .unwrap();
        writer.commit().await.unwrap();
        view
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1 AND wait_event_type='Lock')")
                .bind(pid).fetch_one(&pool).await.unwrap();
            if waiting { break; }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }).await.expect("mutation must wait for the worker's provider lock");
    sqlx::query("SET LOCAL lock_timeout='200ms'")
        .execute(&mut *worker)
        .await
        .unwrap();
    let claim_result = SqlxResolutionRepositoryFactory::on_connection(&mut worker)
        .claim(work, PROFILE)
        .await;
    worker.rollback().await.unwrap();
    let view = tokio::time::timeout(std::time::Duration::from_secs(5), writing)
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(claim_result, Ok(ClaimOutcome::Complete)),
        "{operation} blocked the worker's resolution lock"
    );
    assert_eq!(
        if operation == "erasure" { 2 } else { 1 },
        view.input_revision
    );
}

#[aura_integration_test(services=[SCHEMA], isolated = true)]
async fn bind_then_inspect_lock_order_regression() {
    mutation_before_inspection_lock_order("bind").await;
}

#[aura_integration_test(services=[SCHEMA], isolated = true)]
async fn select_then_inspect_lock_order_regression() {
    mutation_before_inspection_lock_order("select").await;
}

#[aura_integration_test(services=[SCHEMA], isolated = true)]
async fn private_erasure_then_rebind_lock_order_regression() {
    mutation_before_inspection_lock_order("erasure").await;
}

#[aura_integration_test(services=[SCHEMA], isolated = true)]
async fn batch_read_waits_for_provider_control_before_locking_resolution() {
    let pool = get_postgres_client().await;
    configure(&pool).await;
    let source = binding();
    let input = input(Uuid::now_v7(), "Shared reader lock ordering");
    let work = bind(&pool, &input, source).await;
    let ClaimOutcome::Claimed(claim) = claim(&pool, work).await else {
        panic!("expected resolution claim");
    };
    let mut completion = pool.begin().await.unwrap();
    sqlx::query(
        "SELECT profile FROM geo_resolution_provider_controls WHERE profile=$1 FOR NO KEY UPDATE",
    )
    .bind(PROFILE)
    .fetch_one(&mut *completion)
    .await
    .unwrap();
    let mut reader = pool.begin().await.unwrap();
    let reader_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *reader)
        .await
        .unwrap();
    let reading = test_api::spawn_owned(async move {
        let result = SqlxResolutionRepositoryFactory::on_connection(&mut reader)
            .inspect_many(&[source.subject])
            .await;
        reader.commit().await.unwrap();
        result
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1 AND wait_event_type='Lock')")
                .bind(reader_pid).fetch_one(&pool).await.unwrap();
            if waiting { break; }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }).await.expect("reader must wait on provider controls");
    // A resolver already holding controls must still be able to lock/complete the
    // resolution. The previous inverse order timed out here (or deadlocked).
    sqlx::query("SET LOCAL lock_timeout='200ms'")
        .execute(&mut *completion)
        .await
        .unwrap();
    assert!(
        SqlxResolutionRepositoryFactory::on_connection(&mut completion)
            .complete(
                &claim,
                ResolutionCompletion::Evidence(evaluate(&input, result(), PROFILE).unwrap())
            )
            .await
            .unwrap()
    );
    completion.commit().await.unwrap();
    let views = tokio::time::timeout(std::time::Duration::from_secs(5), reading)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(views[0].as_ref().unwrap().accepted_geography().is_some());
}
async fn party(pool: &PgPool) -> PartyId {
    let p = Party::create(NewParty {
        id: PartyId::new(),
        name: PartyName::try_from("Geography test").unwrap(),
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
async fn create(pool: &PgPool, owner: PartyId, label: &str) -> PartyLocationView {
    CreatePartyLocationHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxPartyLocationRepositoryFactory,
        SqlxPartyLocationAccessFactory,
        NoAdmin,
    )
    .execute(
        &context(),
        CreatePartyLocationCommand {
            party_id: owner,
            content: PartyLocationContent {
                label: PartyLocationLabel::new(label.into()).unwrap(),
                roles: Default::default(),
                geography: GeographicDescription::new(
                    Some(AddressText::new("123 Private Street, unit 1").unwrap()),
                    Some(CountryCode::DEU),
                    None,
                    None,
                )
                .unwrap(),
                position: None,
            },
            evidence: None,
            relocates: None,
        },
    )
    .await
    .unwrap()
}
fn expected(v: &PartyLocationView) -> ExpectedPartyLocation {
    ExpectedPartyLocation {
        id: v.id,
        revision: v.revision,
    }
}
fn correction(v: &PartyLocationView) -> UpdatePartyLocationCommand {
    UpdatePartyLocationCommand {
        party_id: v.party_id,
        expected: expected(v),
        label: PatchField::Unchanged,
        roles: PatchField::Unchanged,
        geography: PatchField::Unchanged,
        position: PatchField::Unchanged,
        evidence: PatchField::Unchanged,
    }
}
async fn update(pool: &PgPool, c: UpdatePartyLocationCommand) -> PartyLocationView {
    UpdatePartyLocationHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxPartyLocationRepositoryFactory,
        SqlxPartyLocationAccessFactory,
        NoAdmin,
    )
    .execute(&context(), c)
    .await
    .unwrap()
}
fn location_binding(v: &PartyLocationView) -> ResolutionBinding {
    ResolutionBinding {
        subject: ResolutionSubject::new(RetentionPurpose::PartyLocation, v.id.into_uuid()).unwrap(),
        input_revision: v.input_revision,
        guard: v.revision.into_inner(),
    }
}

#[aura_integration_test(services=[SCHEMA], isolated = true)]
async fn creation_disabled_and_then_maintenance_recovery_reuses_unchanged_inputs_without_provider_transactions()
 {
    let pool = get_postgres_client().await;
    default_off(&pool).await;
    let owner = party(&pool).await;
    let location = create(&pool, owner, "initial").await;
    let b = location_binding(&location);
    let view = inspect(&pool, b).await.unwrap();
    assert_eq!(ResolutionStatus::Pending, view.status);
    let provider = provider(&pool, result()).await;
    let handler = ResolveHandler::new(
        SqlxUnitOfWork::new(provider.pool.clone()),
        SqlxResolutionRepositoryFactory,
        Arc::clone(&provider),
        PROFILE.into(),
    );
    assert_eq!(
        ResolveOutcome::Deferred,
        handler.execute(&context(), view.work).await.unwrap()
    );
    assert_eq!(0, provider.calls.load(Ordering::SeqCst));
    sqlx::query("UPDATE geo_resolution_provider_controls SET enabled=true,storage_permitted=true")
        .execute(&pool)
        .await
        .unwrap();
    // The original journal entry still exists, but its CDC delivery was lost.
    let maintenance = MaintainResolutionsHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxResolutionRepositoryFactory,
    );
    assert_eq!(
        ResolutionMaintenanceCounts {
            expired: 0,
            requeued: 1
        },
        maintenance.execute(&context(), 10).await.unwrap()
    );
    assert_eq!(0, provider.calls.load(Ordering::SeqCst));
    let original = view.work;
    let view = inspect(&pool, b).await.unwrap();
    assert_eq!(original.id, view.work.id);
    assert_eq!(original.generation + 1, view.work.generation);
    assert_eq!(
        ResolveOutcome::Complete,
        handler.execute(&context(), original).await.unwrap()
    );
    handler.execute(&context(), view.work).await.unwrap();
    assert_eq!(1, provider.calls.load(Ordering::SeqCst));
    assert!(
        inspect(&pool, b)
            .await
            .unwrap()
            .accepted_geography()
            .unwrap()
            .position()
            .is_none()
    );
    assert_eq!(
        ResolveOutcome::Complete,
        handler.execute(&context(), view.work).await.unwrap()
    );
    let mut c = correction(&location);
    c.label = PatchField::Set(PartyLocationLabel::new("New label".into()).unwrap());
    let renamed = update(&pool, c).await;
    let second = create(&pool, owner, "second-site").await;
    let second_view = inspect(&pool, location_binding(&second)).await.unwrap();
    assert_eq!(view.work, second_view.work);
    assert_eq!(location.input_revision, renamed.input_revision);
    handler.execute(&context(), second_view.work).await.unwrap();
    assert_eq!(1, provider.calls.load(Ordering::SeqCst));
    assert_ne!(location.id, second.id);
    // Establish a binding for a preexisting site through the protected application
    // contract; requesting it repeatedly must reuse the same permitted result.
    sqlx::query("DELETE FROM geo_resolution_bindings WHERE subject_id=$1")
        .bind(second.id.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    let manage = ManagePartyLocationResolutionHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxPartyLocationRepositoryFactory,
        SqlxPartyLocationAccessFactory,
        NoAdmin,
    );
    for _ in 0..2 {
        let requested = manage
            .execute(
                &context(),
                PartyLocationResolutionCommand::Request {
                    party_id: owner,
                    expected: expected(&second),
                },
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(view.work, requested.work);
        assert!(requested.accepted_geography().is_some());
    }
    assert_eq!(1, provider.calls.load(Ordering::SeqCst));
}

#[aura_integration_test(services=[SCHEMA], isolated = true)]
async fn resolution_claims_and_reuse_allow_independent_binding_invalidation() {
    let pool = get_postgres_client().await;
    configure(&pool).await;
    let a = input(Uuid::now_v7(), "shared scoped input");
    let first = binding();
    let second = binding();
    let w = bind(&pool, &a, first).await;
    bind(&pool, &a, second).await;

    for reserve in [true, false] {
        let mut holder = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
        if reserve {
            assert!(matches!(
                SqlxResolutionRepositoryFactory
                    .resolutions(&mut holder)
                    .claim(w, PROFILE)
                    .await
                    .unwrap(),
                ClaimOutcome::Claimed(_)
            ));
        } else {
            SqlxResolutionRepositoryFactory
                .resolutions(&mut holder)
                .bind(&a, first)
                .await
                .unwrap();
        }

        let mut detach = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
        sqlx::query("SET LOCAL lock_timeout = '300ms'")
            .execute(detach.connection())
            .await
            .unwrap();
        // The claim/reuse transaction coordinates the resolution; detaching a
        // different subject must still journal its invalidation without waiting.
        SqlxResolutionRepositoryFactory
            .resolutions(&mut detach)
            .detach(second.subject)
            .await
            .unwrap();
        detach.commit().await.unwrap();
        drop(holder);
        assert!(inspect(&pool, second).await.is_none());
        bind(&pool, &a, second).await;
    }
}

#[aura_integration_test(services=[SCHEMA], isolated = true)]
async fn binding_changes_journal_cached_reuse_and_invalidation_of_reviewed_geography() {
    let pool = get_postgres_client().await;
    configure(&pool).await;
    let a = input(Uuid::now_v7(), "reviewed scoped input");
    let first = binding();
    let w = bind(&pool, &a, first).await;
    let ClaimOutcome::Claimed(c) = claim(&pool, w).await else {
        panic!("claim required");
    };
    assert!(
        complete(
            &pool,
            &c,
            ResolutionCompletion::Evidence(evaluate(&a, result(), PROFILE).unwrap())
        )
        .await
    );
    let second = binding();
    assert_eq!(w, bind(&pool, &a, second).await);
    bind(&pool, &a, second).await;
    let signals: i64 = sqlx::query_scalar("SELECT count(*) FROM geo_resolution_changes WHERE change_kind='RESOLVED' AND subject_id=$1")
        .bind(second.subject.id()).fetch_one(&pool).await.unwrap();
    assert_eq!(1, signals, "cached reuse must notify the new subject once");

    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    SqlxResolutionRepositoryFactory
        .resolutions(&mut tx)
        .select(first, w, 0, "test-reviewer")
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let presentation = ResolutionBinding { guard: 2, ..first };
    bind(&pool, &a, presentation).await;
    assert!(
        inspect(&pool, first)
            .await
            .unwrap()
            .accepted_geography()
            .unwrap()
            .position()
            .is_some()
    );
    let corrected = ResolutionBinding {
        input_revision: 2,
        guard: 3,
        ..first
    };
    assert_eq!(w, bind(&pool, &a, corrected).await);
    bind(&pool, &a, corrected).await;
    let view = inspect(&pool, first).await.unwrap();
    assert_eq!(2, view.input_revision);
    assert!(view.selected_candidate.is_none());
    assert!(view.accepted_geography().unwrap().position().is_none());
    let signals: i64 = sqlx::query_scalar("SELECT count(*) FROM geo_resolution_changes WHERE change_kind='INVALIDATED' AND subject_id=$1")
        .bind(first.subject.id()).fetch_one(&pool).await.unwrap();
    assert_eq!(
        1, signals,
        "source correction must invalidate the reviewed point once"
    );
}

#[aura_integration_test(services=[SCHEMA], isolated = true)]
async fn inspection_waiting_for_a_resolution_cannot_return_a_replaced_binding() {
    let pool = get_postgres_client().await;
    configure(&pool).await;
    let owner = Uuid::now_v7();
    let b = binding();
    let w = bind(&pool, &input(owner, "original address"), b).await;
    let mut holder = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    sqlx::query(
        "SELECT resolution_id FROM geo_resolutions WHERE resolution_id=$1 FOR NO KEY UPDATE",
    )
    .bind(w.id.into_uuid())
    .execute(holder.connection())
    .await
    .unwrap();
    let reader_pool = pool.clone();
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let reader = test_api::spawn_owned(async move {
        let mut tx = SqlxUnitOfWork::new(reader_pool).begin().await.unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(tx.connection())
            .await
            .unwrap();
        sender.send(pid).unwrap();
        let view = SqlxResolutionRepositoryFactory
            .resolution_reader(&mut tx)
            .inspect(b.subject)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        view
    });
    let pid = receiver.await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1 AND wait_event_type='Lock')")
                .bind(pid).fetch_one(&pool).await.unwrap();
            if waiting { break; }
            tokio::task::yield_now().await;
        }
    }).await.expect("inspection must have read the old binding and be waiting on its resolution");
    let changed = ResolutionBinding {
        input_revision: 2,
        guard: 2,
        ..b
    };
    let current = bind(&pool, &input(owner, "corrected address"), changed).await;
    assert_ne!(w.id, current.id);
    holder.commit().await.unwrap();
    assert!(
        reader.await.unwrap().is_none(),
        "the old binding became ineligible while the reader waited"
    );
    assert_eq!(current, inspect(&pool, changed).await.unwrap().work);
}

#[aura_integration_test(services=[SCHEMA], isolated = true)]
async fn independent_sessions_coalesce_identical_inputs_and_expired_leases_are_fenced() {
    let pool = get_postgres_client().await;
    configure(&pool).await;
    let a = input(Uuid::now_v7(), "same input");
    let b1 = binding();
    let b2 = binding();
    let (w1, w2) = tokio::join!(bind(&pool, &a, b1), bind(&pool, &a, b2));
    assert_eq!(w1, w2);
    let (c1, c2) = tokio::join!(claim(&pool, w1), claim(&pool, w2));
    let first = match (c1, c2) {
        (ClaimOutcome::Claimed(c), ClaimOutcome::Deferred)
        | (ClaimOutcome::Deferred, ClaimOutcome::Claimed(c)) => c,
        _ => panic!("exactly one session may claim"),
    };
    sqlx::query("UPDATE geo_resolutions SET lease_until=clock_timestamp()-interval '1 second' WHERE resolution_id=$1").bind(w1.id.into_uuid()).execute(&pool).await.unwrap();
    let ClaimOutcome::Claimed(second) = claim(&pool, w1).await else {
        panic!("expired claim must recover")
    };
    assert_ne!(first.token, second.token);
    let e = evaluate(&a, result(), PROFILE).unwrap();
    assert!(!complete(&pool, &first, ResolutionCompletion::Evidence(e.clone())).await);
    assert!(complete(&pool, &second, ResolutionCompletion::Evidence(e.clone())).await);
    assert!(!complete(&pool, &second, ResolutionCompletion::Evidence(e)).await);
    let changes: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM geo_resolution_changes WHERE change_kind='RESOLVED'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(1, changes);
    assert!(matches!(claim(&pool, w1).await, ClaimOutcome::Complete));
}

#[aura_integration_test(services=[SCHEMA], isolated = true)]
async fn address_retirement_and_manual_corrections_cancel_stale_application() {
    let pool = get_postgres_client().await;
    configure(&pool).await;
    let owner = party(&pool).await;
    for case in ["address", "retirement", "manual"] {
        let location = create(&pool, owner, case).await;
        let b = location_binding(&location);
        let original = inspect(&pool, b).await.unwrap();
        let ClaimOutcome::Claimed(c) = claim(&pool, original.work).await else {
            panic!("claim required")
        };
        match case {
            "retirement" => {
                SetPartyLocationLifecycleHandler::new(
                    SqlxUnitOfWork::new(pool.clone()),
                    SqlxPartyLocationRepositoryFactory,
                    SqlxPartyLocationAccessFactory,
                    NoAdmin,
                )
                .execute(
                    &context(),
                    SetPartyLocationLifecycleCommand {
                        party_id: owner,
                        expected: expected(&location),
                        lifecycle: PartyLocationLifecycle::Retired,
                    },
                )
                .await
                .unwrap();
            }
            _ => {
                let mut change = correction(&location);
                match case {
                    "address" => {
                        change.geography = PatchField::Set(
                            GeographicDescription::new(
                                Some(AddressText::new("changed address, unit 2").unwrap()),
                                Some(CountryCode::DEU),
                                None,
                                None,
                            )
                            .unwrap()
                            .unwrap(),
                        )
                    }
                    "manual" => {
                        change.position = PatchField::Set(
                            SpatialPosition::new(
                                GeoPoint::new(1., 2.).unwrap(),
                                SpatialPrecision::Premises,
                                None,
                            )
                            .unwrap(),
                        )
                    }
                    _ => unreachable!(),
                };
                update(&pool, change).await;
            }
        }
        // Other locations with the old input may legitimately share a resolution;
        // this subject must never see it as current after its binding changes.
        complete(
            &pool,
            &c,
            ResolutionCompletion::Evidence(evaluate(&c.input, result(), PROFILE).unwrap()),
        )
        .await;
        let current = inspect(&pool, b).await;
        if matches!(case, "manual" | "retirement") {
            assert!(current.is_none());
        } else {
            assert_ne!(original.work.id, current.unwrap().work.id);
        }
    }
}

#[aura_integration_test(services=[SCHEMA], isolated = true)]
async fn review_verifies_current_candidate_and_binding_and_expiry_purges_all_derivatives() {
    let pool = get_postgres_client().await;
    configure(&pool).await;
    let location = create(&pool, party(&pool).await, "review").await;
    let b = location_binding(&location);
    let w = inspect(&pool, b).await.unwrap().work;
    let provider = provider(&pool, result()).await;
    let handler = ResolveHandler::new(
        SqlxUnitOfWork::new(provider.pool.clone()),
        SqlxResolutionRepositoryFactory,
        provider,
        PROFILE.into(),
    );
    handler.execute(&context(), w).await.unwrap();
    let manage = ManagePartyLocationResolutionHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxPartyLocationRepositoryFactory,
        SqlxPartyLocationAccessFactory,
        NoAdmin,
    );
    let accepted = manage
        .execute(
            &context(),
            PartyLocationResolutionCommand::AcceptCandidate {
                party_id: location.party_id,
                expected: expected(&location),
                work: w,
                candidate_index: 0,
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        Some(GeoPoint::new(52.5, 13.4).unwrap()),
        accepted
            .accepted_geography()
            .unwrap()
            .position()
            .map(|p| p.point())
    );
    let mut anonymous = context();
    anonymous.principal = Principal::Anonymous;
    assert!(
        manage
            .execute(
                &anonymous,
                PartyLocationResolutionCommand::Inspect {
                    party_id: location.party_id,
                    id: location.id
                }
            )
            .await
            .is_err()
    );
    let mut c = correction(&location);
    c.label = PatchField::Set(PartyLocationLabel::new("renamed".into()).unwrap());
    let changed = update(&pool, c).await;
    assert!(matches!(
        manage
            .execute(
                &context(),
                PartyLocationResolutionCommand::AcceptCandidate {
                    party_id: location.party_id,
                    expected: expected(&location),
                    work: w,
                    candidate_index: 0
                }
            )
            .await,
        Err(PartyLocationError::ConcurrencyConflict)
    ));
    assert!(matches!(
        manage
            .execute(
                &context(),
                PartyLocationResolutionCommand::AcceptCandidate {
                    party_id: location.party_id,
                    expected: expected(&changed),
                    work: ResolutionWork {
                        generation: w.generation + 1,
                        ..w
                    },
                    candidate_index: 0
                }
            )
            .await,
        Err(PartyLocationError::ConcurrencyConflict)
    ));
    sqlx::query("UPDATE geo_resolutions SET valid_until=clock_timestamp()-interval '1 second'")
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        inspect(&pool, location_binding(&changed))
            .await
            .unwrap()
            .accepted_geography()
            .is_none()
    );
    assert!(
        manage
            .execute(
                &context(),
                PartyLocationResolutionCommand::AcceptCandidate {
                    party_id: location.party_id,
                    expected: expected(&changed),
                    work: w,
                    candidate_index: 0
                }
            )
            .await
            .is_err()
    );
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    // Rejected selection commands roll back asynchronously when SQLx drops their
    // transactions. Wait for those locks before asserting a SKIP LOCKED drain count.
    sqlx::query(
        "SELECT resolution_id FROM geo_resolutions WHERE resolution_id=$1 FOR NO KEY UPDATE",
    )
    .bind(w.id.into_uuid())
    .execute(tx.connection())
    .await
    .unwrap();
    assert_eq!(
        1,
        SqlxResolutionRepositoryFactory
            .resolutions(&mut tx)
            .expire(10)
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    let current = inspect(&pool, location_binding(&changed)).await.unwrap();
    assert_eq!(w.generation + 1, current.work.generation);
    assert!(current.evidence.is_none());
    assert!(current.selected_candidate.is_none());
    let retained: i64 =
        sqlx::query_scalar("SELECT count(*) FROM geo_resolutions WHERE evidence IS NOT NULL")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(0, retained);
    assert!(matches!(claim(&pool, w).await, ClaimOutcome::Complete));
}

#[aura_integration_test(services=[SCHEMA], isolated = true)]
async fn budgets_concurrency_backoff_breaker_and_exhaustion_bound_paid_work() {
    let pool = get_postgres_client().await;
    configure(&pool).await;
    let owner = Uuid::now_v7();
    let a = bind(&pool, &input(owner, "first"), binding()).await;
    let b = bind(&pool, &input(owner, "second"), binding()).await;
    let ClaimOutcome::Claimed(c) = claim(&pool, a).await else {
        panic!("claim required")
    };
    assert!(matches!(claim(&pool, b).await, ClaimOutcome::Deferred));
    assert!(
        !complete(
            &pool,
            &c,
            ResolutionCompletion::Failure(GeocodingFailure::Unavailable)
        )
        .await
    );
    assert!(matches!(claim(&pool, a).await, ClaimOutcome::Deferred));
    for query in ["third", "fourth"] {
        let w = bind(&pool, &input(Uuid::now_v7(), query), binding()).await;
        let ClaimOutcome::Claimed(c) = claim(&pool, w).await else {
            panic!("claim required")
        };
        complete(
            &pool,
            &c,
            ResolutionCompletion::Failure(GeocodingFailure::Unavailable),
        )
        .await;
    }
    assert!(matches!(claim(&pool, b).await, ClaimOutcome::Deferred));
    sqlx::query("UPDATE geo_resolution_provider_controls SET open_until=NULL,scope_daily_limit=1,daily_limit=3").execute(&pool).await.unwrap();
    assert!(matches!(claim(&pool, b).await, ClaimOutcome::Deferred));
    let state: (String, i32) =
        sqlx::query_as("SELECT last_failure,attempts FROM geo_resolutions WHERE resolution_id=$1")
            .bind(a.id.into_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(("UNAVAILABLE".into(), 1), state);
    // Exhausted crash leases are terminal operational failures, never NO_MATCH.
    sqlx::query("UPDATE geo_resolutions SET attempts=5,lease_token=$2,lease_until=clock_timestamp()-interval '1 second' WHERE resolution_id=$1").bind(a.id.into_uuid()).bind(Uuid::now_v7()).execute(&pool).await.unwrap();
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    SqlxResolutionRepositoryFactory
        .resolutions(&mut tx)
        .recover_pending(10)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let status: String =
        sqlx::query_scalar("SELECT status FROM geo_resolutions WHERE resolution_id=$1")
            .bind(a.id.into_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!("FAILED", status);
}

#[aura_integration_test(services=[SCHEMA], isolated = true)]
async fn pending_recovery_skips_exhausted_scopes_and_inactive_provider_profiles() {
    let pool = get_postgres_client().await;
    configure(&pool).await;
    let owner = Uuid::now_v7();
    let exhausted = bind(&pool, &input(owner, "older exhausted work"), binding()).await;
    let eligible = bind(
        &pool,
        &input(Uuid::now_v7(), "later eligible work"),
        binding(),
    )
    .await;
    sqlx::query("INSERT INTO geo_resolution_scope_budgets(profile,scope_key,budget_day,calls) VALUES($1,$2,(clock_timestamp() AT TIME ZONE 'UTC')::date,20)")
        .bind(PROFILE).bind(input(owner, "irrelevant").budget_key()).execute(&pool).await.unwrap();
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    let pending = SqlxResolutionRepositoryFactory
        .resolutions(&mut tx)
        .recover_pending(1)
        .await
        .unwrap();
    assert_eq!(1, pending);
    tx.commit().await.unwrap();
    let current: i64 =
        sqlx::query_scalar("SELECT generation FROM geo_resolutions WHERE resolution_id=$1")
            .bind(eligible.id.into_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(eligible.generation + 1, current as u64);
    let current: i64 =
        sqlx::query_scalar("SELECT generation FROM geo_resolutions WHERE resolution_id=$1")
            .bind(exhausted.id.into_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(exhausted.generation, current as u64);
    sqlx::query("UPDATE geo_resolution_provider_controls SET is_default=false")
        .execute(&pool)
        .await
        .unwrap();
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    assert_eq!(
        0,
        SqlxResolutionRepositoryFactory
            .resolutions(&mut tx)
            .recover_pending(1)
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    sqlx::query("UPDATE geo_resolution_provider_controls SET is_default=true WHERE profile=$1")
        .bind(PROFILE)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE geo_resolution_scope_budgets SET budget_day=budget_day-1")
        .execute(&pool)
        .await
        .unwrap();
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    assert_eq!(
        1,
        SqlxResolutionRepositoryFactory
            .resolutions(&mut tx)
            .recover_pending(1)
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    let current: i64 =
        sqlx::query_scalar("SELECT generation FROM geo_resolutions WHERE resolution_id=$1")
            .bind(exhausted.id.into_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(exhausted.generation + 1, current as u64);
}

#[aura_integration_test(services=[SCHEMA], isolated = true)]
async fn maintenance_recovery_preserves_paid_attempts_and_skips_live_or_locked_work() {
    let pool = get_postgres_client().await;
    configure(&pool).await;
    let subject = binding();
    let retry = bind(&pool, &input(Uuid::now_v7(), "retry"), subject).await;
    let active = bind(&pool, &input(Uuid::now_v7(), "active"), binding()).await;
    let locked = bind(&pool, &input(Uuid::now_v7(), "locked"), binding()).await;
    let ClaimOutcome::Claimed(failed_claim) = claim(&pool, retry).await else {
        panic!("claim required")
    };
    complete(
        &pool,
        &failed_claim,
        ResolutionCompletion::Failure(GeocodingFailure::Unavailable),
    )
    .await;
    assert!(matches!(
        claim(&pool, active).await,
        ClaimOutcome::Claimed(_)
    ));
    let maintenance = MaintainResolutionsHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxResolutionRepositoryFactory,
    );
    // A future retry deadline remains authoritative; the other runnable row is locked.
    let mut locked_tx = pool.begin().await.unwrap();
    sqlx::query("SELECT 1 FROM geo_resolutions WHERE resolution_id=$1 FOR NO KEY UPDATE")
        .bind(locked.id.into_uuid())
        .execute(&mut *locked_tx)
        .await
        .unwrap();
    assert_eq!(
        ResolutionMaintenanceCounts::default(),
        maintenance.execute(&context(), 100).await.unwrap()
    );
    sqlx::query("UPDATE geo_resolutions SET next_attempt_at=clock_timestamp()-interval '1 second' WHERE resolution_id=$1")
        .bind(retry.id.into_uuid()).execute(&pool).await.unwrap();
    let before: (i32, i32) = sqlx::query_as(
        "SELECT calls,consecutive_failures FROM geo_resolution_provider_controls WHERE profile=$1",
    )
    .bind(PROFILE)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        ResolutionMaintenanceCounts {
            expired: 0,
            requeued: 1
        },
        maintenance.execute(&context(), 100).await.unwrap()
    );
    locked_tx.rollback().await.unwrap();
    let recovered = inspect(&pool, subject).await.unwrap();
    assert_eq!(retry.id, recovered.work.id);
    assert_eq!(retry.generation + 1, recovered.work.generation);
    assert_eq!(1, recovered.attempts);
    assert_eq!(Some("UNAVAILABLE"), recovered.last_failure.as_deref());
    let after: (i32, i32) = sqlx::query_as(
        "SELECT calls,consecutive_failures FROM geo_resolution_provider_controls WHERE profile=$1",
    )
    .bind(PROFILE)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(before, after);
    let wakeups: Vec<i64> = sqlx::query_scalar(
        "SELECT generation FROM geo_resolution_wakeups WHERE resolution_id=$1 ORDER BY generation",
    )
    .bind(retry.id.into_uuid())
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(vec![1, 2], wakeups);
    assert!(matches!(claim(&pool, retry).await, ClaimOutcome::Complete));
    let ClaimOutcome::Claimed(next_claim) = claim(&pool, recovered.work).await else {
        panic!("recovered claim required")
    };
    assert_eq!(2, next_claim.attempt);
}

#[aura_integration_test(services=[SCHEMA], isolated = true)]
async fn negative_results_reuse_until_reconsideration_and_terms_revocation_fails_closed() {
    let pool = get_postgres_client().await;
    configure(&pool).await;
    for outcome in [
        GeocodingOutcome::NoMatch,
        GeocodingOutcome::Candidates(
            CandidateSet::new(
                vec![candidate(CountryCode::DEU), candidate(CountryCode::FRA)],
                5,
            )
            .unwrap(),
        ),
    ] {
        let b = binding();
        let w = bind(&pool, &input(Uuid::now_v7(), "negative"), b).await;
        let p = provider(
            &pool,
            GeocodingResult {
                outcome,
                retention: ResultRetention::Storage,
            },
        )
        .await;
        let h = ResolveHandler::new(
            SqlxUnitOfWork::new(p.pool.clone()),
            SqlxResolutionRepositoryFactory,
            Arc::clone(&p),
            PROFILE.into(),
        );
        h.execute(&context(), w).await.unwrap();
        h.execute(&context(), w).await.unwrap();
        assert_eq!(1, p.calls.load(Ordering::SeqCst));
        assert!(matches!(
            inspect(&pool, b).await.unwrap().status,
            ResolutionStatus::NoMatch | ResolutionStatus::Ambiguous
        ));
    }
    sqlx::query("UPDATE geo_resolution_provider_controls SET storage_permitted=false")
        .execute(&pool)
        .await
        .unwrap();
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    assert_eq!(
        2,
        SqlxResolutionRepositoryFactory
            .resolutions(&mut tx)
            .expire(10)
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    let statuses: Vec<String> = sqlx::query_scalar("SELECT status FROM geo_resolutions")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert!(statuses.iter().all(|s| s == "BLOCKED"));
    let retained: i64 =
        sqlx::query_scalar("SELECT count(*) FROM geo_resolutions WHERE evidence IS NOT NULL")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(0, retained);
}

#[aura_integration_test(services=[SCHEMA], isolated = true)]
async fn force_requeue_fences_an_active_call_and_unchanged_completed_retry_costs_nothing() {
    let pool = get_postgres_client().await;
    configure(&pool).await;
    let b = binding();
    let a = input(Uuid::now_v7(), "retry");
    let w = bind(&pool, &a, b).await;
    let ClaimOutcome::Claimed(old) = claim(&pool, w).await else {
        panic!("claim required")
    };
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    SqlxResolutionRepositoryFactory
        .resolutions(&mut tx)
        .requeue(b, w, true)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let current = inspect(&pool, b).await.unwrap();
    assert_eq!(2, current.work.generation);
    // The invalidated call still owns its concurrency reservation while in flight.
    assert!(matches!(
        claim(&pool, current.work).await,
        ClaimOutcome::Deferred
    ));
    assert!(
        !complete(
            &pool,
            &old,
            ResolutionCompletion::Evidence(evaluate(&a, result(), PROFILE).unwrap())
        )
        .await
    );
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    assert!(matches!(
        SqlxResolutionRepositoryFactory
            .resolutions(&mut tx)
            .requeue(b, w, true)
            .await,
        Err(ResolutionError::Conflict)
    ));
    drop(tx);
    let ClaimOutcome::Claimed(new) = claim(&pool, current.work).await else {
        panic!("claim required")
    };
    complete(
        &pool,
        &new,
        ResolutionCompletion::Evidence(evaluate(&a, result(), PROFILE).unwrap()),
    )
    .await;
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    SqlxResolutionRepositoryFactory
        .resolutions(&mut tx)
        .requeue(b, current.work, false)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(current.work, inspect(&pool, b).await.unwrap().work);
}

#[aura_integration_test(services=[SCHEMA], isolated = true)]
async fn accepted_evidence_and_its_signal_roll_back_together_when_journaling_fails() {
    let pool = get_postgres_client().await;
    configure(&pool).await;
    let b = binding();
    let a = input(Uuid::now_v7(), "transactional input");
    let w = bind(&pool, &a, b).await;
    let ClaimOutcome::Claimed(c) = claim(&pool, w).await else {
        panic!("claim required");
    };
    sqlx::query("ALTER TABLE geo_resolution_changes ADD CONSTRAINT geo_test_reject_resolved CHECK(change_kind <> 'RESOLVED')")
        .execute(&pool).await.unwrap();
    let evidence = evaluate(&a, result(), PROFILE).unwrap();
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    assert!(matches!(
        SqlxResolutionRepositoryFactory
            .resolutions(&mut tx)
            .complete(&c, ResolutionCompletion::Evidence(evidence.clone()))
            .await,
        Err(ResolutionError::Storage(_))
    ));
    drop(tx);
    let current = inspect(&pool, b).await.unwrap();
    assert_eq!(ResolutionStatus::Pending, current.status);
    assert!(current.evidence.is_none());
    let signals: i64 = sqlx::query_scalar("SELECT count(*) FROM geo_resolution_changes")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(0, signals);
    sqlx::query("ALTER TABLE geo_resolution_changes DROP CONSTRAINT geo_test_reject_resolved")
        .execute(&pool)
        .await
        .unwrap();
    assert!(complete(&pool, &c, ResolutionCompletion::Evidence(evidence)).await);
    assert!(
        inspect(&pool, b)
            .await
            .unwrap()
            .accepted_geography()
            .is_some()
    );
    let signals: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM geo_resolution_changes WHERE change_kind='RESOLVED'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(1, signals);
}

#[aura_integration_test(services=[SCHEMA], isolated = true)]
async fn corrupt_evidence_cannot_invent_an_automatically_accepted_precise_point() {
    let pool = get_postgres_client().await;
    configure(&pool).await;
    let b = binding();
    let a = input(Uuid::now_v7(), "private input");
    let w = bind(&pool, &a, b).await;
    let ClaimOutcome::Claimed(c) = claim(&pool, w).await else {
        panic!("claim required");
    };
    assert!(
        complete(
            &pool,
            &c,
            ResolutionCompletion::Evidence(evaluate(&a, result(), PROFILE).unwrap())
        )
        .await
    );
    sqlx::query("UPDATE geo_resolutions SET evidence=jsonb_set(evidence,'{accepted_index}','0'::jsonb) WHERE resolution_id=$1")
        .bind(w.id.into_uuid()).execute(&pool).await.unwrap();
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    assert!(matches!(
        SqlxResolutionRepositoryFactory
            .resolution_reader(&mut tx)
            .inspect(b.subject)
            .await,
        Err(ResolutionError::InvalidState(_))
    ));
}

#[aura_integration_test(services=[SCHEMA], isolated = true)]
async fn corrupt_persisted_input_is_rejected_through_geography_constructors() {
    let pool = get_postgres_client().await;
    configure(&pool).await;
    let b = binding();
    let w = bind(&pool, &input(Uuid::now_v7(), "private input"), b).await;
    sqlx::query("UPDATE geo_resolutions SET input=jsonb_set(input,'{query}','\"\"'::jsonb) WHERE resolution_id=$1").bind(w.id.into_uuid()).execute(&pool).await.unwrap();
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    assert!(matches!(
        SqlxResolutionRepositoryFactory
            .resolutions(&mut tx)
            .claim(w, PROFILE)
            .await,
        Err(ResolutionError::InvalidState(_))
    ));
}

#[aura_integration_test(services=[SCHEMA], isolated = true)]
async fn private_postal_assertions_survive_precise_and_administrative_acceptance() {
    let pool = get_postgres_client().await;
    configure(&pool).await;
    let owner = Uuid::now_v7();
    for precision in [SpatialPrecision::Premises, SpatialPrecision::Locality] {
        let description = GeographicDescription::new(
            Some(AddressText::new(format!("Synthetic postal input {precision:?}")).unwrap()),
            Some(CountryCode::GBR),
            None,
            Some(geo::PostalCode::new("SW1A 1AA").unwrap()),
        )
        .unwrap()
        .unwrap();
        let input = ResolutionInput::new(
            GeocodingRequest::new(description.address_text().unwrap().clone())
                .with_submitted(geo::SubmittedGeography::new(description.clone())),
            ResolutionScope::new(ScopeKind::User, Some(owner)).unwrap(),
            RetentionPurpose::UserLocation,
        )
        .unwrap();
        let b = ResolutionBinding {
            subject: ResolutionSubject::new(RetentionPurpose::UserLocation, Uuid::now_v7())
                .unwrap(),
            input_revision: 1,
            guard: 1,
        };
        let w = bind(&pool, &input, b).await;
        let ClaimOutcome::Claimed(c) = claim(&pool, w).await else {
            panic!("claim required")
        };
        let source = GeocodingCandidate::new(
            DerivedGeography::new(
                GeographicDescription::new(
                    description.address_text().cloned(),
                    description.country(),
                    description.subdivision(),
                    Some(geo::PostalCode::new("sw1a1aa").unwrap()),
                )
                .unwrap(),
                Some(
                    SpatialPosition::new(GeoPoint::new(51.5, -0.1).unwrap(), precision, None)
                        .unwrap(),
                ),
            ),
            vec![],
            None,
            candidate(CountryCode::GBR).provenance().clone(),
        );
        let e = evaluate(
            &input,
            GeocodingResult {
                outcome: GeocodingOutcome::Candidates(CandidateSet::new(vec![source], 5).unwrap()),
                retention: ResultRetention::Storage,
            },
            PROFILE,
        )
        .unwrap();
        let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
        assert!(
            SqlxResolutionRepositoryFactory
                .resolutions(&mut tx)
                .complete(&c, ResolutionCompletion::Evidence(e))
                .await
                .unwrap()
        );
        tx.commit().await.unwrap();
        let stored = inspect(&pool, b).await.unwrap().evidence.unwrap();
        assert_eq!(ResolutionStatus::Accepted, stored.status);
        let accepted = &stored.candidates[stored.accepted_index.unwrap()].geography;
        assert_eq!(
            Some(CountryCode::GBR),
            accepted.description().unwrap().country()
        );
        assert_eq!(
            precision == SpatialPrecision::Premises,
            accepted.position().is_some()
        );
        if precision == SpatialPrecision::Locality {
            assert!(accepted.description().unwrap().postal_code().is_none());
            assert!(accepted.description().unwrap().address_text().is_none());
        }
    }
}

#[aura_integration_test(services=[SCHEMA], isolated = true)]
async fn bounded_resolution_reads_preserve_order_missing_entries_and_duplicates() {
    let pool = get_postgres_client().await;
    configure(&pool).await;
    let a = binding();
    let b = binding();
    let missing = binding();
    let owner = Uuid::now_v7();
    let first = bind(&pool, &input(owner, "Synthetic first batch input"), a).await;
    let second = bind(&pool, &input(owner, "Synthetic second batch input"), b).await;
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    let mut reader = SqlxResolutionRepositoryFactory.resolution_reader(&mut tx);
    let entries = reader
        .inspect_many(&[b.subject, missing.subject, a.subject, b.subject])
        .await
        .unwrap();
    assert_eq!(4, entries.len());
    assert_eq!(second, entries[0].as_ref().unwrap().work);
    assert!(entries[1].is_none());
    assert_eq!(first, entries[2].as_ref().unwrap().work);
    assert_eq!(second, entries[3].as_ref().unwrap().work);
    assert!(reader.inspect_many(&[]).await.unwrap().is_empty());
    assert!(matches!(
        reader.inspect_many(&[a.subject; 101]).await,
        Err(ResolutionError::InvalidInput)
    ));
}

#[aura_integration_test(services=[SCHEMA], isolated = true)]
async fn repeatable_read_only_snapshot_keeps_permission_and_logical_expiry_gates() {
    let pool = get_postgres_client().await;
    configure(&pool).await;
    let i = input(Uuid::now_v7(), "Synthetic snapshot site");
    let b = binding();
    let work = bind(&pool, &i, b).await;
    let ClaimOutcome::Claimed(c) = claim(&pool, work).await else {
        panic!("claim required")
    };
    assert!(
        complete(
            &pool,
            &c,
            ResolutionCompletion::Evidence(evaluate(&i, result(), PROFILE).unwrap())
        )
        .await
    );
    for state in ["accepted", "expired", "revoked"] {
        if state == "expired" {
            sqlx::query(
                "UPDATE geo_resolutions SET valid_until=clock_timestamp()-interval '1 second'",
            )
            .execute(&pool)
            .await
            .unwrap();
        }
        if state == "revoked" {
            sqlx::query(
                "UPDATE geo_resolutions SET valid_until=clock_timestamp()+interval '1 hour'",
            )
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query("UPDATE geo_resolution_provider_controls SET storage_permitted=false")
                .execute(&pool)
                .await
                .unwrap();
        }
        let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(tx.connection())
            .await
            .unwrap();
        let snapshots = SqlxResolutionRepositoryFactory::snapshot_reader(tx.connection())
            .inspect_many(&[b.subject, binding().subject])
            .await
            .unwrap();
        assert_eq!(2, snapshots.len());
        assert!(snapshots[1].is_none());
        assert_eq!(
            state == "accepted",
            snapshots[0]
                .as_ref()
                .unwrap()
                .accepted_geography()
                .is_some()
        );
        tx.commit().await.unwrap();
    }
}
