use aura_historia_jobs::{
    DomainJob, DomainJobPayload, IdempotencyKey, OrderingKey, SearchFilterMatchCreatedJob,
    WorkerQueue, encode,
};
use aws_lambda_events::sqs::{SqsBatchResponse, SqsEvent, SqsMessage};
use domain_primitives::event_id::EventId;
use lambda_runtime::{Context, LambdaEvent};
use listing_source_core::ListingSourceId;
use product_listing_core::product_listing_id::ProductListingId;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use search_filter_match_notification_lambda::{
    compose_search_filter_match_notification_use_case, handler,
};
use serde_json::{Value, json};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use user_core::user_id::UserId;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn digest_waits_for_filters_and_selection_recovers_persisted_matches_and_finalizes_once() {
    let result: TestResult = async {
        use notification_service::ports::notification_delivery_repository::{NotificationDeliveryRepository, ClaimNotificationDeliveryOutcome};
        let pool = get_postgres_client().await;
        let user = seed_user(&pool).await?;
        let first = seed_filter(&pool, user).await?;
        let second = seed_filter(&pool, user).await?;
        let (listing, origin) = seed_product(&pool).await?;
        let end = seed_digest_run(&pool, user, &[first, second]).await?;
        let use_case = compose_search_filter_match_notification_use_case(pool.clone());
        seed_run_match(&pool, user, first, listing, origin, end).await?;
        assert!(failure_ids(handler(event("first", job_body(user, first, listing, origin)?), use_case.as_ref()).await?).is_empty());
        terminal_filter(&pool, end, first).await?;
        let maintenance = digest_maintenance(&pool);
        assert_eq!(0, maintenance.execute(10).await?.digests_created);

        // Crash after the second match commits, before its CDC notification job.
        seed_run_match(&pool, user, second, listing, origin, end).await?;
        terminal_filter(&pool, end, second).await?;
        assert_eq!(0, maintenance.execute(10).await?.digests_created);
        let recovery = search_filter_service::use_cases::RecoverSavedSearchNotificationsHandler::new(search_filter_postgres::SqlxPendingSavedSearchNotificationReader::new(pool.clone()), use_case.clone());
        assert_eq!(0, recovery.execute(100).await?);
        let other = digest_maintenance(&pool);
        let (a, b) = tokio::join!(maintenance.execute(10), other.execute(10));
        assert_eq!(1, a?.digests_created + b?.digests_created);
        assert_eq!(0, maintenance.execute(10).await?.digests_created);
        assert!(failure_ids(handler(event("replay", job_body(user, first, listing, origin)?), use_case.as_ref()).await?).is_empty());
        let counts: (i64, i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM notifications WHERE user_id = $1), (SELECT count(*) FROM saved_search_digest_members WHERE user_id = $1), (SELECT count(*) FROM notification_deliveries d JOIN saved_search_digests g USING(digest_id) WHERE g.user_id = $1), (SELECT count(*) FROM search_filter_matches WHERE user_id = $1)")
            .bind(user.as_uuid()).fetch_one(&pool).await?;
        assert_eq!((2, 2, 1, 2), counts);
        let id: uuid::Uuid = sqlx::query_scalar("SELECT d.notification_delivery_id FROM notification_deliveries d JOIN saved_search_digests g USING(digest_id) WHERE g.user_id = $1").bind(user.as_uuid()).fetch_one(&pool).await?;
        let id = notification_delivery_id(id)?;
        let repository = notification_postgres::SqlxNotificationDeliveryRepository::new(pool.clone());
        let now = time::OffsetDateTime::now_utc();
        let ClaimNotificationDeliveryOutcome::Claimed { delivery, source } = repository.claim_and_load_source(id, now, now + time::Duration::minutes(5), uuid::Uuid::now_v7()).await? else { panic!("expected digest claim") };
        assert_eq!(2, source.unwrap().saved_search_digest.unwrap().members.len());
        // Accepted provider result and its exact lease survive a lost commit response.
        assert!(repository.mark_delivered(id, delivery.lease_token, "test-provider-accepted", now).await?);
        assert!(repository.mark_delivered(id, delivery.lease_token, "test-provider-accepted", now).await?);
        assert!(matches!(repository.claim_and_load_source(id, now, now + time::Duration::minutes(5), uuid::Uuid::now_v7()).await?, ClaimNotificationDeliveryOutcome::Delivered));
        Ok(())
    }.await;
    assert!(
        result.is_ok(),
        "digest recovery and concurrent finalization: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn zero_matches_and_legacy_matches_produce_no_new_digest() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let user = seed_user(&pool).await?;
        let filter = seed_filter(&pool, user).await?;
        let (listing, origin) = seed_product(&pool).await?;
        seed_match(&pool, user, filter, listing, origin).await?;
        let generator = compose_search_filter_match_notification_use_case(pool.clone());
        assert!(
            failure_ids(
                handler(
                    event("legacy", job_body(user, filter, listing, origin)?),
                    generator.as_ref()
                )
                .await?
            )
            .is_empty()
        );
        assert_notification(&pool, user, filter, origin).await?;
        let end = seed_digest_run(&pool, user, &[filter]).await?;
        terminal_filter(&pool, end, filter).await?;
        assert_eq!(
            0,
            digest_maintenance(&pool).execute(10).await?.digests_created
        );
        assert!(
            failure_ids(
                handler(
                    event("legacy-replay", job_body(user, filter, listing, origin)?),
                    generator.as_ref()
                )
                .await?
            )
            .is_empty()
        );
        assert_notification(&pool, user, filter, origin).await?;
        Ok(())
    }
    .await;
    assert!(result.is_ok(), "zero/legacy cutover: {result:?}");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn interrupted_run_finalizes_successes_and_late_members_are_carried_to_next_run() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let user = seed_user(&pool).await?;
        let first = seed_filter(&pool, user).await?;
        let failed = seed_filter(&pool, user).await?;
        let (listing, origin) = seed_product(&pool).await?;
        let end = seed_digest_run(&pool, user, &[first, failed]).await?;
        seed_run_match(&pool, user, first, listing, origin, end).await?;
        let generator = compose_search_filter_match_notification_use_case(pool.clone());
        assert!(failure_ids(handler(event("success", job_body(user, first, listing, origin)?), generator.as_ref()).await?).is_empty());
        terminal_filter(&pool, end, first).await?;
        let maintenance = digest_maintenance(&pool);
        let completed_user = seed_user(&pool).await?;
        let completed_filter = seed_filter(&pool, completed_user).await?;
        sqlx::query("INSERT INTO saved_search_digest_groups(window_end, user_id) VALUES ($1, $2)").bind(end).bind(completed_user.as_uuid()).execute(&pool).await?;
        sqlx::query("INSERT INTO search_filter_matching_run_filters(window_end, user_id, user_search_filter_id, snapshot, terminal) SELECT $1, user_id, user_search_filter_id, to_jsonb(f), true FROM search_filters f WHERE user_search_filter_id = $2")
            .bind(end).bind(completed_filter.as_uuid()).execute(&pool).await?;
        seed_run_match(&pool, completed_user, completed_filter, listing, origin, end).await?;
        assert!(failure_ids(handler(event("completed-user", job_body(completed_user, completed_filter, listing, origin)?), generator.as_ref()).await?).is_empty());
        assert_eq!(1, maintenance.execute(10).await?.digests_created);
        assert_eq!(0, maintenance.execute(10).await?.digests_created);
        sqlx::query("UPDATE search_filter_matching_runs SET recovery_deadline = now() - INTERVAL '1 second' WHERE window_end = $1").bind(end).execute(&pool).await?;
        assert_eq!(1, maintenance.execute(10).await?.digests_created);

        let (late_listing, late_origin) = seed_product(&pool).await?;
        seed_run_match(&pool, user, failed, late_listing, late_origin, end).await?;
        assert!(failure_ids(handler(event("late", job_body(user, failed, late_listing, late_origin)?), generator.as_ref()).await?).is_empty());
        assert_eq!(0, maintenance.execute(10).await?.digests_created);
        let unassigned: i64 = sqlx::query_scalar("SELECT count(*) FROM saved_search_digest_members WHERE user_id = $1 AND digest_id IS NULL").bind(user.as_uuid()).fetch_one(&pool).await?;
        assert_eq!(1, unassigned);
        let next = seed_digest_run(&pool, user, &[first, failed]).await?;
        terminal_filter(&pool, next, first).await?;
        terminal_filter(&pool, next, failed).await?;
        assert_eq!(1, maintenance.execute(10).await?.digests_created);
        let sizes: Vec<i64> = sqlx::query_scalar("SELECT count(*) FROM saved_search_digest_members WHERE user_id = $1 GROUP BY digest_id ORDER BY digest_id").bind(user.as_uuid()).fetch_all(&pool).await?;
        assert_eq!(vec![1, 1], sizes);
        Ok(())
    }.await;
    assert!(result.is_ok(), "partial run and late arrivals: {result:?}");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn digest_delivery_rechecks_filter_preference_and_fences_individual_email() {
    let result: TestResult = async {
        use notification_service::ports::notification_delivery_repository::{NotificationDeliveryRepository, ClaimNotificationDeliveryOutcome};
        let pool = get_postgres_client().await;
        let user = seed_user(&pool).await?;
        let filter = seed_filter(&pool, user).await?;
        let (listing, origin) = seed_product(&pool).await?;
        let end = seed_digest_run(&pool, user, &[filter]).await?;
        seed_run_match(&pool, user, filter, listing, origin, end).await?;
        let generator = compose_search_filter_match_notification_use_case(pool.clone());
        assert!(failure_ids(handler(event("new", job_body(user, filter, listing, origin)?), generator.as_ref()).await?).is_empty());
        let old_path = sqlx::query("INSERT INTO notification_deliveries(notification_delivery_id, notification_id, channel, target_key) SELECT $1, notification_id, 'EMAIL', 'PRIMARY' FROM notifications WHERE user_id = $2")
            .bind(uuid::Uuid::now_v7()).bind(user.as_uuid()).execute(&pool).await;
        assert!(old_path.is_err());
        terminal_filter(&pool, end, filter).await?;
        assert_eq!(1, digest_maintenance(&pool).execute(10).await?.digests_created);
        sqlx::query("UPDATE search_filters SET notifications = false WHERE user_search_filter_id = $1").bind(filter.as_uuid()).execute(&pool).await?;
        let id: uuid::Uuid = sqlx::query_scalar("SELECT d.notification_delivery_id FROM notification_deliveries d JOIN saved_search_digests g USING(digest_id) WHERE g.user_id = $1").bind(user.as_uuid()).fetch_one(&pool).await?;
        let repository = notification_postgres::SqlxNotificationDeliveryRepository::new(pool.clone());
        let now = time::OffsetDateTime::now_utc();
        let ClaimNotificationDeliveryOutcome::Claimed { source, .. } = repository.claim_and_load_source(notification_delivery_id(id)?, now, now + time::Duration::minutes(5), uuid::Uuid::now_v7()).await? else { panic!("expected digest claim") };
        assert!(source.is_none());
        assert_eq!(1, notification_count(&pool, user, filter, origin).await?);
        Ok(())
    }.await;
    assert!(
        result.is_ok(),
        "delivery preference and cutover fence: {result:?}"
    );
}

fn notification_delivery_id(
    id: uuid::Uuid,
) -> Result<
    notification_core::notification_delivery_id::NotificationDeliveryId,
    domain_primitives::object_id::ObjectIdError,
> {
    id.try_into()
}

fn digest_maintenance(
    pool: &sqlx::PgPool,
) -> notification_service::saved_search_digest::MaintainSavedSearchDigestsHandler<
    platform_postgres::SqlxUnitOfWork,
    notification_postgres::SqlxSavedSearchDigestStoreFactory,
> {
    notification_service::saved_search_digest::MaintainSavedSearchDigestsHandler::new(
        platform_postgres::SqlxUnitOfWork::new(pool.clone()),
        notification_postgres::SqlxSavedSearchDigestStoreFactory,
    )
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn free_monthly_quota_selects_ten_matches_before_creating_one_email() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let user = seed_user(&pool).await?;
        sqlx::query("UPDATE users SET tier = 'FREE' WHERE user_id = $1").bind(user.as_uuid()).execute(&pool).await?;
        let filter = seed_filter(&pool, user).await?;
        let end = seed_digest_run(&pool, user, &[filter]).await?;
        let generator = compose_search_filter_match_notification_use_case(pool.clone());
        let mut jobs = Vec::new();
        for _ in 0..11 {
            let (listing, origin) = seed_product(&pool).await?;
            seed_run_match(&pool, user, filter, listing, origin, end).await?;
            jobs.push(job_body(user, filter, listing, origin)?);
        }
        for body in &jobs { assert!(failure_ids(handler(event("quota", body.clone()), generator.as_ref()).await?).is_empty()); }
        terminal_filter(&pool, end, filter).await?;
        let report = digest_maintenance(&pool).execute(10).await?;
        assert_eq!(1, report.digests_created);
        assert_eq!(10, report.matches_enrolled);
        for body in jobs { assert!(failure_ids(handler(event("quota-replay", body), generator.as_ref()).await?).is_empty()); }
        let counts: (i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM search_filter_matches WHERE user_id = $1), (SELECT count(*) FROM notifications WHERE user_id = $1), (SELECT count(*) FROM saved_search_digest_members WHERE user_id = $1)").bind(user.as_uuid()).fetch_one(&pool).await?;
        assert_eq!((11, 10, 10), counts);
        Ok(())
    }.await;
    assert!(
        result.is_ok(),
        "digest preserves monthly match quota: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn finalization_rollback_retains_members_and_delivery_uses_snapshots_with_current_guards() {
    let result: TestResult = async {
        use application::transaction::UnitOfWork;
        use notification_service::{saved_search_digest::{SavedSearchDigestStore, SavedSearchDigestStoreFactory}, ports::notification_delivery_repository::{NotificationDeliveryRepository, ClaimNotificationDeliveryOutcome}};
        let pool = get_postgres_client().await;
        let user = seed_user(&pool).await?;
        let filter = seed_filter(&pool, user).await?;
        let end = seed_digest_run(&pool, user, &[filter]).await?;
        let generator = compose_search_filter_match_notification_use_case(pool.clone());
        let mut products = Vec::new();
        for _ in 0..3 {
            let (listing, origin) = seed_product(&pool).await?;
            seed_run_match(&pool, user, filter, listing, origin, end).await?;
            assert!(failure_ids(handler(event("snapshot", job_body(user, filter, listing, origin)?), generator.as_ref()).await?).is_empty());
            products.push((listing, origin));
        }
        terminal_filter(&pool, end, filter).await?;
        let unit = platform_postgres::SqlxUnitOfWork::new(pool.clone());
        let mut tx = unit.begin().await?;
        assert_eq!(1, notification_postgres::SqlxSavedSearchDigestStoreFactory.in_transaction(&mut tx).finalize_next().await?.unwrap().digests_created);
        drop(tx); // Simulate death after enrollment/outbox writes but before commit.
        // SQLx schedules rollback on drop. Wait for the group lock to be released
        // before asserting recovery; SKIP LOCKED intentionally defers live claims.
        sqlx::query("SELECT user_id FROM saved_search_digest_groups WHERE window_end = $1 AND user_id = $2 FOR UPDATE")
            .bind(end).bind(user.as_uuid()).fetch_one(&pool).await?;
        assert_eq!(1, digest_maintenance(&pool).execute(10).await?.digests_created);
        sqlx::query("DELETE FROM notifications WHERE user_id = $1").bind(user.as_uuid()).execute(&pool).await?;
        sqlx::query("UPDATE search_filters SET name = 'Renamed after finalization' WHERE user_search_filter_id = $1").bind(filter.as_uuid()).execute(&pool).await?;
        sqlx::query("UPDATE product_listings SET lifecycle = 'WITHDRAWN', availability = NULL WHERE product_listing_id = $1").bind(products[0].0.as_uuid()).execute(&pool).await?;
        sqlx::query("INSERT INTO product_listing_content_assessments(product_listing_id, source_event_id, decision, category) VALUES ($1, $2, 'REQUIRES_CONSENT', 'NAZI_GERMANY')").bind(products[1].0.as_uuid()).bind(products[1].1.as_uuid()).execute(&pool).await?;
        let id: uuid::Uuid = sqlx::query_scalar("SELECT d.notification_delivery_id FROM notification_deliveries d JOIN saved_search_digests g USING(digest_id) WHERE g.user_id = $1").bind(user.as_uuid()).fetch_one(&pool).await?;
        let now = time::OffsetDateTime::now_utc();
        let repository = notification_postgres::SqlxNotificationDeliveryRepository::new(pool.clone());
        let ClaimNotificationDeliveryOutcome::Claimed { source, .. } = repository.claim_and_load_source(notification_delivery_id(id)?, now, now + time::Duration::minutes(5), uuid::Uuid::now_v7()).await? else { panic!("expected digest") };
        let digest = source.unwrap().saved_search_digest.unwrap();
        assert_eq!(2, digest.members.len());
        for member in &digest.members {
            let notification_core::notification::NotificationContent::SearchFilter { user_search_filter_name, .. } = member else { panic!("unexpected member") };
            assert_eq!("Fixture filter", user_search_filter_name.as_ref());
        }
        assert!(digest.members.iter().any(|member| matches!(member, notification_core::notification::NotificationContent::SearchFilter { snapshot, .. } if snapshot.content_policy == Some(product_listing_core::content_policy::ContentPolicyDecision::RequiresConsent(product_listing_core::content_policy::SensitiveContentCategory::NaziGermany)))));
        Ok(())
    }.await;
    assert!(
        result.is_ok(),
        "finalization rollback/current delivery guards: {result:?}"
    );
}
#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn newer_run_cannot_enroll_members_from_an_unfinished_older_group() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let user = seed_user(&pool).await?;
        let filter = seed_filter(&pool, user).await?;
        let old_end = seed_digest_run(&pool, user, &[filter]).await?;
        let (first, first_origin) = seed_product(&pool).await?;
        let (second, second_origin) = seed_product(&pool).await?;
        seed_run_match(&pool, user, filter, first, first_origin, old_end).await?;
        seed_run_match(&pool, user, filter, second, second_origin, old_end).await?;
        let generator = compose_search_filter_match_notification_use_case(pool.clone());
        assert!(failure_ids(handler(event("selected", job_body(user, filter, first, first_origin)?), generator.as_ref()).await?).is_empty());
        terminal_filter(&pool, old_end, filter).await?;
        sqlx::query("UPDATE search_filter_matching_runs SET status = 'INTERRUPTED' WHERE window_end = $1")
            .bind(old_end).execute(&pool).await?;
        // The old group's second selection is still in retry custody when a new run completes.
        let next_end = seed_digest_run(&pool, user, &[filter]).await?;
        terminal_filter(&pool, next_end, filter).await?;
        let maintenance = digest_maintenance(&pool);
        let report = maintenance.execute(10).await?;
        assert_eq!(1, report.groups_finalized);
        assert_eq!(0, report.digests_created);
        let unassigned: i64 = sqlx::query_scalar("SELECT count(*) FROM saved_search_digest_members WHERE user_id = $1 AND digest_id IS NULL")
            .bind(user.as_uuid()).fetch_one(&pool).await?;
        assert_eq!(1, unassigned);

        assert!(failure_ids(handler(event("recovered", job_body(user, filter, second, second_origin)?), generator.as_ref()).await?).is_empty());
        let report = maintenance.execute(10).await?;
        assert_eq!(1, report.digests_created);
        assert_eq!(2, report.matches_enrolled);
        let delivered_run: time::OffsetDateTime = sqlx::query_scalar("SELECT window_end FROM saved_search_digests WHERE user_id = $1")
            .bind(user.as_uuid()).fetch_one(&pool).await?;
        assert_eq!(old_end, delivered_run);
        Ok(())
    }.await;
    assert!(
        result.is_ok(),
        "unfinished group retains membership: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn digest_claim_rejects_snapshot_identity_corruption_and_preserves_delivery_target() {
    let result: TestResult = async {
        use notification_service::ports::notification_delivery_repository::{
            ClaimNotificationDeliveryOutcome, NotificationDeliveryError, NotificationDeliveryRepository,
        };
        let pool = get_postgres_client().await;
        let user = seed_user(&pool).await?;
        let filter = seed_filter(&pool, user).await?;
        let (listing, origin) = seed_product(&pool).await?;
        let end = seed_digest_run(&pool, user, &[filter]).await?;
        seed_run_match(&pool, user, filter, listing, origin, end).await?;
        let generator = compose_search_filter_match_notification_use_case(pool.clone());
        assert!(failure_ids(handler(event("snapshot", job_body(user, filter, listing, origin)?), generator.as_ref()).await?).is_empty());
        terminal_filter(&pool, end, filter).await?;
        assert_eq!(1, digest_maintenance(&pool).execute(10).await?.digests_created);
        let id: uuid::Uuid = sqlx::query_scalar("SELECT d.notification_delivery_id FROM notification_deliveries d JOIN saved_search_digests g USING(digest_id) WHERE g.user_id = $1")
            .bind(user.as_uuid()).fetch_one(&pool).await?;
        let id = notification_delivery_id(id)?;
        sqlx::query("UPDATE saved_search_digest_members SET snapshot = jsonb_set(snapshot, '{notification_id}', to_jsonb($2::text)) WHERE user_id = $1")
            .bind(user.as_uuid()).bind(uuid::Uuid::now_v7().to_string()).execute(&pool).await?;
        let repository = notification_postgres::SqlxNotificationDeliveryRepository::new(pool.clone());
        let now = time::OffsetDateTime::now_utc();
        assert!(matches!(repository.claim_and_load_source(id, now, now + time::Duration::minutes(5), uuid::Uuid::now_v7()).await,
            Err(NotificationDeliveryError::InvalidPersistedState { .. })));
        // Failed decoding rolls back the claim; corrected state can be claimed normally.
        sqlx::query("UPDATE saved_search_digest_members SET snapshot = jsonb_set(snapshot, '{notification_id}', to_jsonb(notification_id::text)) WHERE user_id = $1")
            .bind(user.as_uuid()).execute(&pool).await?;
        sqlx::query("UPDATE notification_deliveries SET target_key = 'SECONDARY' WHERE notification_delivery_id = $1")
            .bind(id.as_uuid()).execute(&pool).await?;
        let ClaimNotificationDeliveryOutcome::Claimed { delivery, source } = repository.claim_and_load_source(id, now, now + time::Duration::minutes(5), uuid::Uuid::now_v7()).await? else { panic!("expected corrected claim") };
        assert_eq!(1, delivery.attempt_count);
        let source = source.unwrap();
        assert_eq!("SECONDARY", source.target_key.as_str());
        assert_eq!(delivery.source_id, source.source_id);
        Ok(())
    }.await;
    assert!(
        result.is_ok(),
        "digest identity and target decoding: {result:?}"
    );
}

async fn seed_digest_run(
    pool: &sqlx::PgPool,
    user: UserId,
    filters: &[UserSearchFilterId],
) -> Result<time::OffsetDateTime, sqlx::Error> {
    let mut tx = pool.begin().await?;
    // Tests share one isolated database; earlier completed fixtures need no live run.
    sqlx::query(
        "UPDATE search_filter_matching_runs SET status = 'COMPLETED' WHERE status = 'RUNNING'",
    )
    .execute(&mut *tx)
    .await?;
    let end = sqlx::query_scalar("INSERT INTO search_filter_matching_runs(window_end) VALUES (clock_timestamp()) RETURNING window_end").fetch_one(&mut *tx).await?;
    sqlx::query("INSERT INTO saved_search_digest_groups(window_end, user_id) VALUES ($1, $2)")
        .bind(end)
        .bind(user.as_uuid())
        .execute(&mut *tx)
        .await?;
    for filter in filters {
        sqlx::query("INSERT INTO search_filter_matching_run_filters(window_end, user_id, user_search_filter_id, snapshot) SELECT $1, user_id, user_search_filter_id, to_jsonb(f) FROM search_filters f WHERE user_search_filter_id = $2")
            .bind(end).bind(filter.as_uuid()).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(end)
}
async fn seed_run_match(
    pool: &sqlx::PgPool,
    user: UserId,
    filter: UserSearchFilterId,
    listing: ProductListingId,
    origin: EventId,
    end: time::OffsetDateTime,
) -> TestResult {
    use application::transaction::{Transaction, UnitOfWork};
    use search_filter_service::ports::{SearchFilterMatchWriter, SearchFilterMatchWriterFactory};
    let unit = platform_postgres::SqlxUnitOfWork::new(pool.clone());
    let mut tx = unit.begin().await?;
    let inserted = search_filter_postgres::SqlxSearchFilterMatchWriterFactory
        .in_transaction(&mut tx)
        .insert_for_run(
            &[search_filter_core::SearchFilterProductListingMatch {
                user_id: user,
                user_search_filter_id: filter,
                product_listing_id: listing,
                origin_event_id: origin,
                user_search_filter_name: Some("Fixture filter".into()),
                price_match_valuation: None,
                feedback: None,
            }],
            end,
        )
        .await?;
    assert_eq!(1, inserted.inserted);
    tx.commit().await?;
    Ok(())
}

async fn terminal_filter(
    pool: &sqlx::PgPool,
    end: time::OffsetDateTime,
    filter: UserSearchFilterId,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE search_filter_matching_run_filters SET terminal = true WHERE window_end = $1 AND user_search_filter_id = $2").bind(end).bind(filter.as_uuid()).execute(pool).await?;
    Ok(())
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn missing_match_is_retried_then_committed_match_creates_one_notification_and_intent() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let user = seed_user(&pool).await?;
        let filter = seed_filter(&pool, user).await?;
        let (product, source) = seed_product(&pool).await?;
        let body = job_body(user, filter, product, source)?;
        let use_case = compose_search_filter_match_notification_use_case(pool.clone());

        assert_eq!(
            failure_ids(handler(event("missing", body.clone()), use_case.as_ref()).await?),
            ["missing"]
        );
        assert_eq!(0, notification_count(&pool, user, filter, source).await?);
        seed_match(&pool, user, filter, product, source).await?;
        assert!(
            failure_ids(handler(event("retry", body.clone()), use_case.as_ref()).await?).is_empty()
        );
        assert!(
            failure_ids(handler(event("duplicate", body), use_case.as_ref()).await?).is_empty()
        );
        assert_notification(&pool, user, filter, source).await?;
        Ok(())
    }
    .await;
    assert!(
        result.is_ok(),
        "match notification missing-source/redelivery composition failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn overlapping_invocations_insert_only_one_match_notification_and_delivery_intent() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let user = seed_user(&pool).await?;
        let filter = seed_filter(&pool, user).await?;
        let (product, source) = seed_product(&pool).await?;
        seed_match(&pool, user, filter, product, source).await?;
        let body = job_body(user, filter, product, source)?;
        let use_case = compose_search_filter_match_notification_use_case(pool.clone());

        let mut barrier = pool.begin().await?;
        sqlx::query("LOCK TABLE notifications IN ACCESS EXCLUSIVE MODE")
            .execute(&mut *barrier)
            .await?;
        let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *barrier)
            .await?;
        let first = tokio::spawn({
            let use_case = use_case.clone();
            let body = body.clone();
            async move { handler(event("first", body), use_case.as_ref()).await }
        });
        wait_for_blocked(&pool, blocker, 1).await?;
        let second = tokio::spawn({
            let use_case = use_case.clone();
            async move { handler(event("second", body), use_case.as_ref()).await }
        });
        wait_for_blocked(&pool, blocker, 2).await?;
        barrier.commit().await?;
        assert!(failure_ids(first.await??).is_empty());
        assert!(failure_ids(second.await??).is_empty());
        assert_notification(&pool, user, filter, source).await?;
        Ok(())
    }
    .await;
    assert!(
        result.is_ok(),
        "match notification concurrent insert composition failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn withdrawn_listing_suppresses_historical_match_notification() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let user = seed_user(&pool).await?;
        let filter = seed_filter(&pool, user).await?;
        let (product, source) = seed_product(&pool).await?;
        seed_match(&pool, user, filter, product, source).await?;
        sqlx::query("UPDATE product_listings SET lifecycle = 'WITHDRAWN', availability = NULL WHERE product_listing_id = $1")
            .bind(product.as_uuid()).execute(&pool).await?;

        let use_case = compose_search_filter_match_notification_use_case(pool.clone());
        let body = job_body(user, filter, product, source)?;
        assert!(failure_ids(handler(event("withdrawn", body), use_case.as_ref()).await?).is_empty());
        assert_eq!(0, notification_count(&pool, user, filter, source).await?);
        Ok(())
    }.await;
    assert!(
        result.is_ok(),
        "match notification withdrawn-listing composition failed: {result:?}"
    );
}

async fn wait_for_blocked(pool: &sqlx::PgPool, blocker: i32, expected: i64) -> TestResult {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let blocked: i64 = sqlx::query_scalar("WITH RECURSIVE blocked(pid) AS (SELECT pid FROM pg_stat_activity WHERE $1 = ANY(pg_blocking_pids(pid)) UNION SELECT a.pid FROM pg_stat_activity a JOIN blocked b ON b.pid = ANY(pg_blocking_pids(a.pid))) SELECT count(*) FROM blocked")
                .bind(blocker).fetch_one(pool).await?;
            if blocked >= expected { return Ok::<(), sqlx::Error>(()); }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }).await??;
    Ok(())
}

fn event(id: &str, body: String) -> LambdaEvent<SqsEvent> {
    let mut message = SqsMessage::default();
    message.message_id = Some(id.to_owned());
    message.body = Some(body);
    let mut payload = SqsEvent::default();
    payload.records = vec![message];
    let mut context = Context::default();
    context.deadline = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        + 60_000;
    LambdaEvent::new(payload, context)
}

fn failure_ids(response: SqsBatchResponse) -> Vec<String> {
    response
        .batch_item_failures
        .into_iter()
        .map(|failure| failure.item_identifier)
        .collect()
}

fn job_body(
    user: UserId,
    filter: UserSearchFilterId,
    product: ProductListingId,
    source: EventId,
) -> Result<String, aura_historia_jobs::WireError> {
    encode(&DomainJob {
        target_queue: WorkerQueue::SearchFilterMatchNotification,
        idempotency_key: IdempotencyKey::new(format!(
            "search-filter-match:{user}:{filter}:{product}:{source}"
        )),
        ordering_key: OrderingKey::new(format!("user:{user}")),
        payload: DomainJobPayload::SearchFilterMatchCreated(SearchFilterMatchCreatedJob {
            user_id: user,
            user_search_filter_id: filter,
            product_listing_id: product,
            origin_event_id: source,
        }),
    })
}

async fn seed_user(pool: &sqlx::PgPool) -> Result<UserId, sqlx::Error> {
    let user = UserId::new();
    sqlx::query(
        "INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'ULTIMATE', 'USER')",
    )
    .bind(user.as_uuid())
    .bind(format!("lambda-match-{user}@example.test"))
    .execute(pool)
    .await?;
    Ok(user)
}

async fn seed_filter(pool: &sqlx::PgPool, user: UserId) -> Result<UserSearchFilterId, sqlx::Error> {
    let filter = UserSearchFilterId::new();
    sqlx::query("INSERT INTO search_filters (user_search_filter_id, user_id, name, notifications, state, search, language, currency, matching_evidence_policy) VALUES ($1, $2, 'Fixture filter', true, 'ACTIVE', $3, 'en', 'EUR', 'TEXT_ONLY')")
        .bind(filter.as_uuid()).bind(user.as_uuid()).bind(json!({})).execute(pool).await?;
    Ok(filter)
}

async fn seed_product(pool: &sqlx::PgPool) -> Result<(ProductListingId, EventId), sqlx::Error> {
    let product = ProductListingId::new();
    let source = EventId::new();
    let listing_source = ListingSourceId::new();
    let mut tx = pool.begin().await?;
    sqlx::query("WITH operator AS (INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, 'Fixture operator') RETURNING party_id) INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) SELECT $3, $4, 'Match notification lambda source', party_id FROM operator")
        .bind(uuid::Uuid::now_v7()).bind(format!("match-operator-{}", listing_source.as_uuid()))
        .bind(listing_source.as_uuid()).bind(format!("match-source-{}", listing_source.as_uuid()))
        .execute(&mut *tx).await?;
    sqlx::query("INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, title_text, title_language, availability, lifecycle, url, product_images) VALUES ($1, $2, $3, $3, $3, $4, $5, 'Match notification lambda product', 'en', 'AVAILABLE', 'ACTIVE', 'https://example.test/product', '[]')")
        .bind(product.as_uuid()).bind(format!("match-product-{}", &product.as_uuid().simple().to_string()[26..]))
        .bind(source.as_uuid()).bind(listing_source.as_uuid()).bind(product.to_string())
        .execute(&mut *tx).await?;
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_CHANGED', 'DOMAIN', 1, $3, now())")
        .bind(source.as_uuid()).bind(product.as_uuid())
        .bind(json!({"images": {"previousCount": 0, "currentCount": 0}})).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok((product, source))
}

async fn seed_match(
    pool: &sqlx::PgPool,
    user: UserId,
    filter: UserSearchFilterId,
    product: ProductListingId,
    source: EventId,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO search_filter_matches (user_id, user_search_filter_id, product_listing_id, origin_event_id, user_search_filter_name) VALUES ($1, $2, $3, $4, 'Fixture filter')")
        .bind(user.as_uuid()).bind(filter.as_uuid()).bind(product.as_uuid()).bind(source.as_uuid())
        .execute(pool).await?;
    Ok(())
}

async fn notification_count(
    pool: &sqlx::PgPool,
    user: UserId,
    filter: UserSearchFilterId,
    source: EventId,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT count(*) FROM notifications WHERE user_id = $1 AND user_search_filter_id = $2 AND origin_event_id = $3")
        .bind(user.as_uuid()).bind(filter.as_uuid()).bind(source.as_uuid()).fetch_one(pool).await
}

async fn assert_notification(
    pool: &sqlx::PgPool,
    user: UserId,
    filter: UserSearchFilterId,
    source: EventId,
) -> TestResult {
    let rows: Vec<(String, Value, i64)> = sqlx::query_as("SELECT n.kind, n.payload, (SELECT count(*) FROM notification_deliveries d WHERE d.notification_id = n.notification_id) FROM notifications n WHERE n.user_id = $1 AND n.user_search_filter_id = $2 AND n.origin_event_id = $3")
        .bind(user.as_uuid()).bind(filter.as_uuid()).bind(source.as_uuid()).fetch_all(pool).await?;
    assert_eq!(1, rows.len());
    assert_eq!("SEARCH_FILTER_MATCH", rows[0].0);
    assert_eq!(
        Some("SEARCH_FILTER"),
        rows[0].1.get("type").and_then(Value::as_str)
    );
    assert_eq!(
        Some("Fixture filter"),
        rows[0]
            .1
            .get("user_search_filter_name")
            .and_then(Value::as_str)
    );
    assert_eq!(1, rows[0].2);
    Ok(())
}
