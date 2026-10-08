use application::transaction::{Transaction, UnitOfWork};
use platform_postgres::SqlxUnitOfWork;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::{Duration, OffsetDateTime};
use user_core::marketing_consent_sync_intent_id::MarketingConsentSyncIntentId;
use user_postgres::{
    SqlxConsentWorkflowCleanup, SqlxMarketingConsentIntentWorker,
    SqlxNewsletterConfirmationChallengesRepository,
};
use user_service::ports::marketing_consent_intents::ConsentWorkerClaimOutcome;
use user_service::ports::{
    ConsentWorkflowCleanup, ConsentWorkflowCleanupFactory, NewsletterConfirmationClock,
};
use user_service::use_cases::{CleanupConsentWorkflowHandler, CleanupConsentWorkflowUseCase};
use uuid::Uuid;

const BUSINESS_SCHEMA: Postgres = Postgres::new_schema_once("migrations");

#[derive(Clone, Copy)]
struct FixedClock(OffsetDateTime);

impl NewsletterConfirmationClock for FixedClock {
    fn now_utc(&self) -> OffsetDateTime {
        self.0
    }
}

async fn seed_intent(
    pool: &sqlx::PgPool,
    status: &str,
    source: &str,
    desired: bool,
    changed_at: OffsetDateTime,
    completed_at: Option<OffsetDateTime>,
) -> Uuid {
    let id = Uuid::now_v7();
    let completed_token = completed_at.map(|_| Uuid::now_v7());
    let lease_token = (status == "IN_PROGRESS").then(Uuid::now_v7);
    let lease_expiry = lease_token.map(|_| changed_at + Duration::hours(1));
    sqlx::query(
        "INSERT INTO marketing_email_consent_sync_intents \
         (intent_id, source_key, subject_type, source, email, recipient_key, desired, \
          changed_at, status, lease_token, lease_expires_at, completed_lease_token, completed_at, \
          completion_status, not_after) \
         VALUES ($1, $2, 'EMAIL_ONLY', $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)",
    )
    .bind(id)
    .bind(format!("cleanup-{id}"))
    .bind(source)
    .bind(format!("cleanup-{id}@example.test"))
    .bind("a".repeat(64))
    .bind(desired)
    .bind(changed_at)
    .bind(status)
    .bind(lease_token)
    .bind(lease_expiry)
    .bind(completed_token)
    .bind(completed_at)
    .bind(completed_at.map(|_| status))
    .bind(desired.then_some(changed_at + Duration::days(7)))
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn seed_challenge(
    pool: &sqlx::PgPool,
    created_at: OffsetDateTime,
    confirmed_at: Option<OffsetDateTime>,
    intent_id: Option<Uuid>,
) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO newsletter_subscription_confirmations \
         (confirmation_id, token_digest, email, recipient_key, created_at, expires_at, confirmed_at, resulting_intent_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(id)
    .bind(id.as_bytes().repeat(2))
    .bind(format!("challenge-{id}@example.test"))
    .bind("b".repeat(64))
    .bind(created_at)
    .bind(created_at + Duration::hours(24))
    .bind(confirmed_at)
    .bind(intent_id)
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn seed_receipt(
    pool: &sqlx::PgPool,
    processed_at: OffsetDateTime,
    retention_days: i64,
) -> String {
    let id = format!("cleanup-{}", Uuid::now_v7());
    sqlx::query(
        "INSERT INTO loops_webhook_receipts \
         (delivery_id, raw_body_sha256, provider_event_name, event_time, disposition, processed_at, expires_at) \
         VALUES ($1, $2, 'email.unsubscribed', $3, 'APPLIED_WITHDRAWAL', $3, $4)",
    )
    .bind(&id)
    .bind(vec![0_u8; 32])
    .bind(processed_at)
    .bind(processed_at + Duration::days(retention_days))
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn exists(pool: &sqlx::PgPool, table: &str, column: &str, id: Uuid) -> bool {
    let query = format!("SELECT EXISTS (SELECT 1 FROM {table} WHERE {column} = $1)");
    sqlx::query_scalar(sqlx::AssertSqlSafe(query))
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn bounded_cleanup_respects_equality_replay_and_unfinished_work() {
    let pool = get_postgres_client().await;
    let replica_identity: String = sqlx::query_scalar(
        "SELECT relreplident::text FROM pg_class WHERE oid = 'marketing_email_consent_sync_intents'::regclass",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!("d", replica_identity);
    let now = OffsetDateTime::now_utc().replace_nanosecond(0).unwrap();
    let old = now - Duration::days(121);
    let completed = seed_intent(
        &pool,
        "APPLIED",
        "AURA_DOUBLE_OPT_IN",
        true,
        old,
        Some(now - Duration::days(120)),
    )
    .await;
    let second_completed = seed_intent(
        &pool,
        "SUPERSEDED",
        "PROVIDER_RACE_REPAIR",
        false,
        old,
        Some(now - Duration::days(120)),
    )
    .await;
    let recent_completed = seed_intent(
        &pool,
        "APPLIED",
        "AURA_DOUBLE_OPT_IN",
        true,
        old,
        Some(now - Duration::days(120) + Duration::seconds(1)),
    )
    .await;
    let pending = seed_intent(&pool, "PENDING", "AURA_DOUBLE_OPT_IN", true, old, None).await;
    let in_progress = seed_intent(
        &pool,
        "IN_PROGRESS",
        "PROVIDER_RACE_REPAIR",
        false,
        old,
        None,
    )
    .await;
    let blocked = seed_intent(&pool, "BLOCKED", "AURA_DOUBLE_OPT_IN", true, old, Some(old)).await;
    let failed = seed_intent(
        &pool,
        "FAILED",
        "PROVIDER_RACE_REPAIR",
        false,
        old,
        Some(old),
    )
    .await;

    let confirmed = seed_challenge(
        &pool,
        now - Duration::days(8),
        Some(now - Duration::days(7) - Duration::seconds(1)),
        Some(completed),
    )
    .await;
    let second_confirmed = seed_challenge(
        &pool,
        now - Duration::days(8),
        Some(now - Duration::days(7)),
        Some(second_completed),
    )
    .await;
    let recent_confirmed = seed_challenge(
        &pool,
        now - Duration::days(8),
        Some(now - Duration::days(7) + Duration::seconds(1)),
        Some(recent_completed),
    )
    .await;
    let unconfirmed = seed_challenge(&pool, now - Duration::hours(24), None, None).await;
    let recent_unconfirmed = seed_challenge(
        &pool,
        now - Duration::hours(24) + Duration::seconds(1),
        None,
        None,
    )
    .await;
    let old_receipt = seed_receipt(&pool, now - Duration::days(120), 35).await;
    let second_receipt = seed_receipt(&pool, now - Duration::days(120), 120).await;
    let recent_receipt =
        seed_receipt(&pool, now - Duration::days(120) + Duration::seconds(1), 35).await;

    let handler = CleanupConsentWorkflowHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxNewsletterConfirmationChallengesRepository::new(),
        SqlxConsentWorkflowCleanup::new(),
        FixedClock(now),
    );
    assert!(handler.execute(0).await.is_err());
    assert!(handler.execute(1001).await.is_err());
    let first = handler.execute(1).await.unwrap();
    assert_eq!(1, first.unconfirmed_challenges_deleted);
    assert_eq!(1, first.confirmed_challenges_deleted);
    assert_eq!(1, first.completed_intents_deleted);
    assert_eq!(1, first.webhook_receipts_deleted);
    assert!(
        !exists(
            &pool,
            "newsletter_subscription_confirmations",
            "confirmation_id",
            unconfirmed
        )
        .await
    );
    assert!(
        !exists(
            &pool,
            "newsletter_subscription_confirmations",
            "confirmation_id",
            confirmed
        )
        .await
    );
    assert!(
        !exists(
            &pool,
            "marketing_email_consent_sync_intents",
            "intent_id",
            completed
        )
        .await
    );
    assert!(
        exists(
            &pool,
            "marketing_email_consent_sync_intents",
            "intent_id",
            second_completed
        )
        .await
    );
    assert!(
        exists(
            &pool,
            "newsletter_subscription_confirmations",
            "confirmation_id",
            second_confirmed
        )
        .await
    );
    assert!(
        exists(
            &pool,
            "newsletter_subscription_confirmations",
            "confirmation_id",
            recent_confirmed
        )
        .await
    );
    assert!(
        exists(
            &pool,
            "newsletter_subscription_confirmations",
            "confirmation_id",
            recent_unconfirmed
        )
        .await
    );
    for id in [recent_completed, pending, in_progress, blocked, failed] {
        assert!(
            exists(
                &pool,
                "marketing_email_consent_sync_intents",
                "intent_id",
                id
            )
            .await
        );
    }
    let deleted_receipts: i64 = sqlx::query_scalar("SELECT count(*) FROM loops_webhook_receipts WHERE delivery_id = ANY($1) AND processed_at <= $2")
        .bind(vec![old_receipt, second_receipt])
        .bind(now - Duration::days(120))
        .fetch_one(&pool).await.unwrap();
    assert_eq!(1, deleted_receipts);
    let recent_receipt_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM loops_webhook_receipts WHERE delivery_id = $1)",
    )
    .bind(recent_receipt)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(recent_receipt_exists);

    let missing = SqlxMarketingConsentIntentWorker::new();
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    let outcome = missing
        .claim_by_id(
            &mut tx,
            MarketingConsentSyncIntentId::try_from(completed).unwrap(),
        )
        .await
        .unwrap();
    assert!(matches!(outcome, ConsentWorkerClaimOutcome::Missing));
    tx.commit().await.unwrap();
    assert_eq!(
        0,
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM marketing_email_consent_sync_intents WHERE intent_id = $1"
        )
        .bind(completed)
        .fetch_one(&pool)
        .await
        .unwrap()
    );

    let second = handler.execute(1).await.unwrap();
    assert_eq!(1, second.confirmed_challenges_deleted);
    assert_eq!(1, second.completed_intents_deleted);
    assert!(
        !exists(
            &pool,
            "newsletter_subscription_confirmations",
            "confirmation_id",
            second_confirmed
        )
        .await
    );
    assert!(
        exists(
            &pool,
            "newsletter_subscription_confirmations",
            "confirmation_id",
            recent_confirmed
        )
        .await
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn adapter_rejects_invalid_batches_before_deletion() {
    let pool = get_postgres_client().await;
    let now = OffsetDateTime::now_utc();
    let uow = SqlxUnitOfWork::new(pool);
    let repo = SqlxConsentWorkflowCleanup::new();
    let mut tx = uow.begin().await.unwrap();
    let mut cleanup = repo.in_transaction(&mut tx);
    for batch in [0, 1001] {
        assert!(
            cleanup
                .cleanup_confirmed_challenges(now, batch)
                .await
                .is_err()
        );
        assert!(cleanup.cleanup_completed_intents(now, batch).await.is_err());
        assert!(cleanup.cleanup_webhook_receipts(now, batch).await.is_err());
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn later_failure_preserves_committed_batches_for_safe_retry() {
    let pool = get_postgres_client().await;
    let now = OffsetDateTime::now_utc().replace_nanosecond(0).unwrap();
    let challenge = seed_challenge(&pool, now - Duration::days(2), None, None).await;
    let receipt = seed_receipt(&pool, now - Duration::days(121), 120).await;
    sqlx::query("CREATE FUNCTION reject_consent_receipt_delete() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'fixture failure'; END $$")
        .execute(&pool).await.unwrap();
    sqlx::query("CREATE TRIGGER reject_consent_receipt_delete BEFORE DELETE ON loops_webhook_receipts FOR EACH ROW EXECUTE FUNCTION reject_consent_receipt_delete()")
        .execute(&pool).await.unwrap();

    let handler = CleanupConsentWorkflowHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxNewsletterConfirmationChallengesRepository::new(),
        SqlxConsentWorkflowCleanup::new(),
        FixedClock(now),
    );
    assert!(handler.execute(1).await.is_err());
    assert!(
        !exists(
            &pool,
            "newsletter_subscription_confirmations",
            "confirmation_id",
            challenge
        )
        .await
    );
    let receipt_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM loops_webhook_receipts WHERE delivery_id = $1)",
    )
    .bind(&receipt)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(receipt_exists);

    sqlx::query("DROP TRIGGER reject_consent_receipt_delete ON loops_webhook_receipts")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DROP FUNCTION reject_consent_receipt_delete()")
        .execute(&pool)
        .await
        .unwrap();
    let retry = handler.execute(1).await.unwrap();
    assert_eq!(0, retry.unconfirmed_challenges_deleted);
    assert_eq!(1, retry.webhook_receipts_deleted);
}
