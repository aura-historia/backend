use application::{
    operation_context::{CredentialCapability, OperationContext, Principal},
    transaction::{Transaction, UnitOfWork},
};
use geo::{AddressText, CountryCode, GeographicDescription, PostalCode};
use geo_postgres::SqlxResolutionRepositoryFactory;
use geo_service::{geocoding::*, resolution::*};
use platform_postgres::SqlxUnitOfWork;
use sqlx::PgPool;
use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};
use user_core::{user_address::*, user_id::UserId};
use user_postgres::SqlxAddressBookFactory;
use user_service::address_book::*;
const SCHEMA: test_api::Postgres = test_api::Postgres::new("migrations");
const PROFILE: &str = "PRIVATE_USER_TEST";
type Handler =
    AddressBookHandler<SqlxUnitOfWork, SqlxAddressBookFactory, SqlxResolutionRepositoryFactory>;
fn context(id: UserId) -> OperationContext {
    OperationContext {
        principal: Principal::User(id),
        request_id: "address-test".into(),
        correlation_id: "address-test".into(),
    }
}
fn geography(text: &str) -> GeographicDescription {
    GeographicDescription::new(Some(AddressText::new(text).unwrap()), None, None, None)
        .unwrap()
        .unwrap()
}
fn create(id: UserAddressId, text: &str) -> CreateUserAddress {
    CreateUserAddress {
        id,
        label: UserAddressLabel::new("Home".into()).unwrap(),
        geography: geography(text),
    }
}
async fn fixture() -> (PgPool, Handler, OperationContext, OperationContext) {
    let pool = get_postgres_client().await;
    // Isolated schema only; no live credentials or provider calls.
    sqlx::query("UPDATE geo_resolution_provider_controls SET is_default=false")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO geo_resolution_provider_controls(profile,is_default) VALUES($1,true)")
        .bind(PROFILE)
        .execute(&pool)
        .await
        .unwrap();
    let a = UserId::new();
    let b = UserId::new();
    for id in [a, b] {
        sqlx::query("INSERT INTO users(user_id,email,tier,role) VALUES($1,$2,'FREE','USER')")
            .bind(id.into_uuid())
            .bind(format!("{id}@example.test"))
            .execute(&pool)
            .await
            .unwrap();
    }
    let handler = AddressBookHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxAddressBookFactory,
        SqlxResolutionRepositoryFactory,
    );
    (pool, handler, context(a), context(b))
}
#[aura_integration_test(services=[SCHEMA])]
async fn own_crud_pagination_idempotency_and_private_cache_isolation() {
    let (pool, h, a, b) = fixture().await;
    let first = UserAddressId::new();
    let second = UserAddressId::new();
    let one = h.create(&a, create(first, "Synthetic One")).await.unwrap();
    assert_eq!(1, one.version);
    assert!(!one.preferred);
    h.create(&a, create(first, "Synthetic One")).await.unwrap();
    assert!(matches!(
        h.create(&a, create(first, "Different")).await,
        Err(AddressBookError::Conflict)
    ));
    h.create(&a, create(second, "Synthetic Two")).await.unwrap();
    let page = h.list(&a, None, 1).await.unwrap();
    assert_eq!(1, page.items.len());
    assert_eq!(Some(first), page.next);
    let page = h.list(&a, page.next, 1).await.unwrap();
    assert_eq!(second, page.items[0].id);
    assert!(page.next.is_none());
    assert!(matches!(
        h.get(&b, first).await,
        Err(AddressBookError::NotFound)
    ));
    assert!(matches!(
        h.update(
            &b,
            UpdateUserAddress {
                id: first,
                expected_version: 1,
                label: one.label.clone(),
                geography: geography("Other")
            }
        )
        .await,
        Err(AddressBookError::NotFound)
    ));
    assert!(matches!(
        h.delete(&b, first, 1).await,
        Err(AddressBookError::NotFound)
    ));
    assert!(matches!(
        h.set_destination(&b, BrowsingDestination::Address(first), 1)
            .await,
        Err(AddressBookError::NotFound)
    ));
    assert!(matches!(
        h.resolve_destination(&b, Some(BrowsingDestination::Address(first)))
            .await,
        Err(AddressBookError::NotFound)
    ));
    let foreign = UserAddressId::new();
    h.create(&b, create(foreign, "Synthetic One"))
        .await
        .unwrap();
    let ra = h.inspect(&a, first).await.unwrap().unwrap();
    let rb = h.inspect(&b, foreign).await.unwrap().unwrap();
    assert_ne!(ra.work.id, rb.work.id);
    let changed = h
        .update(
            &a,
            UpdateUserAddress {
                id: first,
                expected_version: 1,
                label: one.label.clone(),
                geography: geography("Replacement"),
            },
        )
        .await
        .unwrap();
    assert_eq!(first, changed.id);
    assert_eq!(2, changed.input_revision);
    assert_eq!(2, changed.version);
    let old: (bool, Option<serde_json::Value>) =
        sqlx::query_as("SELECT erased,input FROM geo_resolutions WHERE resolution_id=$1")
            .bind(ra.work.id.into_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((true, None), old);
    assert!(matches!(
        h.update(
            &a,
            UpdateUserAddress {
                id: first,
                expected_version: 1,
                label: one.label,
                geography: geography("Stale")
            }
        )
        .await,
        Err(AddressBookError::Conflict)
    ));
    h.delete(&a, first, 2).await.unwrap();
    assert!(matches!(
        h.get(&a, first).await,
        Err(AddressBookError::NotFound)
    ));
    assert!(matches!(
        h.create(&a, create(first, "Replacement")).await,
        Err(AddressBookError::Conflict)
    ));
    let tombstone: (Option<String>, Option<serde_json::Value>) =
        sqlx::query_as("SELECT label,geography FROM user_addresses WHERE address_id=$1")
            .bind(first.into_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((None, None), tombstone);
}
#[aura_integration_test(services=[SCHEMA])]
async fn independent_sessions_fence_updates_preferences_and_create_retries() {
    let (_pool, h, a, _) = fixture().await;
    let id = UserAddressId::new();
    let other = UserAddressId::new();
    let (l, r) = tokio::join!(
        h.create(&a, create(id, "Synthetic Race")),
        h.create(&a, create(id, "Synthetic Race"))
    );
    assert!(l.is_ok() && r.is_ok());
    assert_eq!(1, h.list(&a, None, 100).await.unwrap().items.len());
    h.create(&a, create(other, "Other")).await.unwrap();
    let (l, r) = tokio::join!(
        h.update(
            &a,
            UpdateUserAddress {
                id,
                expected_version: 1,
                label: UserAddressLabel::new("One".into()).unwrap(),
                geography: geography("One")
            }
        ),
        h.update(
            &a,
            UpdateUserAddress {
                id,
                expected_version: 1,
                label: UserAddressLabel::new("Two".into()).unwrap(),
                geography: geography("Two")
            }
        )
    );
    assert_ne!(l.is_ok(), r.is_ok());
    assert!(
        matches!(l, Err(AddressBookError::Conflict))
            || matches!(r, Err(AddressBookError::Conflict))
    );
    let (l, r) = tokio::join!(
        h.set_destination(&a, BrowsingDestination::Address(id), 1),
        h.set_destination(&a, BrowsingDestination::Address(other), 1)
    );
    assert_ne!(l.is_ok(), r.is_ok());
    let entries = h.list(&a, None, 100).await.unwrap();
    assert_eq!(1, entries.items.iter().filter(|v| v.preferred).count());
    let preferred = entries.items.iter().find(|v| v.preferred).unwrap();
    h.delete(&a, preferred.id, preferred.version).await.unwrap();
    assert!(matches!(
        h.destination(&a).await.unwrap().destination,
        BrowsingDestination::None
    ));
    assert_eq!(
        0,
        h.list(&a, None, 100)
            .await
            .unwrap()
            .items
            .iter()
            .filter(|v| v.preferred)
            .count()
    );
    let raced = UserAddressId::new();
    h.create(&a, create(raced, "Synthetic preference deletion race"))
        .await
        .unwrap();
    let version = h.destination(&a).await.unwrap().version;
    let (selection, deletion) = tokio::join!(
        h.set_destination(&a, BrowsingDestination::Address(raced), version),
        h.delete(&a, raced, 1),
    );
    deletion.unwrap();
    assert!(selection.is_ok() || matches!(selection, Err(AddressBookError::NotFound)));
    assert!(matches!(
        h.destination(&a).await.unwrap().destination,
        BrowsingDestination::None
    ));
}
#[aura_integration_test(services=[SCHEMA])]
async fn explicit_destinations_and_delegated_scope_are_authorized() {
    let (pool, h, a, b) = fixture().await;
    let id = UserAddressId::new();
    h.create(&a, create(id, "Synthetic Destination"))
        .await
        .unwrap();
    h.set_destination(&a, BrowsingDestination::Address(id), 1)
        .await
        .unwrap();
    let explicit = BrowsingDestination::CountryPostal {
        country: CountryCode::GBR,
        postal_code: Some(PostalCode::new("SW1A 1AA").unwrap()),
    };
    let snapshot = h
        .resolve_destination(&a, Some(explicit.clone()))
        .await
        .unwrap();
    assert_eq!(explicit, snapshot.destination);
    assert!(snapshot.derived.is_none());
    assert!(snapshot.submitted.unwrap().address_text().is_none());
    assert!(matches!(
        h.resolve_destination(&a, Some(BrowsingDestination::None))
            .await
            .unwrap()
            .destination,
        BrowsingDestination::None
    ));
    h.set_destination(&a, explicit.clone(), 2).await.unwrap();
    assert_eq!(
        explicit,
        h.resolve_destination(&a, None).await.unwrap().destination
    );
    let owner = match a.principal {
        Principal::User(id) => id,
        _ => unreachable!(),
    };
    let mut delegated = a.clone();
    delegated.principal = Principal::DelegatedUser {
        user_id: owner,
        capabilities: [CredentialCapability::UsersRead].into(),
    };
    assert!(h.get(&delegated, id).await.is_ok());
    assert!(matches!(
        h.delete(&delegated, id, 1).await,
        Err(AddressBookError::Forbidden)
    ));
    delegated.principal = Principal::DelegatedUser {
        user_id: owner,
        capabilities: [CredentialCapability::UsersWrite].into(),
    };
    assert!(matches!(
        h.get(&delegated, id).await,
        Err(AddressBookError::Forbidden)
    ));
    let foreign = match b.principal {
        Principal::User(id) => id,
        _ => unreachable!(),
    };
    delegated.principal = Principal::DelegatedUser {
        user_id: foreign,
        capabilities: [
            CredentialCapability::UsersRead,
            CredentialCapability::UsersWrite,
        ]
        .into(),
    };
    assert!(matches!(
        h.get(&delegated, id).await,
        Err(AddressBookError::NotFound)
    ));
    sqlx::query("UPDATE users SET suspended=true WHERE user_id=$1")
        .bind(owner.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        h.get(&a, id).await,
        Err(AddressBookError::Forbidden)
    ));
}
#[aura_integration_test(services=[SCHEMA])]
async fn failures_leave_bookmarks_and_erasure_fences_live_claims() {
    let (pool, h, a, b) = fixture().await;
    sqlx::query("UPDATE geo_resolution_provider_controls SET enabled=true,storage_permitted=true")
        .execute(&pool)
        .await
        .unwrap();
    let id = UserAddressId::new();
    h.create(&a, create(id, "Synthetic Claim")).await.unwrap();
    let view = h.inspect(&a, id).await.unwrap().unwrap();
    let uow = SqlxUnitOfWork::new(pool.clone());
    let mut tx = uow.begin().await.unwrap();
    let ClaimOutcome::Claimed(claim) = SqlxResolutionRepositoryFactory
        .resolutions(&mut tx)
        .claim(view.work, PROFILE, true)
        .await
        .unwrap()
    else {
        panic!("claim required")
    };
    tx.commit().await.unwrap();
    let mut tx = uow.begin().await.unwrap();
    SqlxResolutionRepositoryFactory
        .resolutions(&mut tx)
        .complete(
            &claim,
            ResolutionCompletion::Failure(GeocodingFailure::Authentication),
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(h.get(&a, id).await.is_ok());
    let current = h.inspect(&a, id).await.unwrap().unwrap();
    assert_eq!(ResolutionStatus::Blocked, current.status);
    assert!(matches!(
        h.retry(
            &b,
            ReviewUserAddress {
                id,
                expected_version: 1,
                work: current.work
            },
            false
        )
        .await,
        Err(AddressBookError::NotFound)
    ));
    assert!(matches!(
        h.select(
            &b,
            ReviewUserAddress {
                id,
                expected_version: 1,
                work: current.work
            },
            0
        )
        .await,
        Err(AddressBookError::NotFound)
    ));
    h.retry(
        &a,
        ReviewUserAddress {
            id,
            expected_version: 1,
            work: current.work,
        },
        true,
    )
    .await
    .unwrap();
    let current = h.inspect(&a, id).await.unwrap().unwrap();
    let mut tx = uow.begin().await.unwrap();
    let ClaimOutcome::Claimed(claim) = SqlxResolutionRepositoryFactory
        .resolutions(&mut tx)
        .claim(current.work, PROFILE, true)
        .await
        .unwrap()
    else {
        panic!("claim required")
    };
    tx.commit().await.unwrap();
    h.update(
        &a,
        UpdateUserAddress {
            id,
            expected_version: 1,
            label: UserAddressLabel::new("Home".into()).unwrap(),
            geography: geography("Edited"),
        },
    )
    .await
    .unwrap();
    let mut tx = uow.begin().await.unwrap();
    assert!(
        SqlxResolutionRepositoryFactory
            .resolutions(&mut tx)
            .complete(
                &claim,
                ResolutionCompletion::Evidence(
                    evaluate(
                        &claim.input,
                        GeocodingResult {
                            outcome: GeocodingOutcome::NoMatch,
                            retention: ResultRetention::Storage,
                        },
                        PROFILE,
                    )
                    .unwrap()
                )
            )
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    assert!(h.inspect(&a, id).await.unwrap().unwrap().evidence.is_none());
    let current = h.inspect(&a, id).await.unwrap().unwrap();
    let mut tx = uow.begin().await.unwrap();
    let ClaimOutcome::Claimed(claim) = SqlxResolutionRepositoryFactory
        .resolutions(&mut tx)
        .claim(current.work, PROFILE, true)
        .await
        .unwrap()
    else {
        panic!("claim required")
    };
    tx.commit().await.unwrap();
    h.set_destination(&a, BrowsingDestination::Address(id), 1)
        .await
        .unwrap();
    let owner = match a.principal {
        Principal::User(id) => id,
        _ => unreachable!(),
    };
    use user_service::use_cases::commands::delete_user::{
        DeleteUserCommand, DeleteUserHandler, DeleteUserUseCase,
    };
    DeleteUserHandler::new(
        uow.clone(),
        user_postgres::SqlxMarketingConsentIntentRepository::new(),
        user_postgres::SqlxUserAdminReaderFactory::new(),
    )
    .execute(&a, DeleteUserCommand { user_id: owner })
    .await
    .unwrap();
    assert_eq!(
        0,
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM user_addresses WHERE user_id=$1")
            .bind(owner.into_uuid())
            .fetch_one(&pool)
            .await
            .unwrap()
    );
    assert_eq!(
        0,
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM user_browsing_destinations WHERE user_id=$1"
        )
        .bind(owner.into_uuid())
        .fetch_one(&pool)
        .await
        .unwrap()
    );
    let mut tx = uow.begin().await.unwrap();
    assert!(
        SqlxResolutionRepositoryFactory
            .resolutions(&mut tx)
            .complete(
                &claim,
                ResolutionCompletion::Evidence(
                    evaluate(
                        &claim.input,
                        GeocodingResult {
                            outcome: GeocodingOutcome::NoMatch,
                            retention: ResultRetention::Storage,
                        },
                        PROFILE,
                    )
                    .unwrap()
                )
            )
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    let state: (bool, Option<serde_json::Value>, Option<serde_json::Value>) =
        sqlx::query_as("SELECT erased,input,evidence FROM geo_resolutions WHERE resolution_id=$1")
            .bind(claim.work.id.into_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((true, None, None), state);
}

#[aura_integration_test(services=[SCHEMA])]
async fn private_shared_input_is_retained_only_until_its_last_bookmark_is_removed() {
    let (pool, h, a, _) = fixture().await;
    let first = UserAddressId::new();
    let second = UserAddressId::new();
    h.create(&a, create(first, "Shared synthetic address"))
        .await
        .unwrap();
    h.create(&a, create(second, "Shared synthetic address"))
        .await
        .unwrap();
    let resolution = h.inspect(&a, first).await.unwrap().unwrap().work;
    assert_eq!(
        resolution,
        h.inspect(&a, second).await.unwrap().unwrap().work
    );
    h.delete(&a, first, 1).await.unwrap();
    let state: (bool, Option<serde_json::Value>) =
        sqlx::query_as("SELECT erased,input FROM geo_resolutions WHERE resolution_id=$1")
            .bind(resolution.id.into_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!state.0);
    assert!(state.1.is_some());
    assert_eq!(
        resolution,
        h.inspect(&a, second).await.unwrap().unwrap().work
    );
    h.update(
        &a,
        UpdateUserAddress {
            id: second,
            expected_version: 1,
            label: UserAddressLabel::new("Home".into()).unwrap(),
            geography: geography("Replacement synthetic address"),
        },
    )
    .await
    .unwrap();
    let state: (bool, Option<serde_json::Value>, Option<serde_json::Value>) =
        sqlx::query_as("SELECT erased,input,evidence FROM geo_resolutions WHERE resolution_id=$1")
            .bind(resolution.id.into_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((true, None, None), state);
    assert_eq!(
        0,
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM geo_resolution_bindings WHERE resolution_id=$1"
        )
        .bind(resolution.id.into_uuid())
        .fetch_one(&pool)
        .await
        .unwrap()
    );
    assert_eq!(
        0,
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM geo_resolution_wakeups WHERE resolution_id=$1"
        )
        .bind(resolution.id.into_uuid())
        .fetch_one(&pool)
        .await
        .unwrap()
    );
    // Returning to removed text starts fresh work instead of reviving an erased cache.
    let third = UserAddressId::new();
    h.create(&a, create(third, "Shared synthetic address"))
        .await
        .unwrap();
    assert_ne!(
        resolution.id,
        h.inspect(&a, third).await.unwrap().unwrap().work.id
    );
}

#[aura_integration_test(services=[SCHEMA])]
async fn saved_filters_survive_bookmark_changes_and_are_erased_with_the_account() {
    use user_service::use_cases::commands::delete_user::{
        DeleteUserCommand, DeleteUserHandler, DeleteUserUseCase,
    };
    let (pool, h, a, _) = fixture().await;
    let Principal::User(owner) = a.principal else {
        unreachable!();
    };
    let address = UserAddressId::new();
    h.create(&a, create(address, "Synthetic original address"))
        .await
        .unwrap();
    let filter = uuid::Uuid::now_v7();
    // The current filter contract has no destination field or address reference.
    sqlx::query("INSERT INTO search_filters(user_search_filter_id,user_id,name,state,search,language,currency) VALUES($1,$2,'Saved private filter','ACTIVE','{}','en','GBP')")
        .bind(filter).bind(owner.into_uuid()).execute(&pool).await.unwrap();
    h.update(
        &a,
        UpdateUserAddress {
            id: address,
            expected_version: 1,
            label: UserAddressLabel::new("Home".into()).unwrap(),
            geography: geography("Synthetic replacement address"),
        },
    )
    .await
    .unwrap();
    h.delete(&a, address, 2).await.unwrap();
    let retained: (String, serde_json::Value, i64) = sqlx::query_as(
        "SELECT name,search,version FROM search_filters WHERE user_search_filter_id=$1",
    )
    .bind(filter)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        ("Saved private filter".into(), serde_json::json!({}), 1),
        retained
    );
    DeleteUserHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        user_postgres::SqlxMarketingConsentIntentRepository::new(),
        user_postgres::SqlxUserAdminReaderFactory::new(),
    )
    .execute(&a, DeleteUserCommand { user_id: owner })
    .await
    .unwrap();
    assert_eq!(
        0,
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM search_filters WHERE user_id=$1")
            .bind(owner.into_uuid())
            .fetch_one(&pool)
            .await
            .unwrap()
    );
}

#[aura_integration_test(services=[SCHEMA])]
async fn candidate_selection_expiry_snapshots_and_unrelated_preferences() {
    let (pool, h, a, b) = fixture().await;
    sqlx::query("UPDATE geo_resolution_provider_controls SET enabled=true,storage_permitted=true")
        .execute(&pool)
        .await
        .unwrap();
    let id = UserAddressId::new();
    let other = UserAddressId::new();
    h.create(&a, create(id, "Synthetic Candidate"))
        .await
        .unwrap();
    h.create(&a, create(other, "Synthetic Candidate"))
        .await
        .unwrap();
    let current = h.inspect(&a, id).await.unwrap().unwrap();
    let uow = SqlxUnitOfWork::new(pool.clone());
    let mut tx = uow.begin().await.unwrap();
    let ClaimOutcome::Claimed(claim) = SqlxResolutionRepositoryFactory
        .resolutions(&mut tx)
        .claim(current.work, PROFILE, true)
        .await
        .unwrap()
    else {
        panic!("claim required")
    };
    tx.commit().await.unwrap();
    let candidate = GeocodingCandidate::new(
        geo::DerivedGeography::new(
            GeographicDescription::new(None, Some(CountryCode::GBR), None, None).unwrap(),
            Some(
                geo::SpatialPosition::new(
                    geo::GeoPoint::new(51.5, -0.1).unwrap(),
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
            external_reference: "synthetic-place".into(),
            attribution_url: "https://example.test/attribution",
        },
    );
    let evidence = evaluate(
        &claim.input,
        GeocodingResult {
            outcome: GeocodingOutcome::Candidates(CandidateSet::new(vec![candidate], 5).unwrap()),
            retention: ResultRetention::Storage,
        },
        PROFILE,
    )
    .unwrap();
    let mut tx = uow.begin().await.unwrap();
    assert!(
        SqlxResolutionRepositoryFactory
            .resolutions(&mut tx)
            .complete(&claim, ResolutionCompletion::Evidence(evidence))
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    let current = h.inspect(&a, id).await.unwrap().unwrap();
    assert_eq!(ResolutionStatus::Accepted, current.status);
    assert!(current.accepted_geography().unwrap().position().is_none());
    assert!(matches!(
        h.select(
            &b,
            ReviewUserAddress {
                id,
                expected_version: 1,
                work: current.work
            },
            0
        )
        .await,
        Err(AddressBookError::NotFound)
    ));
    h.select(
        &a,
        ReviewUserAddress {
            id,
            expected_version: 1,
            work: current.work,
        },
        0,
    )
    .await
    .unwrap();
    h.set_destination(&a, BrowsingDestination::Address(id), 1)
        .await
        .unwrap();
    let snapshot = h.resolve_destination(&a, None).await.unwrap();
    assert!(snapshot.derived.as_ref().unwrap().position().is_some());
    let other_resolution = h.inspect(&a, other).await.unwrap().unwrap();
    assert!(
        other_resolution
            .accepted_geography()
            .unwrap()
            .position()
            .is_none()
    );
    h.update(
        &a,
        UpdateUserAddress {
            id,
            expected_version: 1,
            label: UserAddressLabel::new("Renamed".into()).unwrap(),
            geography: geography("Synthetic Candidate"),
        },
    )
    .await
    .unwrap();
    let unchanged_input = h.inspect(&a, id).await.unwrap().unwrap();
    assert_eq!(1, unchanged_input.input_revision);
    assert_eq!(current.work, unchanged_input.work);
    assert_eq!(Some(0), unchanged_input.selected_candidate);
    sqlx::query(
        "UPDATE geo_resolutions SET valid_until=now()-interval '1 second' WHERE resolution_id=$1",
    )
    .bind(current.work.id.into_uuid())
    .execute(&pool)
    .await
    .unwrap();
    assert!(
        h.resolve_destination(&a, None)
            .await
            .unwrap()
            .derived
            .is_none()
    );
    assert!(h.inspect(&a, id).await.unwrap().unwrap().evidence.is_none());
    h.delete(&a, id, 2).await.unwrap();
    assert!(snapshot.derived.unwrap().position().is_some());
    assert_eq!(
        "Synthetic Candidate",
        snapshot.submitted.unwrap().address_text().unwrap().as_str()
    );
    let version = h.destination(&a).await.unwrap().version;
    let explicit = BrowsingDestination::CountryPostal {
        country: CountryCode::GBR,
        postal_code: None,
    };
    h.set_destination(&a, explicit.clone(), version)
        .await
        .unwrap();
    h.delete(&a, other, 1).await.unwrap();
    assert_eq!(explicit, h.destination(&a).await.unwrap().destination);
}

#[aura_integration_test(services=[SCHEMA])]
async fn erasure_remains_possible_after_counter_exhaustion() {
    let (pool, h, a, _) = fixture().await;
    let id = UserAddressId::new();
    h.create(&a, create(id, "Synthetic exhausted counters"))
        .await
        .unwrap();
    h.set_destination(&a, BrowsingDestination::Address(id), 1)
        .await
        .unwrap();
    let view = h.inspect(&a, id).await.unwrap().unwrap();
    sqlx::query("UPDATE user_addresses SET version=9223372036854775807 WHERE address_id=$1")
        .bind(id.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE user_browsing_destinations SET version=9223372036854775807")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE geo_resolutions SET generation=9223372036854775807 WHERE resolution_id=$1")
        .bind(view.work.id.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    h.delete(&a, id, i64::MAX as u64).await.unwrap();
    assert!(matches!(
        h.destination(&a).await.unwrap().destination,
        BrowsingDestination::None
    ));
    let erased: bool =
        sqlx::query_scalar("SELECT erased FROM geo_resolutions WHERE resolution_id=$1")
            .bind(view.work.id.into_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(erased);
}
