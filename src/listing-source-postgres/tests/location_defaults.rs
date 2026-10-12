use application::{
    operation_context::{CredentialCapability, OperationContext, Principal},
    patch_field::PatchField,
    transaction::{Transaction, UnitOfWork},
};
use geo::{
    AddressText, CountryCode, GeoPoint, GeographicDescription, SpatialPosition, SpatialPrecision,
    data::geography_data::GeographicDescriptionData,
};
use listing_source_core::{ListingSourceId, location_default::*};
use listing_source_postgres::*;
use listing_source_service::{ports::location_default::*, use_cases::location_default::*};
use party_core::{party_id::PartyId, party_location_id::PartyLocationId};
use platform_postgres::SqlxUnitOfWork;
use sqlx::PgPool;
use std::collections::BTreeSet;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
fn context(principal: Principal) -> OperationContext {
    OperationContext {
        principal,
        request_id: "default-test".into(),
        correlation_id: "default-test".into(),
    }
}
fn trusted() -> OperationContext {
    context(Principal::Service("default-test".into()))
}
fn assign(pool: &PgPool) -> impl AssignLocationDefaultUseCase {
    AssignLocationDefaultHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxLocationDefaultRepositoryFactory,
        SqlxSourceLocationAccessFactory,
    )
}
fn clear(pool: &PgPool) -> impl ClearLocationDefaultUseCase {
    ClearLocationDefaultHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxLocationDefaultRepositoryFactory,
        SqlxSourceLocationAccessFactory,
    )
}
fn change(pool: &PgPool) -> impl ChangeLocationDefaultApplicabilityUseCase {
    ChangeLocationDefaultApplicabilityHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxLocationDefaultRepositoryFactory,
        SqlxSourceLocationAccessFactory,
    )
}
fn get(pool: &PgPool) -> impl GetLocationDefaultUseCase {
    GetLocationDefaultHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxLocationDefaultReaderFactory,
        SqlxSourceLocationAccessFactory,
    )
}
fn admin_assign(pool: &PgPool) -> impl AssignLocationDefaultUseCase {
    AssignLocationDefaultHandler::new_admin_only(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxLocationDefaultRepositoryFactory,
        SqlxSourceLocationAccessFactory,
    )
}
fn admin_clear(pool: &PgPool) -> impl ClearLocationDefaultUseCase {
    ClearLocationDefaultHandler::new_admin_only(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxLocationDefaultRepositoryFactory,
        SqlxSourceLocationAccessFactory,
    )
}
fn admin_change(pool: &PgPool) -> impl ChangeLocationDefaultApplicabilityUseCase {
    ChangeLocationDefaultApplicabilityHandler::new_admin_only(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxLocationDefaultRepositoryFactory,
        SqlxSourceLocationAccessFactory,
    )
}
fn admin_get(pool: &PgPool) -> impl GetLocationDefaultUseCase {
    GetLocationDefaultHandler::new_admin_only(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxLocationDefaultReaderFactory,
        SqlxSourceLocationAccessFactory,
    )
}
async fn party(pool: &PgPool) -> PartyId {
    let id = PartyId::new();
    sqlx::query(
        "INSERT INTO parties(party_id,party_slug_id,name) VALUES ($1,$2,'Default operator')",
    )
    .bind(id.into_uuid())
    .bind(format!("operator-{}", id.as_uuid().simple()))
    .execute(pool)
    .await
    .unwrap();
    id
}
async fn source(pool: &PgPool, owner: PartyId) -> ListingSourceId {
    let id = ListingSourceId::new();
    sqlx::query("INSERT INTO listing_sources(listing_source_id,listing_source_slug_id,name,operator_party_id) VALUES($1,$2,'Default source',$3)").bind(id.into_uuid()).bind(format!("source-{}",id.as_uuid().simple())).bind(owner.into_uuid()).execute(pool).await.unwrap();
    id
}
async fn location(pool: &PgPool, owner: PartyId) -> PartyLocationId {
    let id = PartyLocationId::new();
    let description = GeographicDescription::new(
        Some(AddressText::new("Sensitive premises").unwrap()),
        Some(CountryCode::DEU),
        None,
        None,
    )
    .unwrap()
    .unwrap();
    let position = SpatialPosition::new(
        GeoPoint::new(52.5, 13.4).unwrap(),
        SpatialPrecision::Premises,
        None,
    )
    .unwrap();
    let point = geo::data::geography_data::SpatialPositionData::from(position);
    sqlx::query("INSERT INTO party_locations(party_location_id,party_id,label,lifecycle,geography,position,created_actor,updated_actor) VALUES($1,$2,'Sensitive label','ACTIVE',$3,$4,'test','test')").bind(id.into_uuid()).bind(owner.into_uuid()).bind(serde_json::to_value(GeographicDescriptionData::from(&description)).unwrap()).bind(serde_json::to_value(point).unwrap()).execute(pool).await.unwrap();
    id
}
fn evidence(scope: SourceLocationMeaning) -> SourceLocationEvidence {
    SourceLocationEvidence::new("Explicit source observation".into(), scope).unwrap()
}
fn command(
    source_id: ListingSourceId,
    location_id: PartyLocationId,
) -> AssignLocationDefaultCommand {
    AssignLocationDefaultCommand {
        source_id,
        expected_revision: LocationDefaultRevision::INITIAL,
        assignment: SourceLocationAssignment::new(
            location_id,
            Some(evidence(SourceLocationMeaning::AssociatedBusiness)),
            None,
        )
        .unwrap(),
    }
}
async fn read(pool: &PgPool, source_id: ListingSourceId) -> LocationDefaultView {
    get(pool)
        .execute(&trusted(), GetLocationDefaultRequest { source_id })
        .await
        .unwrap()
}
async fn signals(pool: &PgPool, id: ListingSourceId) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM listing_source_location_default_changes WHERE listing_source_id=$1",
    )
    .bind(id.into_uuid())
    .fetch_one(pool)
    .await
    .unwrap()
}
#[tokio::test]
async fn invalid_revisions_are_rejected_before_opening_a_transaction() {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://postgres:postgres@127.0.0.1:1/postgres")
        .unwrap();
    pool.close().await;
    let source_id = ListingSourceId::new();
    let expected_revision = LocationDefaultRevision::try_from(i64::MAX as u64 + 1).unwrap();
    let mut assignment = command(source_id, PartyLocationId::new());
    assignment.expected_revision = expected_revision;
    assert!(matches!(
        assign(&pool).execute(&trusted(), assignment).await,
        Err(LocationDefaultError::InvalidRevision)
    ));
    assert!(matches!(
        clear(&pool)
            .execute(
                &trusted(),
                ClearLocationDefaultCommand {
                    source_id,
                    expected_revision
                }
            )
            .await,
        Err(LocationDefaultError::InvalidRevision)
    ));
    assert!(matches!(
        change(&pool)
            .execute(
                &trusted(),
                ChangeLocationDefaultApplicabilityCommand {
                    source_id,
                    expected_revision,
                    business_association: PatchField::Unchanged,
                    advertised_inventory: PatchField::Unchanged,
                }
            )
            .await,
        Err(LocationDefaultError::InvalidRevision)
    ));
}
#[aura_integration_test(services=[BUSINESS_SCHEMA])]
async fn separate_sources_select_one_location_and_changes_are_atomic_noops() {
    let pool = get_postgres_client().await;
    let owner = party(&pool).await;
    let x = source(&pool, owner).await;
    let y = source(&pool, owner).await;
    let a = location(&pool, owner).await;
    let b = location(&pool, owner).await;
    assert!(read(&pool, x).await.assignment.is_none());
    assert_eq!(0, signals(&pool, x).await);
    let original = assign(&pool)
        .execute(&trusted(), command(x, a))
        .await
        .unwrap();
    assign(&pool)
        .execute(&trusted(), command(y, b))
        .await
        .unwrap();
    assert_eq!(a, read(&pool, x).await.assignment.unwrap().location_id());
    assert_eq!(b, read(&pool, y).await.assignment.unwrap().location_id());
    let mut same = command(x, a);
    same.expected_revision = original.revision;
    assert_eq!(
        original,
        assign(&pool).execute(&trusted(), same).await.unwrap()
    );
    assert_eq!(1, signals(&pool, x).await);
    assert_eq!(
        original,
        change(&pool)
            .execute(
                &trusted(),
                ChangeLocationDefaultApplicabilityCommand {
                    source_id: x,
                    expected_revision: original.revision,
                    business_association: PatchField::Unchanged,
                    advertised_inventory: PatchField::Unchanged,
                },
            )
            .await
            .unwrap()
    );
    assert_eq!(1, signals(&pool, x).await);
    assert!(matches!(
        assign(&pool).execute(&trusted(), command(x, a)).await,
        Err(LocationDefaultError::ConcurrencyConflict)
    ));
    let with_inventory = change(&pool)
        .execute(
            &trusted(),
            ChangeLocationDefaultApplicabilityCommand {
                source_id: x,
                expected_revision: original.revision,
                business_association: PatchField::Unchanged,
                advertised_inventory: PatchField::Set(evidence(
                    SourceLocationMeaning::AdvertisedInventory,
                )),
            },
        )
        .await
        .unwrap();
    assert!(
        with_inventory
            .assignment
            .as_ref()
            .unwrap()
            .advertised_inventory()
            .is_some()
    );
    let business_only = change(&pool)
        .execute(
            &trusted(),
            ChangeLocationDefaultApplicabilityCommand {
                source_id: x,
                expected_revision: with_inventory.revision,
                business_association: PatchField::Unchanged,
                advertised_inventory: PatchField::Clear,
            },
        )
        .await
        .unwrap();
    let mut replacement = command(x, b);
    replacement.expected_revision = business_only.revision;
    let replaced = assign(&pool)
        .execute(&trusted(), replacement)
        .await
        .unwrap();
    assert_eq!(b, replaced.assignment.unwrap().location_id());
    let cleared = clear(&pool)
        .execute(
            &trusted(),
            ClearLocationDefaultCommand {
                source_id: x,
                expected_revision: replaced.revision,
            },
        )
        .await
        .unwrap();
    assert!(cleared.assignment.is_none());
    assert_eq!(
        cleared,
        clear(&pool)
            .execute(
                &trusted(),
                ClearLocationDefaultCommand {
                    source_id: x,
                    expected_revision: cleared.revision
                }
            )
            .await
            .unwrap()
    );
    assert!(matches!(
        clear(&pool)
            .execute(
                &trusted(),
                ClearLocationDefaultCommand {
                    source_id: x,
                    expected_revision: replaced.revision
                }
            )
            .await,
        Err(LocationDefaultError::ConcurrencyConflict)
    ));
    assert_eq!(5, signals(&pool, x).await);
    let forbidden_delete = sqlx::query("DELETE FROM party_locations WHERE party_location_id=$1")
        .bind(b.into_uuid())
        .execute(&pool)
        .await
        .unwrap_err();
    assert!(
        forbidden_delete
            .as_database_error()
            .unwrap()
            .is_foreign_key_violation()
    );

    let historical: uuid::Uuid = sqlx::query_scalar("SELECT party_location_id FROM listing_source_location_default_changes WHERE listing_source_id=$1 ORDER BY revision LIMIT 1").bind(x.into_uuid()).fetch_one(&pool).await.unwrap();
    assert_eq!(a.into_uuid(), historical);
    // Owned configuration disappears with the source; immutable invalidation history remains.
    sqlx::query("DELETE FROM listing_sources WHERE listing_source_id=$1")
        .bind(x.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(5, signals(&pool, x).await);
    assert!(matches!(
        SqlxEffectiveSourceGeographyReader::new(pool.clone())
            .get(x)
            .await,
        Err(LocationDefaultError::SourceNotFound)
    ));
}
#[aura_integration_test(services=[BUSINESS_SCHEMA])]
async fn foreign_missing_retired_and_unscoped_references_fail_closed() {
    let pool = get_postgres_client().await;
    let owner = party(&pool).await;
    let other = party(&pool).await;
    let id = source(&pool, owner).await;
    let foreign = location(&pool, other).await;
    let retired = location(&pool, owner).await;
    sqlx::query("UPDATE party_locations SET lifecycle='RETIRED' WHERE party_location_id=$1")
        .bind(retired.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    for location in [foreign, PartyLocationId::new()] {
        assert!(matches!(
            assign(&pool)
                .execute(&trusted(), command(id, location))
                .await,
            Err(LocationDefaultError::LocationNotFound)
        ));
    }
    assert!(matches!(
        assign(&pool)
            .execute(&trusted(), command(id, retired))
            .await,
        Err(LocationDefaultError::LocationRetired)
    ));
    assert!(matches!(
        assign(&pool)
            .execute(&trusted(), command(ListingSourceId::new(), retired))
            .await,
        Err(LocationDefaultError::SourceNotFound)
    ));
    let foreign_write = sqlx::query("INSERT INTO listing_source_location_defaults(listing_source_id,party_id,party_location_id,revision,updated_actor) VALUES($1,$2,$3,2,'test')").bind(id.into_uuid()).bind(owner.into_uuid()).bind(foreign.into_uuid()).execute(&pool).await.unwrap_err();
    assert!(
        foreign_write
            .as_database_error()
            .unwrap()
            .is_foreign_key_violation()
    );
    let active = location(&pool, owner).await;
    let v = assign(&pool)
        .execute(&trusted(), command(id, active))
        .await
        .unwrap();
    assert!(matches!(
        change(&pool)
            .execute(
                &trusted(),
                ChangeLocationDefaultApplicabilityCommand {
                    source_id: id,
                    expected_revision: v.revision,
                    business_association: PatchField::Unchanged,
                    advertised_inventory: PatchField::Set(evidence(
                        SourceLocationMeaning::AssociatedBusiness
                    ))
                }
            )
            .await,
        Err(LocationDefaultError::InvalidAssignment(_))
    ));
    assert_eq!(v, read(&pool, id).await);
    assert_eq!(1, signals(&pool, id).await);
}
#[aura_integration_test(services=[BUSINESS_SCHEMA])]
async fn authoritative_geography_checks_independent_meanings_exact_points_retirement_and_pending_resolution()
 {
    let pool = get_postgres_client().await;
    let owner = party(&pool).await;
    let id = source(&pool, owner).await;
    let a = location(&pool, owner).await;
    let _alternative = location(&pool, owner).await;
    let reader = SqlxEffectiveSourceGeographyReader::new(pool.clone());
    assert_eq!(
        EffectiveSourceGeographyView::default(),
        reader.get(id).await.unwrap()
    );
    let mut c = command(id, a);
    let v = assign(&pool).execute(&trusted(), c.clone()).await.unwrap();
    let view = reader.get(id).await.unwrap();
    assert!(view.advertised_inventory.is_none());
    let business = view.business_association.unwrap();
    assert_eq!("SOURCE_DEFAULT_DERIVED", business.basis.kind());
    assert_eq!(v.revision, business.basis.assignment_revision);
    assert_eq!(1, business.basis.location_revision);
    assert!(business.geography.unwrap().position().is_some());
    c.expected_revision = v.revision;
    c.assignment = SourceLocationAssignment::new(
        a,
        Some(evidence(SourceLocationMeaning::AssociatedBusiness)),
        Some(evidence(SourceLocationMeaning::AdvertisedInventory)),
    )
    .unwrap();
    assign(&pool).execute(&trusted(), c).await.unwrap();
    assert!(reader.get(id).await.unwrap().advertised_inventory.is_some());
    let inventory = reader.get(id).await.unwrap().advertised_inventory.unwrap();
    let geography = inventory.geography.unwrap();
    assert!(geography.position().is_some());
    assert_eq!(
        Some(CountryCode::DEU),
        geography.description().unwrap().country()
    );
    assert!(geography.description().unwrap().address_text().is_some());
    sqlx::query("UPDATE party_locations SET geography=NULL,position=NULL,revision=revision+1,input_revision=input_revision+1 WHERE party_location_id=$1").bind(a.into_uuid()).execute(&pool).await.unwrap();
    let pending = reader.get(id).await.unwrap().advertised_inventory.unwrap();
    assert!(pending.geography.is_none());
    assert_eq!(a, pending.basis.location_id);
    assert_eq!(2, pending.basis.location_input_revision);
    sqlx::query("UPDATE party_locations SET lifecycle='RETIRED',revision=revision+1 WHERE party_location_id=$1").bind(a.into_uuid()).execute(&pool).await.unwrap();
    assert_eq!(
        EffectiveSourceGeographyView::default(),
        reader.get(id).await.unwrap()
    );
    let retained = read(&pool, id).await;
    assert_eq!(a, retained.assignment.as_ref().unwrap().location_id());
    assert_eq!(
        Some(SelectedLocationEligibility::Retired),
        retained.eligibility
    );
    let mut same = command(id, a);
    same.expected_revision = retained.revision;
    same.assignment = retained.assignment.clone().unwrap();
    assert!(matches!(
        assign(&pool).execute(&trusted(), same).await,
        Err(LocationDefaultError::LocationRetired)
    ));
    assert!(matches!(
        change(&pool)
            .execute(
                &trusted(),
                ChangeLocationDefaultApplicabilityCommand {
                    source_id: id,
                    expected_revision: retained.revision,
                    business_association: PatchField::Unchanged,
                    advertised_inventory: PatchField::Clear,
                }
            )
            .await,
        Err(LocationDefaultError::LocationRetired)
    ));
    sqlx::query("UPDATE party_locations SET lifecycle='ACTIVE',revision=revision+1 WHERE party_location_id=$1")
        .bind(a.into_uuid()).execute(&pool).await.unwrap();
    let restored = reader.get(id).await.unwrap().advertised_inventory.unwrap();
    assert_eq!(a, restored.basis.location_id);
    assert_eq!(retained.revision, restored.basis.assignment_revision);
    assert!(restored.geography.is_none());
    sqlx::query("UPDATE party_locations SET lifecycle='RETIRED',revision=revision+1 WHERE party_location_id=$1")
        .bind(a.into_uuid()).execute(&pool).await.unwrap();
    let cleared = clear(&pool)
        .execute(
            &trusted(),
            ClearLocationDefaultCommand {
                source_id: id,
                expected_revision: retained.revision,
            },
        )
        .await
        .unwrap();
    assert!(cleared.assignment.is_none());
    assert_eq!(
        EffectiveSourceGeographyView::default(),
        reader.get(id).await.unwrap()
    );
}
#[aura_integration_test(services=[BUSINESS_SCHEMA])]
async fn concurrent_changes_have_one_winner_and_invalidation_failure_rolls_back() {
    let pool = get_postgres_client().await;
    let owner = party(&pool).await;
    let id = source(&pool, owner).await;
    let a = location(&pool, owner).await;
    let b = location(&pool, owner).await;
    let ctx = trusted();
    let h1 = assign(&pool);
    let h2 = assign(&pool);
    let (first, second) = tokio::join!(
        h1.execute(&ctx, command(id, a)),
        h2.execute(&ctx, command(id, b))
    );
    assert_eq!(
        1,
        [first.is_ok(), second.is_ok()]
            .into_iter()
            .filter(|v| *v)
            .count()
    );
    assert!(
        matches!(first, Err(LocationDefaultError::ConcurrencyConflict))
            || matches!(second, Err(LocationDefaultError::ConcurrencyConflict))
    );
    let current = read(&pool, id).await;
    sqlx::query("ALTER TABLE listing_source_location_default_changes ADD CONSTRAINT test_signal_failure CHECK (revision <> 3)").execute(&pool).await.unwrap();
    let result = clear(&pool)
        .execute(
            &ctx,
            ClearLocationDefaultCommand {
                source_id: id,
                expected_revision: current.revision,
            },
        )
        .await;
    sqlx::query(
        "ALTER TABLE listing_source_location_default_changes DROP CONSTRAINT test_signal_failure",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(matches!(result, Err(LocationDefaultError::Internal { .. })));
    assert_eq!(current, read(&pool, id).await);
    assert_eq!(1, signals(&pool, id).await);
    sqlx::query(
        "UPDATE listing_source_location_defaults SET revision=$1 WHERE listing_source_id=$2",
    )
    .bind(i64::MAX)
    .bind(id.into_uuid())
    .execute(&pool)
    .await
    .unwrap();
    let exhausted = read(&pool, id).await;
    let mut same = command(id, exhausted.assignment.as_ref().unwrap().location_id());
    same.expected_revision = exhausted.revision;
    assert_eq!(exhausted, assign(&pool).execute(&ctx, same).await.unwrap());
    assert!(matches!(
        clear(&pool)
            .execute(
                &ctx,
                ClearLocationDefaultCommand {
                    source_id: id,
                    expected_revision: exhausted.revision
                }
            )
            .await,
        Err(LocationDefaultError::RevisionExhausted)
    ));
    assert_eq!(1, signals(&pool, id).await);
}

async fn member(pool: &PgPool, owner: PartyId, id: ListingSourceId) -> Principal {
    let user = uuid::Uuid::now_v7();
    let partnership = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO users(user_id,email,tier,role) VALUES($1,$2,'FREE','USER')")
        .bind(user)
        .bind(format!("default-{}@example.test", user.simple()))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO partnerships(partnership_id,party_id) VALUES($1,$2)")
        .bind(partnership)
        .bind(owner.into_uuid())
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO partnership_members(partnership_id,user_id) VALUES($1,$2)")
        .bind(partnership)
        .bind(user)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO partnership_listing_source_grants(partnership_id,listing_source_id) VALUES($1,$2)").bind(partnership).bind(id.into_uuid()).execute(pool).await.unwrap();
    Principal::DelegatedUser {
        user_id: user.try_into().unwrap(),
        capabilities: BTreeSet::from([
            CredentialCapability::ListingSourcesWrite,
            CredentialCapability::ListingSourcesRead,
        ]),
    }
}
#[aura_integration_test(services=[BUSINESS_SCHEMA])]
async fn current_operator_membership_source_grant_and_scope_are_required() {
    let pool = get_postgres_client().await;
    let owner = party(&pool).await;
    let id = source(&pool, owner).await;
    let other_source = source(&pool, owner).await;
    let a = location(&pool, owner).await;
    let principal = member(&pool, owner, id).await;
    let ctx = context(principal.clone());
    for capability in [
        CredentialCapability::ProductListingsWrite,
        CredentialCapability::PartiesWrite,
        CredentialCapability::ListingSourcesRead,
    ] {
        let mut insufficient = principal.clone();
        if let Principal::DelegatedUser { capabilities, .. } = &mut insufficient {
            *capabilities = BTreeSet::from([capability]);
        }
        assert!(matches!(
            assign(&pool)
                .execute(&context(insufficient), command(id, a))
                .await,
            Err(LocationDefaultError::Forbidden)
        ));
    }
    let mut unscoped = principal.clone();
    if let Principal::DelegatedUser { capabilities, .. } = &mut unscoped {
        *capabilities = BTreeSet::from([CredentialCapability::ListingSourcesWrite]);
    }
    assert!(matches!(
        get(&pool)
            .execute(
                &context(unscoped),
                GetLocationDefaultRequest { source_id: id }
            )
            .await,
        Err(LocationDefaultError::Forbidden)
    ));
    assert!(matches!(
        assign(&pool).execute(&ctx, command(other_source, a)).await,
        Err(LocationDefaultError::Forbidden)
    ));
    assert!(matches!(
        assign(&pool)
            .execute(&context(Principal::Anonymous), command(id, a))
            .await,
        Err(LocationDefaultError::AuthenticationRequired)
    ));
    let v = assign(&pool).execute(&ctx, command(id, a)).await.unwrap();
    assert_eq!(
        v,
        get(&pool)
            .execute(&ctx, GetLocationDefaultRequest { source_id: id })
            .await
            .unwrap()
    );
    sqlx::query("DELETE FROM partnership_listing_source_grants WHERE listing_source_id=$1")
        .bind(id.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        clear(&pool)
            .execute(
                &ctx,
                ClearLocationDefaultCommand {
                    source_id: id,
                    expected_revision: v.revision
                }
            )
            .await,
        Err(LocationDefaultError::Forbidden)
    ));
    assert!(matches!(
        get(&pool)
            .execute(&ctx, GetLocationDefaultRequest { source_id: id })
            .await,
        Err(LocationDefaultError::Forbidden)
    ));
    let Principal::DelegatedUser { user_id, .. } = principal else {
        unreachable!()
    };
    sqlx::query("UPDATE users SET role='ADMIN' WHERE user_id=$1")
        .bind(user_id.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        v,
        get(&pool)
            .execute(&ctx, GetLocationDefaultRequest { source_id: id })
            .await
            .unwrap()
    );
    sqlx::query("UPDATE users SET suspended=true WHERE user_id=$1")
        .bind(user_id.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        get(&pool)
            .execute(&ctx, GetLocationDefaultRequest { source_id: id })
            .await,
        Err(LocationDefaultError::Forbidden)
    ));
    assert_eq!(1, signals(&pool, id).await);
}
async fn assert_admin_mutations_forbidden(
    pool: &PgPool,
    ctx: &OperationContext,
    source_id: ListingSourceId,
    location_id: PartyLocationId,
    expected_revision: LocationDefaultRevision,
) {
    let mut assignment = command(source_id, location_id);
    assignment.expected_revision = expected_revision;
    for result in [
        admin_assign(pool).execute(ctx, assignment).await,
        admin_change(pool)
            .execute(
                ctx,
                ChangeLocationDefaultApplicabilityCommand {
                    source_id,
                    expected_revision,
                    business_association: PatchField::Unchanged,
                    advertised_inventory: PatchField::Unchanged,
                },
            )
            .await,
        admin_clear(pool)
            .execute(
                ctx,
                ClearLocationDefaultCommand {
                    source_id,
                    expected_revision,
                },
            )
            .await,
    ] {
        assert!(matches!(result, Err(LocationDefaultError::Forbidden)));
    }
}

#[aura_integration_test(services=[BUSINESS_SCHEMA])]
async fn admin_policy_excludes_operator_members_and_preserves_default_contract() {
    let pool = get_postgres_client().await;
    let owner = party(&pool).await;
    let id = source(&pool, owner).await;
    let a = location(&pool, owner).await;
    let b = location(&pool, owner).await;
    let principal = member(&pool, owner, id).await;
    let ctx = context(principal.clone());
    let request = GetLocationDefaultRequest { source_id: id };
    assert!(matches!(
        admin_get(&pool).execute(&ctx, request).await,
        Err(LocationDefaultError::Forbidden)
    ));
    assert_admin_mutations_forbidden(&pool, &ctx, id, a, LocationDefaultRevision::INITIAL).await;
    assert_eq!(0, signals(&pool, id).await);
    // Operator authority continues to work on the source-management routes.
    let original = assign(&pool).execute(&ctx, command(id, a)).await.unwrap();
    let Principal::DelegatedUser { user_id, .. } = principal else {
        unreachable!()
    };
    sqlx::query("UPDATE users SET role='ADMIN' WHERE user_id=$1")
        .bind(user_id.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        original,
        admin_get(&pool).execute(&ctx, request).await.unwrap()
    );
    let mut replacement = command(id, b);
    replacement.expected_revision = original.revision;
    let replaced = admin_assign(&pool)
        .execute(&ctx, replacement)
        .await
        .unwrap();
    assert_eq!(b, replaced.assignment.as_ref().unwrap().location_id());
    let changed = admin_change(&pool)
        .execute(
            &ctx,
            ChangeLocationDefaultApplicabilityCommand {
                source_id: id,
                expected_revision: replaced.revision,
                business_association: PatchField::Unchanged,
                advertised_inventory: PatchField::Set(evidence(
                    SourceLocationMeaning::AdvertisedInventory,
                )),
            },
        )
        .await
        .unwrap();
    assert!(
        changed
            .assignment
            .as_ref()
            .unwrap()
            .advertised_inventory()
            .is_some()
    );
    assert!(matches!(
        admin_clear(&pool)
            .execute(
                &ctx,
                ClearLocationDefaultCommand {
                    source_id: id,
                    expected_revision: original.revision
                },
            )
            .await,
        Err(LocationDefaultError::ConcurrencyConflict)
    ));
    let cleared = admin_clear(&pool)
        .execute(
            &ctx,
            ClearLocationDefaultCommand {
                source_id: id,
                expected_revision: changed.revision,
            },
        )
        .await
        .unwrap();
    assert!(cleared.assignment.is_none());
    assert_eq!(4, signals(&pool, id).await);
    // Cognito authority uses the same current role, without delegated scope requirements.
    assert_eq!(
        cleared,
        admin_get(&pool)
            .execute(&context(Principal::User(user_id)), request)
            .await
            .unwrap()
    );
    assert_eq!(
        cleared,
        admin_get(&pool).execute(&trusted(), request).await.unwrap()
    );
    for (role, suspended) in [("ADMIN", true), ("USER", false)] {
        sqlx::query("UPDATE users SET role=$2,suspended=$3 WHERE user_id=$1")
            .bind(user_id.into_uuid())
            .bind(role)
            .bind(suspended)
            .execute(&pool)
            .await
            .unwrap();
        assert!(matches!(
            admin_get(&pool).execute(&ctx, request).await,
            Err(LocationDefaultError::Forbidden)
        ));
        assert_admin_mutations_forbidden(&pool, &ctx, id, a, cleared.revision).await;
        assert_eq!(4, signals(&pool, id).await);
    }
}

#[aura_integration_test(services=[BUSINESS_SCHEMA])]
async fn delegated_admin_defaults_require_the_exact_read_or_write_capability() {
    let pool = get_postgres_client().await;
    let owner = party(&pool).await;
    let id = source(&pool, owner).await;
    let a = location(&pool, owner).await;
    let Principal::DelegatedUser { user_id, .. } = member(&pool, owner, id).await else {
        unreachable!()
    };
    sqlx::query("UPDATE users SET role='ADMIN' WHERE user_id=$1")
        .bind(user_id.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    // Administrator authority is sufficient without a partnership or source grant.
    sqlx::query("DELETE FROM partnership_members WHERE user_id=$1")
        .bind(user_id.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    let request = GetLocationDefaultRequest { source_id: id };
    for capabilities in [
        BTreeSet::new(),
        BTreeSet::from([CredentialCapability::ProductListingsWrite]),
    ] {
        let ctx = context(Principal::DelegatedUser {
            user_id,
            capabilities,
        });
        assert!(matches!(
            admin_get(&pool).execute(&ctx, request).await,
            Err(LocationDefaultError::Forbidden)
        ));
        assert_admin_mutations_forbidden(&pool, &ctx, id, a, LocationDefaultRevision::INITIAL)
            .await;
    }
    let read_only = context(Principal::DelegatedUser {
        user_id,
        capabilities: BTreeSet::from([CredentialCapability::ListingSourcesRead]),
    });
    assert!(
        admin_get(&pool)
            .execute(&read_only, request)
            .await
            .unwrap()
            .assignment
            .is_none()
    );
    assert_admin_mutations_forbidden(&pool, &read_only, id, a, LocationDefaultRevision::INITIAL)
        .await;
    let write_only = context(Principal::DelegatedUser {
        user_id,
        capabilities: BTreeSet::from([CredentialCapability::ListingSourcesWrite]),
    });
    assert!(matches!(
        admin_get(&pool).execute(&write_only, request).await,
        Err(LocationDefaultError::Forbidden)
    ));
    admin_assign(&pool)
        .execute(&write_only, command(id, a))
        .await
        .unwrap();
    assert_eq!(1, signals(&pool, id).await);
}

async fn wait_for_lock(pool: &PgPool) {
    tokio::time::timeout(std::time::Duration::from_secs(5),async {
        loop {
            let waiting: bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND (query LIKE '%FROM parties WHERE party_id=$1 FOR UPDATE%' OR query LIKE '%FOR SHARE OF p,m,g%' OR query LIKE '%SELECT role,suspended FROM users%'))").fetch_one(pool).await.unwrap();
            if waiting { break; }
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
}
#[aura_integration_test(services=[BUSINESS_SCHEMA])]
async fn admin_execution_revalidates_role_after_a_concurrent_demotion() {
    let pool = get_postgres_client().await;
    let owner = party(&pool).await;
    let id = source(&pool, owner).await;
    let principal = member(&pool, owner, id).await;
    let Principal::DelegatedUser { user_id, .. } = &principal else {
        unreachable!()
    };
    sqlx::query("UPDATE users SET role='ADMIN' WHERE user_id=$1")
        .bind(user_id.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    let mut demotion = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    sqlx::query("UPDATE users SET role='USER' WHERE user_id=$1")
        .bind(user_id.into_uuid())
        .execute(demotion.connection())
        .await
        .unwrap();
    let request = GetLocationDefaultRequest { source_id: id };
    let query = tokio::spawn({
        let pool = pool.clone();
        let principal = principal.clone();
        async move { admin_get(&pool).execute(&context(principal), request).await }
    });
    wait_for_lock(&pool).await;
    demotion.commit().await.unwrap();
    assert!(matches!(
        query.await.unwrap(),
        Err(LocationDefaultError::Forbidden)
    ));
    // The retained operator grant must not provide an admin-route fallback.
    assert!(
        get(&pool)
            .execute(&context(principal), request)
            .await
            .is_ok()
    );
    assert_eq!(0, signals(&pool, id).await);
}
#[aura_integration_test(services=[BUSINESS_SCHEMA])]
async fn execution_revalidates_locations_and_grants_after_concurrent_changes() {
    let pool = get_postgres_client().await;
    let owner = party(&pool).await;
    let id = source(&pool, owner).await;
    let a = location(&pool, owner).await;
    let mut retirement = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    sqlx::query("SELECT party_id FROM parties WHERE party_id=$1 FOR UPDATE")
        .bind(owner.into_uuid())
        .execute(retirement.connection())
        .await
        .unwrap();
    sqlx::query("UPDATE party_locations SET lifecycle='RETIRED',revision=revision+1 WHERE party_location_id=$1").bind(a.into_uuid()).execute(retirement.connection()).await.unwrap();
    let assignment = tokio::spawn({
        let pool = pool.clone();
        async move { assign(&pool).execute(&trusted(), command(id, a)).await }
    });
    wait_for_lock(&pool).await;
    retirement.commit().await.unwrap();
    assert!(matches!(
        assignment.await.unwrap(),
        Err(LocationDefaultError::LocationRetired)
    ));
    assert!(read(&pool, id).await.assignment.is_none());
    let a = location(&pool, owner).await;
    let principal = member(&pool, owner, id).await;
    // Grant deletion does not currently lock the Party in the existing grant adapter.
    // The authoritative permission read must therefore lock the grant row itself.
    let mut revocation = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    sqlx::query("DELETE FROM partnership_listing_source_grants WHERE listing_source_id=$1")
        .bind(id.into_uuid())
        .execute(revocation.connection())
        .await
        .unwrap();
    let assignment = tokio::spawn({
        let pool = pool.clone();
        async move {
            assign(&pool)
                .execute(&context(principal), command(id, a))
                .await
        }
    });
    wait_for_lock(&pool).await;
    revocation.commit().await.unwrap();
    assert!(matches!(
        assignment.await.unwrap(),
        Err(LocationDefaultError::Forbidden)
    ));
    assert_eq!(0, signals(&pool, id).await);
}
#[aura_integration_test(services=[BUSINESS_SCHEMA])]
async fn malformed_persisted_evidence_and_geography_fail_closed() {
    let pool = get_postgres_client().await;
    let owner = party(&pool).await;
    let id = source(&pool, owner).await;
    let a = location(&pool, owner).await;
    let v = assign(&pool)
        .execute(&trusted(), command(id, a))
        .await
        .unwrap();
    sqlx::query("UPDATE listing_source_location_defaults SET business_evidence=jsonb_set(business_evidence,'{reference}','\"\"') WHERE listing_source_id=$1").bind(id.into_uuid()).execute(&pool).await.unwrap();
    assert!(matches!(
        get(&pool)
            .execute(&trusted(), GetLocationDefaultRequest { source_id: id })
            .await,
        Err(LocationDefaultError::InvalidPersistedState { .. })
    ));
    let reader = SqlxEffectiveSourceGeographyReader::new(pool.clone());
    assert!(matches!(
        reader.get(id).await,
        Err(LocationDefaultError::InvalidPersistedState { .. })
    ));
    assert!(matches!(
        clear(&pool)
            .execute(
                &trusted(),
                ClearLocationDefaultCommand {
                    source_id: id,
                    expected_revision: v.revision
                }
            )
            .await,
        Err(LocationDefaultError::InvalidPersistedState { .. })
    ));
    sqlx::query("UPDATE listing_source_location_defaults SET business_evidence=jsonb_set(business_evidence,'{reference}','\"valid\"') WHERE listing_source_id=$1").bind(id.into_uuid()).execute(&pool).await.unwrap();
    sqlx::query("UPDATE party_locations SET geography=jsonb_set(geography,'{country}','\"XX\"') WHERE party_location_id=$1").bind(a.into_uuid()).execute(&pool).await.unwrap();
    assert!(matches!(
        reader.get(id).await,
        Err(LocationDefaultError::InvalidPersistedState { .. })
    ));
}
