mod support;

use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use content_notice_core::{
    additional_url::AdditionalUrls,
    good_faith_statement_version::GoodFaithStatementVersion,
    notice_explanation::NoticeExplanation,
    reason_category::ReasonCategory,
    submitter::{Submitter, SubmitterEmail, SubmitterName},
    submitter_role::SubmitterRole,
};
use content_notice_postgres::{
    SqlxContentNoticeRepositoryFactory, SqlxContentNoticeSubmissionHistoryFactory,
    SqlxContentNoticeTargetReaderFactory,
};
use content_notice_service::{
    ports::SystemContentNoticeClock,
    submission_policy::MAX_NOTICES_PER_ADDRESS,
    use_cases::commands::submit_content_notice::{
        ContentNoticeTargetReference, SubmitContentNoticeCommand, SubmitContentNoticeError,
        SubmitContentNoticeHandler, SubmitContentNoticeResult, SubmitContentNoticeUseCase,
    },
};
use listing_source_core::ListingSourceId;
use localization::Language;
use platform_postgres::SqlxUnitOfWork;
use product_listing_core::listing_lifecycle::ListingLifecycle;
use sqlx::PgPool;
use std::sync::Arc;
use support::{seed_listing_source, seed_product_listing};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use tokio::task::JoinSet;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

type Outcome = Result<SubmitContentNoticeResult, SubmitContentNoticeError>;

fn handler(pool: &PgPool) -> Arc<dyn SubmitContentNoticeUseCase> {
    Arc::new(SubmitContentNoticeHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxContentNoticeTargetReaderFactory::new(),
        SqlxContentNoticeRepositoryFactory::new(),
        SqlxContentNoticeSubmissionHistoryFactory::new(),
        SystemContentNoticeClock,
    ))
}

fn context() -> OperationContext {
    OperationContext {
        principal: Principal::Anonymous,
        request_id: RequestId::new("request"),
        correlation_id: CorrelationId::new("correlation"),
    }
}

fn command(
    listing_source_id: ListingSourceId,
    email: &str,
    explanation: &str,
) -> SubmitContentNoticeCommand {
    SubmitContentNoticeCommand {
        target: ContentNoticeTargetReference::ListingSource { listing_source_id },
        submitter_role: SubmitterRole::RightsHolder,
        reason_category: ReasonCategory::RightsInfringement,
        explanation: Some(NoticeExplanation::try_from(explanation).unwrap()),
        additional_urls: AdditionalUrls::default(),
        submitter: Submitter {
            name: SubmitterName::try_from("Erika Mustermann").unwrap(),
            email: SubmitterEmail::try_from(email).unwrap(),
            language: Language::De,
        },
        good_faith_statement_version: GoodFaithStatementVersion::GoodFaithV1,
    }
}

/// Runs the submissions concurrently, each in its own transaction. A gate holds back every
/// insert into `content_notices`, but no read, until all submissions wait on a lock. They
/// therefore overlap however they are scheduled, and only the address lock can keep one from
/// deciding on a history that another is about to change.
async fn submit_concurrently(
    pool: &PgPool,
    commands: Vec<SubmitContentNoticeCommand>,
) -> Vec<Outcome> {
    let submissions = commands.len();
    let mut gate = pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE content_notices IN SHARE MODE")
        .execute(&mut *gate)
        .await
        .unwrap();
    let handler = handler(pool);
    let mut running = JoinSet::new();
    for command in commands {
        let handler = Arc::clone(&handler);
        running.spawn(async move { handler.execute(&context(), command).await });
    }
    wait_until_waiting_on_locks(pool, submissions).await;
    gate.rollback().await.unwrap();
    running.join_all().await
}

async fn wait_until_waiting_on_locks(pool: &PgPool, sessions: usize) {
    for _ in 0..500 {
        let waiting: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity WHERE datname = current_database() AND wait_event_type = 'Lock'",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        if waiting as usize == sessions {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("{sessions} submissions did not all reach a lock");
}

async fn stored_notices(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM content_notices")
        .fetch_one(pool)
        .await
        .unwrap()
}

fn count(outcomes: &[Outcome], expected: fn(&Outcome) -> bool) -> usize {
    outcomes.iter().filter(|outcome| expected(outcome)).count()
}

fn stored(outcome: &Outcome) -> bool {
    matches!(outcome, Ok(SubmitContentNoticeResult::Stored { .. }))
}

fn duplicate(outcome: &Outcome) -> bool {
    matches!(outcome, Ok(SubmitContentNoticeResult::Duplicate))
}

fn rate_limited(outcome: &Outcome) -> bool {
    matches!(outcome, Err(SubmitContentNoticeError::RateLimited { .. }))
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_store_exactly_one_notice_for_concurrent_identical_submissions() {
    let pool = get_postgres_client().await;
    let source = seed_listing_source(&pool, "Antik Shop", None, None).await;
    let submissions = 8;

    let outcomes = submit_concurrently(
        &pool,
        (0..submissions)
            .map(|_| command(source, "erika@example.com", "Das Foto ist meins."))
            .collect(),
    )
    .await;

    assert_eq!(1, count(&outcomes, stored), "{outcomes:?}");
    assert_eq!(submissions - 1, count(&outcomes, duplicate), "{outcomes:?}");
    assert_eq!(1, stored_notices(&pool).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_never_store_more_than_the_budget_for_concurrent_distinct_submissions() {
    let pool = get_postgres_client().await;
    let source = seed_listing_source(&pool, "Antik Shop", None, None).await;
    let handler = handler(&pool);
    let remaining = 2;
    for index in 0..MAX_NOTICES_PER_ADDRESS - remaining {
        handler
            .execute(
                &context(),
                command(source, "erika@example.com", &format!("Altes Foto {index}.")),
            )
            .await
            .unwrap();
    }
    let addresses = [
        "erika@example.com",
        "Erika@Example.com",
        "ERIKA@EXAMPLE.COM",
    ];
    let submissions = 6;

    let outcomes = submit_concurrently(
        &pool,
        (0..submissions)
            .map(|index| {
                command(
                    source,
                    addresses[index % addresses.len()],
                    &format!("Neues Foto {index}."),
                )
            })
            .collect(),
    )
    .await;

    assert_eq!(remaining, count(&outcomes, stored), "{outcomes:?}");
    assert_eq!(
        submissions - remaining,
        count(&outcomes, rate_limited),
        "{outcomes:?}"
    );
    assert_eq!(MAX_NOTICES_PER_ADDRESS as i64, stored_notices(&pool).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_share_one_budget_and_one_duplicate_check_across_address_case() {
    let pool = get_postgres_client().await;
    let source = seed_listing_source(&pool, "Antik Shop", None, None).await;
    let handler = handler(&pool);
    for index in 0..MAX_NOTICES_PER_ADDRESS {
        handler
            .execute(
                &context(),
                command(
                    source,
                    "erika@example.com",
                    &format!("Foto Nummer {index}."),
                ),
            )
            .await
            .unwrap();
    }

    let resubmitted = handler
        .execute(
            &context(),
            command(source, "Erika@Example.COM", "Foto Nummer 0."),
        )
        .await;
    let new = handler
        .execute(
            &context(),
            command(source, "ERIKA@example.com", "Neues Foto."),
        )
        .await;
    let other_address = handler
        .execute(
            &context(),
            command(source, "max@example.com", "Neues Foto."),
        )
        .await;

    assert!(duplicate(&resubmitted), "{resubmitted:?}");
    let Err(SubmitContentNoticeError::RateLimited { retry_after }) = new else {
        panic!("expected rate limit, got {new:?}");
    };
    assert!(
        (24 * 60 * 60 - 60..=24 * 60 * 60).contains(&retry_after.seconds()),
        "{retry_after:?}"
    );
    assert!(stored(&other_address), "{other_address:?}");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_store_an_identical_submission_again_once_the_stored_one_left_the_window() {
    let pool = get_postgres_client().await;
    let source = seed_listing_source(&pool, "Antik Shop", None, None).await;
    let product_listing_id =
        seed_product_listing(&pool, source, ListingLifecycle::Active, Some("Vase")).await;
    let handler = handler(&pool);
    let context = context();
    let submit = || {
        let mut about_listing = command(source, "erika@example.com", "Das Foto ist meins.");
        about_listing.target = ContentNoticeTargetReference::ProductListing { product_listing_id };
        handler.execute(&context, about_listing)
    };
    assert!(stored(&submit().await));
    sqlx::query("UPDATE content_notices SET created = now() - interval '24 hours 1 second'")
        .execute(&pool)
        .await
        .unwrap();

    let outcome = submit().await;

    assert!(stored(&outcome), "{outcome:?}");
    assert_eq!(2, stored_notices(&pool).await);
}
