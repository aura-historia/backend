use application::{
    operation_context::{OperationContext, Principal},
    transaction::{Transaction, UnitOfWork},
};
use geo::{AddressText, CountryCode, GeoPoint, GeographicDescription, SpatialPosition};
use geo_postgres::SqlxResolutionRepositoryFactory;
use geo_service::{geocoding::*, resolution::*};
use party_core::{
    party::{NewParty, PartyContact},
    party_id::PartyId,
    party_location::{PartyLocationContent, PartyLocationLabel},
    party_location_id::PartyLocationId,
    party_name::PartyName,
};
use party_postgres::{
    SqlxPartyLocationAccessFactory, SqlxPartyLocationRepositoryFactory,
    SqlxPartyLocationResolutionReviewReaderFactory, SqlxPartyRepositoryFactory,
};
use party_service::{
    party_creation::insert_new_party,
    ports::{PartyRepositoryFactory, party_location::*},
    use_cases::{party_location_resolution::*, party_locations::*},
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
const PROFILE: &str = "AMAZON_LOCATION_PLACES_V2_DEFAULT_FRANKFURT_2026_10";

/// Administrator role as persisted, like the production admin check.
struct DatabaseAdmin(PgPool);
#[async_trait::async_trait]
impl CheckUserAdminUseCase for DatabaseAdmin {
    async fn execute(
        &self,
        context: &OperationContext,
        _: CheckUserAdminRequest,
    ) -> Result<CheckUserAdminResult, CheckUserAdminError> {
        let Principal::User(user) = context.principal else {
            return Err(CheckUserAdminError::Forbidden);
        };
        let admin: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM users WHERE user_id=$1 AND role='ADMIN')",
        )
        .bind(user.into_uuid())
        .fetch_one(&self.0)
        .await
        .unwrap();
        admin
            .then_some(CheckUserAdminResult)
            .ok_or(CheckUserAdminError::Forbidden)
    }
}
fn context(principal: Principal) -> OperationContext {
    OperationContext {
        principal,
        request_id: "resolution-review-test".into(),
        correlation_id: "resolution-review-test".into(),
    }
}
fn trusted() -> OperationContext {
    context(Principal::Service("resolution-review-test".to_owned()))
}
async fn user(pool: &PgPool, role: &str) -> UserId {
    let user = UserId::new();
    sqlx::query("INSERT INTO users(user_id,email,tier,role) VALUES ($1,$2,'FREE',$3)")
        .bind(user.into_uuid())
        .bind(format!("{user}@example.test"))
        .bind(role)
        .execute(pool)
        .await
        .unwrap();
    user
}
async fn party(pool: &PgPool) -> PartyId {
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    let stored = insert_new_party(
        &mut SqlxPartyRepositoryFactory.in_transaction(&mut tx),
        NewParty {
            id: PartyId::new(),
            name: PartyName::try_from("Resolution review").unwrap(),
            contact: PartyContact::default(),
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    stored.party.id()
}
async fn location(
    pool: &PgPool,
    party_id: PartyId,
    address: &str,
    point: bool,
) -> PartyLocationView {
    CreatePartyLocationHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxPartyLocationRepositoryFactory,
        SqlxPartyLocationAccessFactory,
        DatabaseAdmin(pool.clone()),
    )
    .execute(
        &trusted(),
        CreatePartyLocationCommand {
            party_id,
            content: PartyLocationContent {
                label: PartyLocationLabel::new(address.to_owned()).unwrap(),
                roles: BTreeSet::new(),
                geography: GeographicDescription::new(
                    Some(AddressText::new(address.to_owned()).unwrap()),
                    Some(CountryCode::DEU),
                    None,
                    None,
                )
                .unwrap(),
                position: point.then(|| {
                    SpatialPosition::new(
                        GeoPoint::new(52.5, 13.4).unwrap(),
                        geo::SpatialPrecision::Premises,
                        None,
                    )
                    .unwrap()
                }),
            },
            evidence: None,
            relocates: None,
        },
    )
    .await
    .unwrap()
}
fn subject(id: PartyLocationId) -> ResolutionSubject {
    ResolutionSubject::new(RetentionPurpose::PartyLocation, id.into_uuid()).unwrap()
}
async fn resolution(pool: &PgPool, id: PartyLocationId) -> ResolutionView {
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    let view = SqlxResolutionRepositoryFactory
        .resolution_reader(&mut tx)
        .inspect(subject(id))
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();
    view
}
fn candidate() -> GeocodingCandidate {
    GeocodingCandidate::new(
        geo::DerivedGeography::new(
            GeographicDescription::new(None, Some(CountryCode::DEU), None, None).unwrap(),
            Some(
                SpatialPosition::new(
                    GeoPoint::new(52.5, 13.4).unwrap(),
                    geo::SpatialPrecision::Premises,
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
            external_reference: "private provider record".into(),
            attribution_url: "https://example.test/attribution",
        },
    )
}
/// Records provider evidence with two acceptable premises candidates.
async fn make_ambiguous(pool: &PgPool, id: PartyLocationId) {
    let work = resolution(pool, id).await.work;
    let uow = SqlxUnitOfWork::new(pool.clone());
    let mut tx = uow.begin().await.unwrap();
    let ClaimOutcome::Claimed(claim) = SqlxResolutionRepositoryFactory
        .resolutions(&mut tx)
        .claim(work, PROFILE)
        .await
        .unwrap()
    else {
        panic!("expected claim")
    };
    tx.commit().await.unwrap();
    let evidence = evaluate(
        &claim.input,
        GeocodingResult {
            outcome: GeocodingOutcome::Candidates(
                CandidateSet::new(
                    vec![candidate(), candidate()],
                    claim.input.request().candidate_limit(),
                )
                .unwrap(),
            ),
            retention: ResultRetention::Storage,
        },
        PROFILE,
    )
    .unwrap();
    let mut tx = uow.begin().await.unwrap();
    assert_eq!(
        CompletionOutcome::Recorded(ResolutionStatus::Ambiguous),
        SqlxResolutionRepositoryFactory
            .resolutions(&mut tx)
            .complete(&claim, ResolutionCompletion::Evidence(evidence))
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
}
async fn set_failed_status(pool: &PgPool, id: PartyLocationId, status: &str) {
    let work = resolution(pool, id).await.work;
    sqlx::query(
        "UPDATE geo_resolutions SET status=$2,last_failure='CONFIGURATION' WHERE resolution_id=$1",
    )
    .bind(work.id.into_uuid())
    .bind(status)
    .execute(pool)
    .await
    .unwrap();
}
fn list(pool: &PgPool) -> impl ListPartyLocationResolutionReviewsUseCase {
    ListPartyLocationResolutionReviewsHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxPartyLocationResolutionReviewReaderFactory,
        SqlxPartyLocationAccessFactory,
        DatabaseAdmin(pool.clone()),
    )
}
fn admin_review(pool: &PgPool) -> impl ManagePartyLocationResolutionUseCase {
    ManagePartyLocationResolutionHandler::new_admin_only(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxPartyLocationRepositoryFactory,
        SqlxPartyLocationAccessFactory,
        DatabaseAdmin(pool.clone()),
    )
}
async fn queue(
    pool: &PgPool,
    status: Option<ResolutionAttention>,
    limit: u32,
) -> Vec<(PartyLocationId, ResolutionStatus)> {
    let mut seen = vec![];
    let mut after = None;
    loop {
        let page = list(pool)
            .execute(
                &trusted(),
                ListPartyLocationResolutionReviewsRequest {
                    status,
                    after,
                    limit,
                },
            )
            .await
            .unwrap();
        assert!(page.items.len() <= limit as usize);
        seen.extend(
            page.items
                .iter()
                .map(|item| (item.location.id, item.resolution.status)),
        );
        match page.next {
            Some(next) => after = Some(next),
            None => return seen,
        }
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_page_unreviewed_locations_needing_attention_across_parties() {
    let pool = get_postgres_client().await;
    let (first, second) = (party(&pool).await, party(&pool).await);
    let ambiguous = location(&pool, first, "1 Ambiguous Street", false).await;
    // Pending work and an explicit point (no binding) never enter the queue.
    location(&pool, first, "2 Pending Street", false).await;
    let blocked = location(&pool, second, "3 Blocked Street", false).await;
    location(&pool, second, "4 Explicit Street", true).await;
    let failed = location(&pool, second, "5 Failed Street", false).await;
    make_ambiguous(&pool, ambiguous.id).await;
    set_failed_status(&pool, blocked.id, "BLOCKED").await;
    set_failed_status(&pool, failed.id, "FAILED").await;

    let expected = vec![
        (ambiguous.id, ResolutionStatus::Ambiguous),
        (blocked.id, ResolutionStatus::Blocked),
        (failed.id, ResolutionStatus::Failed),
    ];
    for limit in [1, 2, 100] {
        assert_eq!(expected, queue(&pool, None, limit).await, "limit {limit}");
    }
    assert_eq!(
        vec![(blocked.id, ResolutionStatus::Blocked)],
        queue(&pool, Some(ResolutionAttention::Blocked), 100).await
    );
    let page = list(&pool)
        .execute(
            &trusted(),
            ListPartyLocationResolutionReviewsRequest {
                status: Some(ResolutionAttention::Ambiguous),
                after: None,
                limit: 100,
            },
        )
        .await
        .unwrap();
    let item = &page.items[0];
    assert_eq!(ambiguous.revision, item.location.revision);
    assert_eq!(
        2,
        item.resolution.evidence.as_ref().unwrap().candidates.len()
    );

    // A reviewed selection settles the ambiguity; Retry of a blocked resolution reopens work.
    let w = item.resolution.work;
    admin_review(&pool)
        .execute(
            &trusted(),
            PartyLocationResolutionCommand::AcceptCandidate {
                party_id: first,
                expected: ExpectedPartyLocation {
                    id: ambiguous.id,
                    revision: ambiguous.revision,
                },
                work: w,
                candidate_index: 1,
            },
        )
        .await
        .unwrap();
    let blocked_work = resolution(&pool, blocked.id).await.work;
    let retried = admin_review(&pool)
        .execute(
            &trusted(),
            PartyLocationResolutionCommand::Retry {
                party_id: second,
                expected: ExpectedPartyLocation {
                    id: blocked.id,
                    revision: blocked.revision,
                },
                work: blocked_work,
                force: false,
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ResolutionStatus::Pending, retried.status);
    assert_eq!(blocked_work.generation + 1, retried.work.generation);
    assert_eq!(
        vec![(failed.id, ResolutionStatus::Failed)],
        queue(&pool, None, 100).await
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_restrict_admin_review_to_administrators_even_for_location_managers() {
    let pool = get_postgres_client().await;
    let owner = party(&pool).await;
    let site = location(&pool, owner, "6 Review Street", false).await;
    let manager = user(&pool, "USER").await;
    let admin = user(&pool, "ADMIN").await;
    GrantPartyLocationManagementHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxPartyLocationAccessFactory,
        DatabaseAdmin(pool.clone()),
    )
    .execute(
        &trusted(),
        GrantPartyLocationManagementCommand {
            party_id: owner,
            user_id: manager,
            granted: true,
        },
    )
    .await
    .unwrap();
    let inspect = || PartyLocationResolutionCommand::Inspect {
        party_id: owner,
        id: site.id,
    };
    let shared_policy = ManagePartyLocationResolutionHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxPartyLocationRepositoryFactory,
        SqlxPartyLocationAccessFactory,
        DatabaseAdmin(pool.clone()),
    );
    let as_manager = context(Principal::User(manager));
    assert!(
        shared_policy
            .execute(&as_manager, inspect())
            .await
            .unwrap()
            .is_some()
    );
    assert!(matches!(
        admin_review(&pool).execute(&as_manager, inspect()).await,
        Err(PartyLocationError::Forbidden)
    ));
    let as_admin = context(Principal::User(admin));
    assert!(
        admin_review(&pool)
            .execute(&as_admin, inspect())
            .await
            .unwrap()
            .is_some()
    );
    let request = || ListPartyLocationResolutionReviewsRequest {
        status: None,
        after: None,
        limit: 10,
    };
    assert!(matches!(
        list(&pool).execute(&as_manager, request()).await,
        Err(PartyLocationError::Forbidden)
    ));
    assert!(list(&pool).execute(&as_admin, request()).await.is_ok());
    sqlx::query("UPDATE users SET suspended=true WHERE user_id=$1")
        .bind(admin.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        list(&pool).execute(&as_admin, request()).await,
        Err(PartyLocationError::Forbidden)
    ));
}
