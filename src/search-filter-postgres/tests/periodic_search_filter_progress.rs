use application::transaction::{Transaction, UnitOfWork};
use localization::Language;
use money::Currency;
use platform_postgres::SqlxUnitOfWork;
use product_listing_core::product_listing_search::ProductListingSearch;
use search_filter_core::{
    NewSearchFilter, SearchFilter, search_filter_state::SearchFilterState,
    user_search_filter_id::UserSearchFilterId, user_search_filter_name::UserSearchFilterName,
};
use search_filter_postgres::{
    SqlxPeriodicSearchFilterProgressFactory, SqlxSearchFilterRepositoryFactory,
};
use search_filter_service::ports::{
    PeriodicSearchFilterProgress, PeriodicSearchFilterProgressFactory,
    PeriodicSearchFilterProgressLockOutcome, PeriodicSearchFilterProgressWriteOutcome,
    SearchFilterRepository, SearchFilterRepositoryFactory,
};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::{Duration, OffsetDateTime};
use user_core::user_id::UserId;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn logical_run_freezes_filters_resumes_after_crash_and_does_not_repeat_completed_users() {
    use search_filter_postgres::SqlxPeriodicSearchFilterCandidateReader;
    use search_filter_service::ports::{
        PeriodicSearchFilterCandidatePageRequest, PeriodicSearchFilterCandidateReader,
    };
    let pool = get_postgres_client().await;
    let unit = SqlxUnitOfWork::new(pool.clone());
    let filter = seed_filter(&unit, &pool, "run-resume@example.test").await;
    let end = OffsetDateTime::from_unix_timestamp_nanos(
        (OffsetDateTime::now_utc() + Duration::seconds(1)).unix_timestamp_nanos() / 1000 * 1000,
    )
    .unwrap();
    let progress = SqlxPeriodicSearchFilterProgressFactory;
    let mut tx = begin(&unit).await;
    assert_eq!(
        end,
        progress
            .in_transaction(&mut tx)
            .begin_run(end)
            .await
            .unwrap()
    );
    drop(tx); // A crash before registering the run commits no partial snapshot.
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM search_filter_matching_runs")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(0, count);
    let mut tx = begin(&unit).await;
    progress
        .in_transaction(&mut tx)
        .begin_run(end)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let selected = (0..50)
        .map(
            |_| product_listing_service::ports::ProductListingSearchFilterMatchSourceRef {
                product_listing_id: product_listing_core::product_listing_id::ProductListingId::new(
                ),
                event_id: domain_primitives::event_id::EventId::new(),
            },
        )
        .collect::<Vec<_>>();
    let mut tx = begin(&unit).await;
    assert_eq!(
        selected,
        progress
            .in_transaction(&mut tx)
            .freeze_selection(end, filter.id(), &selected)
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    let mut tx = begin(&unit).await;
    assert_eq!(
        Some(selected.clone()),
        progress
            .in_transaction(&mut tx)
            .load_selection(end, filter.id())
            .await
            .unwrap()
    );
    assert_eq!(
        selected,
        progress
            .in_transaction(&mut tx)
            .freeze_selection(end, filter.id(), &[])
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    sqlx::query("UPDATE search_filters SET name = 'Changed after snapshot', version = version + 1 WHERE user_search_filter_id = $1").bind(filter.id().into_uuid()).execute(&pool).await.unwrap();
    let mut tx = begin(&unit).await;
    assert_eq!(
        end,
        progress
            .in_transaction(&mut tx)
            .begin_run(end + Duration::hours(1))
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    let candidates = SqlxPeriodicSearchFilterCandidateReader::new(pool.clone())
        .find_active_page(PeriodicSearchFilterCandidatePageRequest {
            after: None,
            page_size: 100,
            eligible_at_or_before: end,
        })
        .await
        .unwrap();
    assert_eq!(1, candidates.len());
    assert_eq!(filter.id(), candidates[0].search_filter_id);
    assert_eq!(1, candidates[0].version);
    assert_ne!("Changed after snapshot", candidates[0].name.as_ref());
    let mut tx = begin(&unit).await;
    progress
        .in_transaction(&mut tx)
        .record_filter_attempt(end, filter.id(), true)
        .await
        .unwrap();
    progress
        .in_transaction(&mut tx)
        .finish_run(end)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let candidates = SqlxPeriodicSearchFilterCandidateReader::new(pool.clone())
        .find_active_page(PeriodicSearchFilterCandidatePageRequest {
            after: None,
            page_size: 100,
            eligible_at_or_before: end,
        })
        .await
        .unwrap();
    assert!(candidates.is_empty());
    let next = end + Duration::hours(1);
    let mut tx = begin(&unit).await;
    assert_eq!(
        next,
        progress
            .in_transaction(&mut tx)
            .begin_run(next)
            .await
            .unwrap()
    );
    assert!(
        progress
            .in_transaction(&mut tx)
            .freeze_selection(next, filter.id(), &[])
            .await
            .unwrap()
            .is_empty()
    );
    tx.commit().await.unwrap();
    let mut tx = begin(&unit).await;
    assert_eq!(
        Some(vec![]),
        progress
            .in_transaction(&mut tx)
            .load_selection(next, filter.id())
            .await
            .unwrap()
    );
    assert!(
        progress
            .in_transaction(&mut tx)
            .freeze_selection(next, filter.id(), &selected)
            .await
            .unwrap()
            .is_empty()
    );
    tx.commit().await.unwrap();
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_resolve_missing_progress_to_creation_and_only_advance_forward() {
    let pool = get_postgres_client().await;
    let unit = SqlxUnitOfWork::new(pool.clone());
    let filter = seed_filter(&unit, &pool, "periodic-progress-forward@example.com").await;
    let created = filter_created(&pool, filter.id()).await;
    let first_target = created + Duration::hours(1);
    let later_target = first_target + Duration::hours(1);
    let progress = SqlxPeriodicSearchFilterProgressFactory;

    let mut tx = begin(&unit).await;
    assert_eq!(
        PeriodicSearchFilterProgressLockOutcome::Current {
            matched_through: created
        },
        progress
            .in_transaction(&mut tx)
            .lock_and_read(filter.id(), 1, created, first_target)
            .await
            .unwrap_or_else(|error| panic!("lock and read failed: {error:?}"))
    );
    assert_eq!(
        PeriodicSearchFilterProgressWriteOutcome::Advanced,
        progress
            .in_transaction(&mut tx)
            .compare_and_set(filter.id(), created, first_target)
            .await
            .unwrap_or_else(|error| panic!("first checkpoint failed: {error:?}"))
    );
    tx.commit()
        .await
        .unwrap_or_else(|error| panic!("first commit failed: {error:?}"));

    let mut tx = begin(&unit).await;
    assert_eq!(
        PeriodicSearchFilterProgressWriteOutcome::AlreadyCovered,
        progress
            .in_transaction(&mut tx)
            .compare_and_set(filter.id(), first_target, first_target)
            .await
            .unwrap_or_else(|error| panic!("equal checkpoint failed: {error:?}"))
    );
    assert_eq!(
        PeriodicSearchFilterProgressWriteOutcome::Superseded,
        progress
            .in_transaction(&mut tx)
            .compare_and_set(filter.id(), created, later_target)
            .await
            .unwrap_or_else(|error| panic!("stale checkpoint failed: {error:?}"))
    );
    tx.commit()
        .await
        .unwrap_or_else(|error| panic!("second commit failed: {error:?}"));

    let saved = checkpoint(&pool, filter.id()).await;
    assert_eq!(Some(first_target), saved);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_rollback_checkpoint_when_final_transaction_drops() {
    let pool = get_postgres_client().await;
    let unit = SqlxUnitOfWork::new(pool.clone());
    let filter = seed_filter(&unit, &pool, "periodic-progress-rollback@example.com").await;
    let created = filter_created(&pool, filter.id()).await;
    let target = created + Duration::hours(1);
    let progress = SqlxPeriodicSearchFilterProgressFactory;

    {
        let mut tx = begin(&unit).await;
        assert_eq!(
            PeriodicSearchFilterProgressWriteOutcome::Advanced,
            progress
                .in_transaction(&mut tx)
                .compare_and_set(filter.id(), created, target)
                .await
                .unwrap_or_else(|error| panic!("checkpoint failed: {error:?}"))
        );
    }

    assert_eq!(None, checkpoint(&pool, filter.id()).await);
}

async fn seed_filter(unit: &SqlxUnitOfWork, pool: &sqlx::PgPool, email: &str) -> SearchFilter {
    let user_id = seed_user(pool, email).await;
    let filter = SearchFilter::create(NewSearchFilter {
        user_search_filter_id: UserSearchFilterId::new(),
        user_id,
        name: UserSearchFilterName::from("periodic progress"),
        notifications: true,
        state: SearchFilterState::Active,
        search: ProductListingSearch::new(Language::En, Currency::Eur),
        embedding: None,
        matching_evidence_policy:
            search_filter_core::MatchingEvidencePolicy::EnhancedTextAndFourImages,
    });
    let repository = SqlxSearchFilterRepositoryFactory;
    let mut tx = begin(unit).await;
    repository
        .in_transaction(&mut tx)
        .insert(&filter)
        .await
        .unwrap_or_else(|error| panic!("insert filter failed: {error:?}"));
    tx.commit()
        .await
        .unwrap_or_else(|error| panic!("commit failed: {error:?}"));
    filter
}

async fn begin(unit: &SqlxUnitOfWork) -> platform_postgres::SqlxTransaction {
    unit.begin()
        .await
        .unwrap_or_else(|error| panic!("begin failed: {error:?}"))
}

async fn filter_created(pool: &sqlx::PgPool, filter_id: UserSearchFilterId) -> OffsetDateTime {
    sqlx::query_scalar("SELECT created FROM search_filters WHERE user_search_filter_id = $1")
        .bind(filter_id.into_uuid())
        .fetch_one(pool)
        .await
        .unwrap_or_else(|error| panic!("read filter creation failed: {error:?}"))
}

async fn checkpoint(pool: &sqlx::PgPool, filter_id: UserSearchFilterId) -> Option<OffsetDateTime> {
    sqlx::query_scalar(
        "SELECT matched_through FROM search_filter_periodic_match_state WHERE user_search_filter_id = $1",
    )
    .bind(filter_id.into_uuid())
    .fetch_optional(pool)
    .await
    .unwrap_or_else(|error| panic!("read checkpoint failed: {error:?}"))
}

async fn seed_user(pool: &sqlx::PgPool, email: &str) -> UserId {
    let id = UserId::new();
    sqlx::query(
        "INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'ULTIMATE', 'USER')",
    )
    .bind(id.into_uuid())
    .bind(email)
    .execute(pool)
    .await
    .unwrap_or_else(|error| panic!("seed user failed: {error:?}"));
    id
}
