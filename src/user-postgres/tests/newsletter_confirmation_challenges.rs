use application::transaction::{Transaction, UnitOfWork};
use platform_postgres::SqlxUnitOfWork;
use serde_email::Email;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::{Duration, OffsetDateTime};
use user_core::marketing_consent_sync_intent_id::MarketingConsentSyncIntentId;
use user_core::newsletter_confirmation::RawNewsletterConfirmationToken;
use user_core::newsletter_confirmation_id::NewsletterConfirmationId;
use user_core::user_id::UserId;
use user_core::{first_name::FirstName, last_name::LastName};
use user_postgres::{
    SqlxMarketingConsentIntentRepository, SqlxMarketingConsentIntentWorker,
    SqlxNewsletterConfirmationChallengesRepository,
};
use user_service::ports::marketing_consent_intents::ConsentWorkerClaimOutcome;
use user_service::ports::{
    NewNewsletterConfirmationChallenge, NewsletterConfirmationChallenge,
    NewsletterConfirmationChallenges, NewsletterConfirmationChallengesFactory,
    NewsletterConfirmationClock, NewsletterConfirmationIssueOutcome,
    NewsletterConfirmationSendStatus,
};
use user_service::use_cases::commands::confirm_newsletter_subscription::{
    ConfirmNewsletterSubscriptionError, ConfirmNewsletterSubscriptionHandler,
    ConfirmNewsletterSubscriptionUseCase,
};
use user_service::use_cases::commands::coordinate_marketing_consent::MarketingConsentCoordinator;

const BUSINESS_SCHEMA: Postgres = Postgres::new_schema_once("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn email_only_confirmation_persists_profile_for_the_worker_claim() {
    let pool = get_postgres_client().await;
    let uow = SqlxUnitOfWork::new(pool.clone());
    let challenge_repo = SqlxNewsletterConfirmationChallengesRepository::new();
    let consent_repo = SqlxMarketingConsentIntentRepository::new();
    let target_email = email("doi-profile-snapshot@example.test");
    let now = OffsetDateTime::now_utc();
    let profile = user_service::ports::NewsletterProfile {
        first_name: Some(FirstName::from("Ada")),
        last_name: Some(LastName::from("Lovelace")),
        language: Some(localization::Language::En),
        currency: Some(money::Currency::Usd),
    };
    let (outcome, confirmation_id, token) = create_challenge_with_profile(
        &uow,
        &challenge_repo,
        &target_email,
        None,
        now,
        0x4f,
        profile.clone(),
    )
    .await;
    assert_eq!(NewsletterConfirmationIssueOutcome::Issued, outcome);
    let handler = ConfirmNewsletterSubscriptionHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        challenge_repo,
        consent_repo,
        FixedClock(now),
    );
    handler.execute(token.as_str()).await.unwrap();

    let intent_uuid: uuid::Uuid = sqlx::query_scalar(
        "SELECT resulting_intent_id FROM newsletter_subscription_confirmations WHERE confirmation_id = $1",
    )
    .bind(confirmation_id.into_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    let intent_id = MarketingConsentSyncIntentId::try_from(intent_uuid).unwrap();
    let stored_profile: serde_json::Value = sqlx::query_scalar(
        "SELECT profile_snapshot FROM marketing_email_consent_sync_intents WHERE intent_id = $1",
    )
    .bind(intent_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        serde_json::json!({
            "first_name": "Ada",
            "last_name": "Lovelace",
            "language": "en",
            "currency": "USD"
        }),
        stored_profile
    );

    let worker = SqlxMarketingConsentIntentWorker::new();
    let mut tx = uow.begin().await.unwrap();
    let claim = match worker.claim_by_id(&mut tx, intent_id).await.unwrap() {
        ConsentWorkerClaimOutcome::Claimed(claim) => claim,
        _ => panic!("new DOI intent should be claimable"),
    };
    assert_eq!(
        user_service::ports::ConsentSubject::EmailOnly,
        claim.intent.subject
    );
    assert_eq!(Some(Box::new(profile)), claim.intent.profile_snapshot);
    tx.commit().await.unwrap();
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn failed_confirmation_commit_rolls_back_grant_intent_and_challenge_updates() {
    let pool = get_postgres_client().await;
    let uow = SqlxUnitOfWork::new(pool.clone());
    let challenge_repo = SqlxNewsletterConfirmationChallengesRepository::new();
    let consent_repo = SqlxMarketingConsentIntentRepository::new();
    let target_id = UserId::new();
    let target_email = email("doi-rollback@example.test");
    seed_user(&pool, target_id, &target_email).await;
    let now = OffsetDateTime::now_utc();
    let (first, target_challenge_id, target_token) = create_challenge(
        &uow,
        &challenge_repo,
        &target_email,
        None,
        now - Duration::minutes(12),
        0x35,
    )
    .await;
    let (sibling, sibling_challenge_id, _) = create_challenge(
        &uow,
        &challenge_repo,
        &target_email,
        None,
        now - Duration::minutes(6),
        0x36,
    )
    .await;
    assert_eq!(NewsletterConfirmationIssueOutcome::Issued, first);
    assert_eq!(NewsletterConfirmationIssueOutcome::Issued, sibling);

    sqlx::query(
        "CREATE OR REPLACE FUNCTION reject_newsletter_confirmation_commit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.confirmed_at IS NOT NULL THEN RAISE EXCEPTION 'injected newsletter confirmation commit failure'; END IF; RETURN NEW; END $$",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "DROP TRIGGER IF EXISTS reject_newsletter_confirmation_commit ON newsletter_subscription_confirmations",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE CONSTRAINT TRIGGER reject_newsletter_confirmation_commit AFTER UPDATE ON newsletter_subscription_confirmations DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION reject_newsletter_confirmation_commit()",
    )
    .execute(&pool)
    .await
    .unwrap();

    let handler = ConfirmNewsletterSubscriptionHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        challenge_repo,
        consent_repo,
        FixedClock(now),
    );
    let result = handler.execute(target_token.as_str()).await;

    sqlx::query(
        "DROP TRIGGER IF EXISTS reject_newsletter_confirmation_commit ON newsletter_subscription_confirmations",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("DROP FUNCTION IF EXISTS reject_newsletter_confirmation_commit()")
        .execute(&pool)
        .await
        .unwrap();

    assert_eq!(
        Err(ConfirmNewsletterSubscriptionError::TemporarilyUnavailable),
        result
    );
    let user_consent: (bool, i64) = sqlx::query_as(
        "SELECT marketing_email_consent, marketing_email_consent_revision FROM users WHERE user_id = $1",
    )
    .bind(target_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((false, 0), user_consent);
    let intent_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM marketing_email_consent_sync_intents WHERE email = $1",
    )
    .bind::<&str>(target_email.as_ref())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(0, intent_count);
    for challenge_id in [target_challenge_id, sibling_challenge_id] {
        let state: (Option<OffsetDateTime>, Option<OffsetDateTime>) = sqlx::query_as(
            "SELECT confirmed_at, invalidated_at FROM newsletter_subscription_confirmations WHERE confirmation_id = $1",
        )
        .bind(challenge_id.into_uuid())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!((None, None), state);
    }
}

fn email(value: &str) -> Email {
    Email::try_from(value).unwrap()
}

#[derive(Clone, Copy)]
struct FixedClock(OffsetDateTime);

impl NewsletterConfirmationClock for FixedClock {
    fn now_utc(&self) -> OffsetDateTime {
        self.0
    }
}

async fn seed_user(pool: &sqlx::PgPool, id: UserId, email: &Email) {
    sqlx::query("INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'FREE', 'USER')")
        .bind(id.as_uuid())
        .bind::<&str>(email.as_ref())
        .execute(pool)
        .await
        .unwrap();
}

async fn create_challenge(
    uow: &SqlxUnitOfWork,
    repo: &SqlxNewsletterConfirmationChallengesRepository,
    email: &Email,
    requester: Option<UserId>,
    at: OffsetDateTime,
    entropy_byte: u8,
) -> (
    NewsletterConfirmationIssueOutcome,
    NewsletterConfirmationId,
    RawNewsletterConfirmationToken,
) {
    create_challenge_with_profile(
        uow,
        repo,
        email,
        requester,
        at,
        entropy_byte,
        user_service::ports::NewsletterProfile::default(),
    )
    .await
}

async fn create_challenge_with_profile(
    uow: &SqlxUnitOfWork,
    repo: &SqlxNewsletterConfirmationChallengesRepository,
    email: &Email,
    requester: Option<UserId>,
    at: OffsetDateTime,
    entropy_byte: u8,
    profile: user_service::ports::NewsletterProfile,
) -> (
    NewsletterConfirmationIssueOutcome,
    NewsletterConfirmationId,
    RawNewsletterConfirmationToken,
) {
    let token = RawNewsletterConfirmationToken::from_entropy([entropy_byte; 32]);
    let id = NewsletterConfirmationId::new();
    let mut tx = uow.begin().await.unwrap();
    let outcome = repo
        .in_transaction(&mut tx)
        .create_if_allowed(NewNewsletterConfirmationChallenge {
            id,
            token_digest: token.digest(),
            email: email.clone(),
            requested_by_user_id: requester,
            profile,
            now: at,
        })
        .await
        .unwrap();
    tx.commit().await.unwrap();
    (outcome, id, token)
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn issuance_limits_share_locked_budget_and_rejected_sends_still_count() {
    let pool = get_postgres_client().await;
    let uow = SqlxUnitOfWork::new(pool.clone());
    let repo = SqlxNewsletterConfirmationChallengesRepository::new();
    let target = email("doi-budget@example.test");
    let base = OffsetDateTime::now_utc() - Duration::hours(1);

    for (attempt, send_status, should_be_invalidated) in [
        (
            0,
            NewsletterConfirmationSendStatus::DefinitelyRejected,
            true,
        ),
        (1, NewsletterConfirmationSendStatus::Accepted, false),
        (
            2,
            NewsletterConfirmationSendStatus::AcceptanceUnknown,
            false,
        ),
    ] {
        let at = base
            + Duration::minutes(i64::from(attempt) * 5)
            + Duration::seconds(i64::from(attempt));
        let (outcome, id, token) =
            create_challenge(&uow, &repo, &target, None, at, 0x40 + attempt).await;
        assert_eq!(NewsletterConfirmationIssueOutcome::Issued, outcome);
        let mut tx = uow.begin().await.unwrap();
        repo.in_transaction(&mut tx)
            .record_send_outcome(id, send_status, at + Duration::seconds(1))
            .await
            .unwrap();
        tx.commit().await.unwrap();

        let invalidated: bool = sqlx::query_scalar(
            "SELECT invalidated_at IS NOT NULL FROM newsletter_subscription_confirmations WHERE confirmation_id = $1",
        )
        .bind(id.into_uuid())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(should_be_invalidated, invalidated);

        let stored_digest: Vec<u8> = sqlx::query_scalar(
            "SELECT token_digest FROM newsletter_subscription_confirmations WHERE confirmation_id = $1",
        )
        .bind(id.into_uuid())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            token.digest().as_bytes().as_slice(),
            stored_digest.as_slice()
        );
        let stored_columns: String = sqlx::query_scalar(
            "SELECT (to_jsonb(challenge) - 'token_digest')::text FROM newsletter_subscription_confirmations AS challenge WHERE confirmation_id = $1",
        )
        .bind(id.into_uuid())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(!stored_columns.contains(token.as_str()));
    }

    let (outcome, _, _) = create_challenge(
        &uow,
        &repo,
        &target,
        None,
        base + Duration::minutes(15) + Duration::seconds(3),
        0x43,
    )
    .await;
    assert_eq!(NewsletterConfirmationIssueOutcome::Suppressed, outcome);
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM newsletter_subscription_confirmations WHERE email = $1",
    )
    .bind::<&str>(target.as_ref())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(3, count);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn confirmation_binds_exact_target_and_replay_after_withdrawal_is_a_noop() {
    let pool = get_postgres_client().await;
    let uow = SqlxUnitOfWork::new(pool.clone());
    let challenge_repo = SqlxNewsletterConfirmationChallengesRepository::new();
    let consent_repo = SqlxMarketingConsentIntentRepository::new();
    let requester_id = UserId::new();
    let target_id = UserId::new();
    let requester_email = email("doi-requester-a@example.test");
    let target_email = email("doi-target-b@example.test");
    seed_user(&pool, requester_id, &requester_email).await;
    seed_user(&pool, target_id, &target_email).await;
    let now = OffsetDateTime::now_utc();
    let (outcome, _id, token) = create_challenge(
        &uow,
        &challenge_repo,
        &target_email,
        Some(requester_id),
        now,
        0x51,
    )
    .await;
    assert_eq!(NewsletterConfirmationIssueOutcome::Issued, outcome);

    let handler = ConfirmNewsletterSubscriptionHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        challenge_repo,
        consent_repo,
        FixedClock(now),
    );
    let (left, right) = tokio::join!(
        handler.execute(token.as_str()),
        handler.execute(token.as_str())
    );
    left.unwrap();
    right.unwrap();

    let state: (bool, bool) = sqlx::query_as(
        "SELECT (SELECT marketing_email_consent FROM users WHERE user_id = $1), (SELECT marketing_email_consent FROM users WHERE user_id = $2)",
    )
    .bind(requester_id.as_uuid())
    .bind(target_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((false, true), state);

    let mut read_tx = uow.begin().await.unwrap();
    let challenge: NewsletterConfirmationChallenge = challenge_repo
        .in_transaction(&mut read_tx)
        .find_by_token_digest(token.digest())
        .await
        .unwrap()
        .unwrap();
    read_tx.commit().await.unwrap();
    assert_eq!(Some(requester_id), challenge.requested_by_user_id);
    assert_eq!(Some(target_id), challenge.bound_user_id);
    let confirmed_intent = challenge.resulting_intent_id.unwrap();
    assert_eq!("nsc", NewsletterConfirmationId::PREFIX);
    assert!(confirmed_intent.to_string().starts_with("mci_"));

    let mut tx = uow.begin().await.unwrap();
    MarketingConsentCoordinator::new(&mut tx, &consent_repo)
        .provider_withdrawal(target_email.clone(), now + Duration::seconds(1))
        .await
        .unwrap();
    tx.commit().await.unwrap();

    handler.execute(token.as_str()).await.unwrap();
    let state: (bool, i64) = sqlx::query_as(
        "SELECT marketing_email_consent, marketing_email_consent_revision FROM users WHERE user_id = $1",
    )
    .bind(target_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((false, 2), state);
    let intents: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM marketing_email_consent_sync_intents WHERE email = $1",
    )
    .bind::<&str>(target_email.as_ref())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(1, intents);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn expiry_boundary_and_provider_withdrawal_reject_unconfirmed_proofs_without_consent() {
    let pool = get_postgres_client().await;
    let uow = SqlxUnitOfWork::new(pool.clone());
    let challenge_repo = SqlxNewsletterConfirmationChallengesRepository::new();
    let consent_repo = SqlxMarketingConsentIntentRepository::new();
    let target_id = UserId::new();
    let target_email = email("doi-expired@example.test");
    seed_user(&pool, target_id, &target_email).await;
    let created_at = OffsetDateTime::now_utc();
    let (outcome, _, expired_token) =
        create_challenge(&uow, &challenge_repo, &target_email, None, created_at, 0x61).await;
    assert_eq!(NewsletterConfirmationIssueOutcome::Issued, outcome);
    let handler = ConfirmNewsletterSubscriptionHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        challenge_repo,
        consent_repo,
        FixedClock(created_at + Duration::hours(24)),
    );
    assert_eq!(
        Err(ConfirmNewsletterSubscriptionError::InvalidConfirmation),
        handler.execute(expired_token.as_str()).await
    );

    let pending_email = email("doi-provider-withdrawal@example.test");
    let (outcome, _, pending_token) = create_challenge(
        &uow,
        &challenge_repo,
        &pending_email,
        None,
        created_at,
        0x62,
    )
    .await;
    assert_eq!(NewsletterConfirmationIssueOutcome::Issued, outcome);
    let mut tx = uow.begin().await.unwrap();
    MarketingConsentCoordinator::new(&mut tx, &consent_repo)
        .provider_withdrawal(pending_email, created_at)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let handler = ConfirmNewsletterSubscriptionHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        challenge_repo,
        consent_repo,
        FixedClock(created_at + Duration::seconds(1)),
    );
    assert_eq!(
        Err(ConfirmNewsletterSubscriptionError::InvalidConfirmation),
        handler.execute(pending_token.as_str()).await
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn sibling_links_cannot_create_independent_email_only_grants() {
    let pool = get_postgres_client().await;
    let uow = SqlxUnitOfWork::new(pool.clone());
    let challenge_repo = SqlxNewsletterConfirmationChallengesRepository::new();
    let consent_repo = SqlxMarketingConsentIntentRepository::new();
    let target_email = email("doi-siblings@example.test");
    let now = OffsetDateTime::now_utc();
    let (first_outcome, first_id, first_token) = create_challenge(
        &uow,
        &challenge_repo,
        &target_email,
        None,
        now - Duration::minutes(10) - Duration::seconds(2),
        0x71,
    )
    .await;
    let (second_outcome, second_id, second_token) = create_challenge(
        &uow,
        &challenge_repo,
        &target_email,
        None,
        now - Duration::minutes(5) - Duration::seconds(1),
        0x72,
    )
    .await;
    assert_eq!(NewsletterConfirmationIssueOutcome::Issued, first_outcome);
    assert_eq!(NewsletterConfirmationIssueOutcome::Issued, second_outcome);

    let handler = ConfirmNewsletterSubscriptionHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        challenge_repo,
        consent_repo,
        FixedClock(now),
    );
    handler.execute(first_token.as_str()).await.unwrap();
    assert_eq!(
        Err(ConfirmNewsletterSubscriptionError::InvalidConfirmation),
        handler.execute(second_token.as_str()).await
    );

    let states: Vec<(Option<OffsetDateTime>, Option<OffsetDateTime>)> = sqlx::query_as(
        "SELECT confirmed_at, invalidated_at FROM newsletter_subscription_confirmations WHERE confirmation_id = ANY($1) ORDER BY confirmation_id",
    )
    .bind(vec![first_id.into_uuid(), second_id.into_uuid()])
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(2, states.len());
    assert_eq!(
        1,
        states
            .iter()
            .filter(|(confirmed, _)| confirmed.is_some())
            .count()
    );
    assert_eq!(
        1,
        states
            .iter()
            .filter(|(_, invalidated)| invalidated.is_some())
            .count()
    );
    let intents: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM marketing_email_consent_sync_intents WHERE email = $1 AND desired",
    )
    .bind::<&str>(target_email.as_ref())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(1, intents);
    let users: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE email = $1")
        .bind::<&str>(target_email.as_ref())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(0, users);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn email_only_proof_binds_exact_existing_user_but_deleted_binding_never_becomes_anonymous() {
    let pool = get_postgres_client().await;
    let uow = SqlxUnitOfWork::new(pool.clone());
    let challenge_repo = SqlxNewsletterConfirmationChallengesRepository::new();
    let consent_repo = SqlxMarketingConsentIntentRepository::new();
    let now = OffsetDateTime::now_utc();

    let late_user_id = UserId::new();
    let late_user_email = email("doi-late-user@example.test");
    let (outcome, _, token) =
        create_challenge(&uow, &challenge_repo, &late_user_email, None, now, 0x73).await;
    assert_eq!(NewsletterConfirmationIssueOutcome::Issued, outcome);
    seed_user(&pool, late_user_id, &late_user_email).await;
    let handler = ConfirmNewsletterSubscriptionHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        challenge_repo,
        consent_repo,
        FixedClock(now),
    );
    handler.execute(token.as_str()).await.unwrap();
    let (bound_user, consent): (Option<uuid::Uuid>, bool) = sqlx::query_as(
        "SELECT bound_user_id, (SELECT marketing_email_consent FROM users WHERE user_id = $2) FROM newsletter_subscription_confirmations WHERE token_digest = $1",
    )
    .bind(token.digest().as_bytes().as_slice())
    .bind(late_user_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(Some(late_user_id.into_uuid()), bound_user);
    assert!(consent);

    let deleted_user_id = UserId::new();
    let deleted_email = email("doi-deleted-user@example.test");
    seed_user(&pool, deleted_user_id, &deleted_email).await;
    let (outcome, deleted_challenge_id, deleted_token) =
        create_challenge(&uow, &challenge_repo, &deleted_email, None, now, 0x74).await;
    assert_eq!(NewsletterConfirmationIssueOutcome::Issued, outcome);
    sqlx::query("DELETE FROM users WHERE user_id = $1")
        .bind(deleted_user_id.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        Err(ConfirmNewsletterSubscriptionError::InvalidConfirmation),
        handler.execute(deleted_token.as_str()).await
    );
    let invalidated_at: Option<OffsetDateTime> = sqlx::query_scalar(
        "SELECT invalidated_at FROM newsletter_subscription_confirmations WHERE confirmation_id = $1",
    )
    .bind(deleted_challenge_id.into_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(invalidated_at.is_some());
    let intents: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM marketing_email_consent_sync_intents WHERE email = $1",
    )
    .bind::<&str>(deleted_email.as_ref())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(0, intents);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn concurrent_issuance_is_suppressed_by_the_shared_recipient_lock() {
    let pool = get_postgres_client().await;
    let uow = SqlxUnitOfWork::new(pool.clone());
    let repo = SqlxNewsletterConfirmationChallengesRepository::new();
    let target = email("doi-concurrent-request@example.test");
    let now = OffsetDateTime::now_utc();
    let (first, second) = tokio::join!(
        create_challenge(&uow, &repo, &target, None, now, 0x75),
        create_challenge(&uow, &repo, &target, None, now, 0x76),
    );
    let outcomes = [first.0, second.0];
    assert_eq!(
        1,
        outcomes
            .iter()
            .filter(|outcome| **outcome == NewsletterConfirmationIssueOutcome::Issued)
            .count()
    );
    assert_eq!(
        1,
        outcomes
            .iter()
            .filter(|outcome| **outcome == NewsletterConfirmationIssueOutcome::Suppressed)
            .count()
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM newsletter_subscription_confirmations WHERE email = $1",
    )
    .bind::<&str>(target.as_ref())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(1, count);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn provider_withdrawal_racing_confirmation_commits_one_coherent_mailbox_decision() {
    let pool = get_postgres_client().await;
    let target_id = UserId::new();
    let target_email = email("doi-withdrawal-race@example.test");
    seed_user(&pool, target_id, &target_email).await;
    let uow = SqlxUnitOfWork::new(pool.clone());
    let challenge_repo = SqlxNewsletterConfirmationChallengesRepository::new();
    let consent_repo = SqlxMarketingConsentIntentRepository::new();
    let now = OffsetDateTime::now_utc();
    let (outcome, challenge_id, token) = create_challenge(
        &uow,
        &challenge_repo,
        &target_email,
        None,
        now - Duration::minutes(1),
        0x7a,
    )
    .await;
    assert_eq!(NewsletterConfirmationIssueOutcome::Issued, outcome);
    let handler = ConfirmNewsletterSubscriptionHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        challenge_repo,
        consent_repo,
        FixedClock(now),
    );
    let withdrawal = async {
        let mut tx = uow.begin().await.unwrap();
        MarketingConsentCoordinator::new(&mut tx, &consent_repo)
            .provider_withdrawal(target_email.clone(), now)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    };
    let (confirmation_result, ()) = tokio::join!(handler.execute(token.as_str()), withdrawal);
    assert!(matches!(
        confirmation_result,
        Ok(()) | Err(ConfirmNewsletterSubscriptionError::InvalidConfirmation)
    ));

    let state: (bool, i64) = sqlx::query_as(
        "SELECT marketing_email_consent, marketing_email_consent_revision FROM users WHERE user_id = $1",
    )
    .bind(target_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!state.0);

    let challenge_state: (Option<OffsetDateTime>, Option<OffsetDateTime>) = sqlx::query_as(
        "SELECT confirmed_at, invalidated_at FROM newsletter_subscription_confirmations WHERE confirmation_id = $1",
    )
    .bind(challenge_id.into_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_ne!(challenge_state.0.is_some(), challenge_state.1.is_some());
    let desired_intents: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM marketing_email_consent_sync_intents WHERE email = $1 AND desired",
    )
    .bind::<&str>(target_email.as_ref())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(i64::from(challenge_state.0.is_some()), desired_intents);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn unknown_malformed_and_mismatched_user_proofs_fail_without_granting_consent() {
    let pool = get_postgres_client().await;
    let uow = SqlxUnitOfWork::new(pool.clone());
    let challenge_repo = SqlxNewsletterConfirmationChallengesRepository::new();
    let consent_repo = SqlxMarketingConsentIntentRepository::new();
    let now = OffsetDateTime::now_utc();
    let handler = ConfirmNewsletterSubscriptionHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        challenge_repo,
        consent_repo,
        FixedClock(now),
    );
    let malformed = handler.execute("malformed-token").await;
    let unknown = RawNewsletterConfirmationToken::from_entropy([0x7b; 32]);
    let unknown_result = handler.execute(unknown.as_str()).await;
    assert_eq!(
        Err(ConfirmNewsletterSubscriptionError::InvalidConfirmation),
        malformed
    );
    assert_eq!(
        Err(ConfirmNewsletterSubscriptionError::InvalidConfirmation),
        unknown_result
    );

    let original_user_id = UserId::new();
    let replacement_user_id = UserId::new();
    let target_email = email("doi-corrupt-binding@example.test");
    let replacement_email = email("doi-different-user@example.test");
    seed_user(&pool, original_user_id, &target_email).await;
    seed_user(&pool, replacement_user_id, &replacement_email).await;
    let (outcome, challenge_id, token) =
        create_challenge(&uow, &challenge_repo, &target_email, None, now, 0x7c).await;
    assert_eq!(NewsletterConfirmationIssueOutcome::Issued, outcome);
    sqlx::query(
        "UPDATE newsletter_subscription_confirmations SET bound_user_id = $2 WHERE confirmation_id = $1",
    )
    .bind(challenge_id.into_uuid())
    .bind(replacement_user_id.as_uuid())
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        Err(ConfirmNewsletterSubscriptionError::InvalidPersistedState),
        handler.execute(token.as_str()).await
    );
    let consent_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM marketing_email_consent_sync_intents WHERE email IN ($1, $2)",
    )
    .bind::<&str>(target_email.as_ref())
    .bind::<&str>(replacement_email.as_ref())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(0, consent_count);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn expired_cleanup_is_bounded_and_retains_confirmed_replay_rows() {
    let pool = get_postgres_client().await;
    let uow = SqlxUnitOfWork::new(pool.clone());
    let challenge_repo = SqlxNewsletterConfirmationChallengesRepository::new();
    let consent_repo = SqlxMarketingConsentIntentRepository::new();
    let now = OffsetDateTime::now_utc();
    let created_at = now - Duration::hours(26);
    let retained_email = email("doi-cleanup-confirmed@example.test");
    let (_, retained_id, retained_token) = create_challenge(
        &uow,
        &challenge_repo,
        &retained_email,
        None,
        created_at,
        0x77,
    )
    .await;
    let confirmed_handler = ConfirmNewsletterSubscriptionHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        challenge_repo,
        consent_repo,
        FixedClock(created_at + Duration::hours(1)),
    );
    confirmed_handler
        .execute(retained_token.as_str())
        .await
        .unwrap();

    let expired_email = email("doi-cleanup-expired-a@example.test");
    let (_, _, _) = create_challenge(
        &uow,
        &challenge_repo,
        &expired_email,
        None,
        created_at,
        0x78,
    )
    .await;
    let second_expired_email = email("doi-cleanup-expired-b@example.test");
    let (_, _, _) = create_challenge(
        &uow,
        &challenge_repo,
        &second_expired_email,
        None,
        created_at,
        0x79,
    )
    .await;
    let mut tx = uow.begin().await.unwrap();
    let removed = challenge_repo
        .in_transaction(&mut tx)
        .cleanup_expired(now, 1)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(1, removed);
    let retained: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM newsletter_subscription_confirmations WHERE confirmation_id = $1)",
    )
    .bind(retained_id.into_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(retained);
    let unconfirmed_expired: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM newsletter_subscription_confirmations WHERE confirmed_at IS NULL AND expires_at <= $1",
    )
    .bind(now)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(1, unconfirmed_expired);

    let mut tx = uow.begin().await.unwrap();
    let removed = challenge_repo
        .in_transaction(&mut tx)
        .cleanup_expired(now, 1)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(1, removed);
    let unconfirmed_expired: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM newsletter_subscription_confirmations WHERE confirmed_at IS NULL AND expires_at <= $1",
    )
    .bind(now)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(0, unconfirmed_expired);
}
