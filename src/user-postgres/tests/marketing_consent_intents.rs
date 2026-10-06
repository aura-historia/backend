use application::transaction::{Transaction, UnitOfWork};
use platform_postgres::SqlxUnitOfWork;
use serde_email::Email;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::{Duration, OffsetDateTime};
use user_core::{marketing_consent_sync_intent_id::MarketingConsentSyncIntentId, user_id::UserId};
use user_postgres::{
    ConsentIntentFinalization, ConsentIntentSource, ConsentIntentStatus, ConsentSubject,
    MarketingConsentPersistenceError, SqlxMarketingConsentIntentRepository,
    SqlxMarketingConsentIntentWorker, SqlxUserRepositoryFactory,
};
use user_service::ports::{
    ConsentIntentSource as PortSource, MarketingConsentIntentError, MarketingConsentIntents,
    MarketingConsentIntentsFactory, UserRepository, UserRepositoryFactory, UserStorageVersion,
};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
fn email(value: &str) -> Email {
    Email::try_from(value).unwrap()
}
async fn seed(pool: &sqlx::PgPool, id: UserId, email: &Email) {
    sqlx::query("INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'FREE', 'USER')")
        .bind(id.as_uuid())
        .bind::<&str>(email.as_ref())
        .execute(pool)
        .await
        .unwrap();
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn verified_user_proofs_are_atomic_replay_safe_and_revision_fenced() {
    let pool = get_postgres_client().await;
    let id = UserId::new();
    let address = email("confirmed@example.test");
    seed(&pool, id, &address).await;
    let uow = SqlxUnitOfWork::new(pool.clone());
    let repo = SqlxMarketingConsentIntentRepository::new();
    let worker = SqlxMarketingConsentIntentWorker::new();
    let at = OffsetDateTime::now_utc();
    let mut tx = uow.begin().await.unwrap();
    let grant = repo
        .record_user_transition(
            &mut tx,
            id,
            UserStorageVersion::INITIAL,
            &address,
            true,
            ConsentIntentSource::CognitoSignup,
            "signup:proof",
            at,
        )
        .await
        .unwrap();
    assert_eq!(ConsentSubject::User(id), grant.subject);
    assert_eq!(ConsentIntentSource::CognitoSignup, grant.source);
    assert_eq!(Some(1), grant.consent_revision);
    assert!(
        (grant.not_after.unwrap() - (at + Duration::days(7))).abs() < Duration::milliseconds(1)
    );
    assert!(grant.intent_id.to_string().starts_with("mci_"));
    assert_eq!(64, grant.recipient_key.len());
    assert_eq!(
        grant.intent_id,
        MarketingConsentSyncIntentId::try_from(*grant.intent_id.as_uuid()).unwrap()
    );
    assert_eq!(
        grant.intent_id,
        repo.record_user_transition(
            &mut tx,
            id,
            UserStorageVersion::INITIAL,
            &address,
            true,
            ConsentIntentSource::CognitoSignup,
            "signup:proof",
            at
        )
        .await
        .unwrap()
        .intent_id
    );
    assert!(matches!(
        repo.record_user_transition(
            &mut tx,
            id,
            UserStorageVersion::INITIAL,
            &address,
            false,
            ConsentIntentSource::UserWithdrawal,
            "signup:proof",
            at
        )
        .await,
        Err(MarketingConsentPersistenceError::SourceKeyConflict)
    ));
    assert!(matches!(
        repo.record_user_transition(
            &mut tx,
            id,
            UserStorageVersion::INITIAL,
            &email("wrong@example.test"),
            true,
            ConsentIntentSource::AuraDoubleOptIn,
            "wrong-proof",
            at
        )
        .await,
        Err(MarketingConsentPersistenceError::ConcurrencyConflict)
    ));
    tx.commit().await.unwrap();
    let mut tx = uow.begin().await.unwrap();
    let claim = worker.claim_next(&mut tx).await.unwrap().unwrap();
    tx.commit().await.unwrap();
    let mut tx = uow.begin().await.unwrap();
    let newer = repo
        .record_user_transition(
            &mut tx,
            id,
            UserStorageVersion::try_from(2_i64).unwrap(),
            &address,
            true,
            ConsentIntentSource::AuraDoubleOptIn,
            "new-proof",
            OffsetDateTime::now_utc(),
        )
        .await
        .unwrap();
    assert_eq!(Some(2), newer.consent_revision);
    tx.commit().await.unwrap();
    let mut tx = uow.begin().await.unwrap();
    assert!(worker.read_claim(&mut tx, &claim).await.unwrap().is_none());
    assert!(
        !worker
            .finalize(
                &mut tx,
                &claim,
                ConsentIntentFinalization::Applied {
                    provider_contact_id: None
                },
                OffsetDateTime::now_utc()
            )
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    let state: (bool, i64, i64) = sqlx::query_as("SELECT marketing_email_consent, marketing_email_consent_revision, version FROM users WHERE user_id = $1")
        .bind(id.as_uuid()).fetch_one(&pool).await.unwrap();
    assert_eq!((true, 2, 3), state);
    assert_eq!(
        "SUPERSEDED",
        sqlx::query_scalar::<_, String>(
            "SELECT status FROM marketing_email_consent_sync_intents WHERE intent_id = $1"
        )
        .bind(grant.intent_id.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap()
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn email_only_lease_receipt_and_provider_backsync_cancellation() {
    let pool = get_postgres_client().await;
    let uow = SqlxUnitOfWork::new(pool.clone());
    let repo = SqlxMarketingConsentIntentRepository::new();
    let worker = SqlxMarketingConsentIntentWorker::new();
    let address = email("Anonymous@EXAMPLE.test");
    let mut tx = uow.begin().await.unwrap();
    let grant = repo
        .record_email_only_intent(
            &mut tx,
            &address,
            true,
            ConsentIntentSource::AuraDoubleOptIn,
            "double-opt-in",
            OffsetDateTime::now_utc(),
        )
        .await
        .unwrap();
    assert_eq!(ConsentSubject::EmailOnly, grant.subject);
    tx.commit().await.unwrap();
    let mut tx = uow.begin().await.unwrap();
    let first = worker.claim_next(&mut tx).await.unwrap().unwrap();
    assert_eq!(1, first.attempt_count);
    tx.commit().await.unwrap();
    sqlx::query("UPDATE marketing_email_consent_sync_intents SET lease_expires_at = clock_timestamp() - interval '1 second' WHERE intent_id = $1")
        .bind(grant.intent_id.as_uuid()).execute(&pool).await.unwrap();
    let mut tx = uow.begin().await.unwrap();
    assert!(
        !worker
            .finalize(
                &mut tx,
                &first,
                ConsentIntentFinalization::Applied {
                    provider_contact_id: None
                },
                first.lease_expires_at - Duration::seconds(1)
            )
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    assert_eq!(
        "IN_PROGRESS",
        sqlx::query_scalar::<_, String>(
            "SELECT status FROM marketing_email_consent_sync_intents WHERE intent_id = $1"
        )
        .bind(grant.intent_id.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap()
    );
    let mut tx = uow.begin().await.unwrap();
    let next = worker.claim_next(&mut tx).await.unwrap().unwrap();
    assert_ne!(first.lease_token, next.lease_token);
    assert_eq!(2, next.attempt_count);
    assert!(worker.read_claim(&mut tx, &first).await.unwrap().is_none());
    assert!(
        !worker
            .finalize(
                &mut tx,
                &first,
                ConsentIntentFinalization::Applied {
                    provider_contact_id: None
                },
                OffsetDateTime::now_utc()
            )
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    let mut tx = uow.begin().await.unwrap();
    assert_eq!(
        1,
        repo.cancel_provider_backsync(&mut tx, &address)
            .await
            .unwrap()
    );
    assert_eq!(
        0,
        repo.cancel_provider_backsync(&mut tx, &address)
            .await
            .unwrap()
    );
    assert!(worker.read_claim(&mut tx, &next).await.unwrap().is_none());
    assert!(
        !worker
            .finalize(
                &mut tx,
                &next,
                ConsentIntentFinalization::Applied {
                    provider_contact_id: None
                },
                OffsetDateTime::now_utc()
            )
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    let mut tx = uow.begin().await.unwrap();
    let revoke = repo
        .record_email_only_intent(
            &mut tx,
            &address,
            false,
            ConsentIntentSource::EmailOnlyWithdrawal,
            "anon-withdraw",
            OffsetDateTime::now_utc(),
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mut tx = uow.begin().await.unwrap();
    let claim = worker.claim_next(&mut tx).await.unwrap().unwrap();
    assert_eq!(revoke.intent_id, claim.intent.intent_id);
    tx.commit().await.unwrap();
    let completed_at = OffsetDateTime::now_utc();
    let applied = ConsentIntentFinalization::Applied {
        provider_contact_id: Some("contact-1"),
    };
    let mut tx = uow.begin().await.unwrap();
    assert!(
        worker
            .finalize(&mut tx, &claim, applied, completed_at)
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    let mut tx = uow.begin().await.unwrap();
    assert!(
        worker
            .finalize(&mut tx, &claim, applied, completed_at)
            .await
            .unwrap()
    );
    assert!(
        !worker
            .finalize(
                &mut tx,
                &claim,
                ConsentIntentFinalization::Applied {
                    provider_contact_id: None
                },
                completed_at
            )
            .await
            .unwrap()
    );
    let mut wrong_email = claim.clone();
    wrong_email.intent.email = email("anonymous@example.test");
    assert_ne!(wrong_email.intent.email, claim.intent.email);
    assert!(
        !worker
            .finalize(&mut tx, &wrong_email, applied, completed_at)
            .await
            .unwrap()
    );
    assert_eq!(
        ConsentIntentStatus::Applied,
        repo.find_by_source_key(&mut tx, "anon-withdraw")
            .await
            .unwrap()
            .unwrap()
            .status
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn service_port_withdrawal_updates_registered_user_and_cancels_only_exact_email() {
    let pool = get_postgres_client().await;
    let uow = SqlxUnitOfWork::new(pool.clone());
    let factory = SqlxMarketingConsentIntentRepository::new();
    let worker = SqlxMarketingConsentIntentWorker::new();
    let id = UserId::new();
    let address = email("Ada@example.test");
    let other_case = email("ada@example.test");
    assert_ne!(address, other_case);
    seed(&pool, id, &address).await;
    let mut tx = uow.begin().await.unwrap();
    let mut port = factory.in_transaction(&mut tx);
    assert!(
        port.find_user_by_email(&other_case)
            .await
            .unwrap()
            .is_none()
    );
    let user = port.find_user_by_id(id).await.unwrap().unwrap();
    let grant = port
        .record_user_transition(
            &user,
            true,
            PortSource::CognitoSignup,
            "signup:ada",
            OffsetDateTime::now_utc(),
        )
        .await
        .unwrap();
    assert_eq!(
        grant.intent_id,
        port.find_by_source_key("signup:ada")
            .await
            .unwrap()
            .unwrap()
            .intent_id
    );
    drop(port);
    tx.commit().await.unwrap();

    let mut tx = uow.begin().await.unwrap();
    let mut port = factory.in_transaction(&mut tx);
    port.cancel_provider_backsync(&other_case).await.unwrap();
    let user = port.find_user_by_email(&address).await.unwrap().unwrap();
    assert!(matches!(
        port.record_user_transition(
            &user,
            true,
            PortSource::EmailOnlyWithdrawal,
            "bad-source",
            OffsetDateTime::now_utc()
        )
        .await,
        Err(MarketingConsentIntentError::InvalidInput)
    ));
    assert!(matches!(
        port.record_user_transition(
            &user,
            true,
            PortSource::AuraDoubleOptIn,
            "signup:ada",
            OffsetDateTime::now_utc()
        )
        .await,
        Err(MarketingConsentIntentError::SourceKeyConflict)
    ));
    port.apply_provider_withdrawal(&user, OffsetDateTime::now_utc())
        .await
        .unwrap();
    port.cancel_provider_backsync(&address).await.unwrap();
    drop(port);
    tx.commit().await.unwrap();
    let state: (bool, i64, i64) = sqlx::query_as("SELECT marketing_email_consent, marketing_email_consent_revision, version FROM users WHERE user_id = $1")
        .bind(id.as_uuid()).fetch_one(&pool).await.unwrap();
    assert_eq!((false, 2, 3), state);
    assert_eq!(
        1,
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM marketing_email_consent_sync_intents")
            .fetch_one(&pool)
            .await
            .unwrap()
    );
    let mut tx = uow.begin().await.unwrap();
    assert!(worker.claim_next(&mut tx).await.unwrap().is_none());
    assert_eq!(
        ConsentIntentStatus::Blocked,
        factory
            .find_by_source_key(&mut tx, "signup:ada")
            .await
            .unwrap()
            .unwrap()
            .status
    );
    drop(tx);
    let mut tx = uow.begin().await.unwrap();
    let mut port = factory.in_transaction(&mut tx);
    assert!(matches!(
        port.apply_provider_withdrawal(&user, OffsetDateTime::now_utc())
            .await,
        Err(MarketingConsentIntentError::ConcurrencyConflict)
    ));
    let current = port.find_user_by_email(&address).await.unwrap().unwrap();
    port.apply_provider_withdrawal(&current, OffsetDateTime::now_utc())
        .await
        .unwrap();
    drop(port);
    tx.commit().await.unwrap();
    let unchanged: (i64, i64) = sqlx::query_as(
        "SELECT marketing_email_consent_revision, version FROM users WHERE user_id = $1",
    )
    .bind(id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((2, 3), unchanged);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn case_folded_lock_does_not_merge_distinct_exact_provider_addresses() {
    let pool = get_postgres_client().await;
    let uow = SqlxUnitOfWork::new(pool.clone());
    let repo = SqlxMarketingConsentIntentRepository::new();
    let worker = SqlxMarketingConsentIntentWorker::new();
    let first_email = email("Case@example.test");
    let second_email = email("case@example.test");
    let mut tx = uow.begin().await.unwrap();
    let first = repo
        .record_email_only_intent(
            &mut tx,
            &first_email,
            true,
            ConsentIntentSource::AuraDoubleOptIn,
            "first-case",
            OffsetDateTime::now_utc(),
        )
        .await
        .unwrap();
    let second = repo
        .record_email_only_intent(
            &mut tx,
            &second_email,
            true,
            ConsentIntentSource::AuraDoubleOptIn,
            "second-case",
            OffsetDateTime::now_utc(),
        )
        .await
        .unwrap();
    assert_eq!(first.recipient_key, second.recipient_key);
    assert_ne!(first.email, second.email);
    tx.commit().await.unwrap();
    let mut tx = uow.begin().await.unwrap();
    let first_claim = worker.claim_next(&mut tx).await.unwrap().unwrap();
    assert_eq!(first.intent_id, first_claim.intent.intent_id);
    tx.commit().await.unwrap();
    let mut tx = uow.begin().await.unwrap();
    assert_eq!(
        1,
        repo.cancel_provider_backsync(&mut tx, &second_email)
            .await
            .unwrap()
    );
    assert!(
        worker
            .read_claim(&mut tx, &first_claim)
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(
        ConsentIntentStatus::Blocked,
        repo.find_by_source_key(&mut tx, "second-case")
            .await
            .unwrap()
            .unwrap()
            .status
    );
    tx.commit().await.unwrap();
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn provider_withdrawal_rolls_back_both_user_change_and_cancellation() {
    let pool = get_postgres_client().await;
    let uow = SqlxUnitOfWork::new(pool.clone());
    let factory = SqlxMarketingConsentIntentRepository::new();
    let address = email("rollback-provider@example.test");
    let id = UserId::new();
    seed(&pool, id, &address).await;
    let mut tx = uow.begin().await.unwrap();
    let mut port = factory.in_transaction(&mut tx);
    let user = port.find_user_by_id(id).await.unwrap().unwrap();
    port.record_user_transition(
        &user,
        true,
        PortSource::CognitoSignup,
        "grant-for-rollback",
        OffsetDateTime::now_utc(),
    )
    .await
    .unwrap();
    drop(port);
    tx.commit().await.unwrap();
    let mut tx = uow.begin().await.unwrap();
    let mut port = factory.in_transaction(&mut tx);
    let user = port.find_user_by_email(&address).await.unwrap().unwrap();
    port.apply_provider_withdrawal(&user, OffsetDateTime::now_utc())
        .await
        .unwrap();
    port.cancel_provider_backsync(&address).await.unwrap();
    drop(port);
    drop(tx);
    assert!(
        sqlx::query_scalar::<_, bool>(
            "SELECT marketing_email_consent FROM users WHERE user_id = $1"
        )
        .bind(id.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap()
    );
    let mut tx = uow.begin().await.unwrap();
    assert_eq!(
        ConsentIntentStatus::Pending,
        factory
            .find_by_source_key(&mut tx, "grant-for-rollback")
            .await
            .unwrap()
            .unwrap()
            .status
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn deletion_cancels_user_grant_and_preserves_user_revoke() {
    let pool = get_postgres_client().await;
    let uow = SqlxUnitOfWork::new(pool.clone());
    let repo = SqlxMarketingConsentIntentRepository::new();
    let worker = SqlxMarketingConsentIntentWorker::new();
    let id = UserId::new();
    let address = email("deleted@example.test");
    seed(&pool, id, &address).await;
    let mut tx = uow.begin().await.unwrap();
    let grant = repo
        .record_user_transition(
            &mut tx,
            id,
            UserStorageVersion::INITIAL,
            &address,
            true,
            ConsentIntentSource::CognitoSignup,
            "signup:deleted",
            OffsetDateTime::now_utc(),
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mut tx = uow.begin().await.unwrap();
    let grant_claim = worker.claim_next(&mut tx).await.unwrap().unwrap();
    assert_eq!(grant.intent_id, grant_claim.intent.intent_id);
    tx.commit().await.unwrap();

    let mut tx = uow.begin().await.unwrap();
    let revoke = repo
        .record_user_deletion(
            &mut tx,
            id,
            UserStorageVersion::try_from(2_i64).unwrap(),
            &address,
            "delete:account",
            OffsetDateTime::now_utc(),
        )
        .await
        .unwrap();
    assert_eq!(ConsentSubject::User(id), revoke.subject);
    assert_eq!(ConsentIntentSource::UserDeletion, revoke.source);
    assert_eq!(address, revoke.email);
    assert!(!revoke.desired);
    assert_eq!(Some(2), revoke.consent_revision);
    tx.commit().await.unwrap();
    assert!(
        !sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM users WHERE user_id = $1)")
            .bind(id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap()
    );
    let mut tx = uow.begin().await.unwrap();
    assert!(
        worker
            .read_claim(&mut tx, &grant_claim)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        !worker
            .finalize(
                &mut tx,
                &grant_claim,
                ConsentIntentFinalization::Applied {
                    provider_contact_id: None
                },
                OffsetDateTime::now_utc()
            )
            .await
            .unwrap()
    );
    assert_eq!(
        revoke.intent_id,
        repo.record_user_deletion(
            &mut tx,
            id,
            UserStorageVersion::INITIAL,
            &address,
            "delete:account",
            OffsetDateTime::now_utc()
        )
        .await
        .unwrap()
        .intent_id
    );
    tx.commit().await.unwrap();
    let mut tx = uow.begin().await.unwrap();
    let revoke_claim = worker.claim_next(&mut tx).await.unwrap().unwrap();
    assert_eq!(revoke.intent_id, revoke_claim.intent.intent_id);
    assert!(
        worker
            .read_claim(&mut tx, &revoke_claim)
            .await
            .unwrap()
            .is_some()
    );
    tx.commit().await.unwrap();
    let mut tx = uow.begin().await.unwrap();
    assert!(
        worker
            .finalize(
                &mut tx,
                &revoke_claim,
                ConsentIntentFinalization::Applied {
                    provider_contact_id: None
                },
                OffsetDateTime::now_utc()
            )
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    assert_eq!(
        "APPLIED",
        sqlx::query_scalar::<_, String>(
            "SELECT status FROM marketing_email_consent_sync_intents WHERE intent_id = $1"
        )
        .bind(revoke.intent_id.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap()
    );
    assert_eq!(1, sqlx::query_scalar::<_, i64>("SELECT count(*) FROM marketing_email_consent_sync_intents WHERE user_id = $1 AND NOT desired")
        .bind(id.as_uuid()).fetch_one(&pool).await.unwrap());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn deleted_address_revoke_cannot_withdraw_a_new_owners_consent() {
    let pool = get_postgres_client().await;
    let uow = SqlxUnitOfWork::new(pool.clone());
    let repo = SqlxMarketingConsentIntentRepository::new();
    let worker = SqlxMarketingConsentIntentWorker::new();
    let previous = UserId::new();
    let address = email("reassigned@example.test");
    seed(&pool, previous, &address).await;
    let mut tx = uow.begin().await.unwrap();
    let revoke = repo
        .record_user_deletion(
            &mut tx,
            previous,
            UserStorageVersion::INITIAL,
            &address,
            "delete:reassigned",
            OffsetDateTime::now_utc(),
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mut tx = uow.begin().await.unwrap();
    let claim = worker.claim_next(&mut tx).await.unwrap().unwrap();
    assert_eq!(revoke.intent_id, claim.intent.intent_id);
    tx.commit().await.unwrap();
    let new_owner = UserId::new();
    seed(&pool, new_owner, &address).await;
    let mut tx = uow.begin().await.unwrap();
    assert!(worker.read_claim(&mut tx, &claim).await.unwrap().is_none());
    assert!(
        !worker
            .finalize(
                &mut tx,
                &claim,
                ConsentIntentFinalization::Applied {
                    provider_contact_id: None
                },
                OffsetDateTime::now_utc()
            )
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    let mut tx = uow.begin().await.unwrap();
    let persisted = repo
        .find_by_source_key(&mut tx, "delete:reassigned")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ConsentIntentStatus::Blocked, persisted.status);
    assert_eq!(ConsentSubject::User(previous), persisted.subject);
    assert_eq!(address, persisted.email);
    assert!(
        sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM users WHERE user_id = $1)")
            .bind(new_owner.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap()
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn anonymous_grant_cannot_override_registered_email() {
    let pool = get_postgres_client().await;
    let uow = SqlxUnitOfWork::new(pool.clone());
    let repo = SqlxMarketingConsentIntentRepository::new();
    let worker = SqlxMarketingConsentIntentWorker::new();
    let address = email("later@example.test");
    let mut tx = uow.begin().await.unwrap();
    repo.record_email_only_intent(
        &mut tx,
        &address,
        true,
        ConsentIntentSource::AuraDoubleOptIn,
        "anonymous",
        OffsetDateTime::now_utc(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    seed(&pool, UserId::new(), &address).await;
    let mut tx = uow.begin().await.unwrap();
    assert!(worker.claim_next(&mut tx).await.unwrap().is_none());
    assert!(matches!(
        repo.record_email_only_intent(
            &mut tx,
            &address,
            true,
            ConsentIntentSource::AuraDoubleOptIn,
            "another",
            OffsetDateTime::now_utc()
        )
        .await,
        Err(MarketingConsentPersistenceError::RegisteredEmail)
    ));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn concurrent_same_proof_commits_one_intent_and_one_consent_revision() {
    let pool = get_postgres_client().await;
    let id = UserId::new();
    let address = email("concurrent-proof@example.test");
    seed(&pool, id, &address).await;

    async fn accept(
        pool: sqlx::PgPool,
        id: UserId,
        address: Email,
    ) -> MarketingConsentSyncIntentId {
        let uow = SqlxUnitOfWork::new(pool);
        let mut tx = uow.begin().await.unwrap();
        let intent = SqlxMarketingConsentIntentRepository::new()
            .record_user_transition(
                &mut tx,
                id,
                UserStorageVersion::INITIAL,
                &address,
                true,
                ConsentIntentSource::CognitoSignup,
                "signup:concurrent-proof",
                OffsetDateTime::now_utc(),
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();
        intent.intent_id
    }

    let (first, second) = tokio::join!(
        accept(pool.clone(), id, address.clone()),
        accept(pool.clone(), id, address),
    );
    assert_eq!(first, second);
    let state: (bool, i64, i64) = sqlx::query_as(
        "SELECT marketing_email_consent, marketing_email_consent_revision, version FROM users WHERE user_id = $1",
    )
    .bind(id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((true, 1, 2), state);
    assert_eq!(
        1,
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM marketing_email_consent_sync_intents WHERE source_key = $1"
        )
        .bind("signup:concurrent-proof")
        .fetch_one(&pool)
        .await
        .unwrap()
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn ordinary_profile_update_does_not_advance_revision_between_distinct_proofs() {
    use user_core::user::UserProfile;

    let pool = get_postgres_client().await;
    let id = UserId::new();
    let address = email("profile-proof@example.test");
    seed(&pool, id, &address).await;
    let uow = SqlxUnitOfWork::new(pool.clone());
    let intents = SqlxMarketingConsentIntentRepository::new();
    let users = SqlxUserRepositoryFactory::new();
    let mut tx = uow.begin().await.unwrap();
    let first = intents
        .record_user_transition(
            &mut tx,
            id,
            UserStorageVersion::INITIAL,
            &address,
            true,
            ConsentIntentSource::CognitoSignup,
            "signup:profile-proof",
            OffsetDateTime::now_utc(),
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let mut tx = uow.begin().await.unwrap();
    let loaded = users
        .in_transaction(&mut tx)
        .find_by_id(id)
        .await
        .unwrap()
        .unwrap();
    let mut changed = loaded.value;
    changed
        .replace_profile(UserProfile {
            first_name: Some("Updated".into()),
            last_name: None,
        })
        .unwrap();
    let updated = users
        .in_transaction(&mut tx)
        .update(&changed, loaded.version)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let after_profile: (bool, i64, i64) = sqlx::query_as(
        "SELECT marketing_email_consent, marketing_email_consent_revision, version FROM users WHERE user_id = $1",
    ).bind(id.as_uuid()).fetch_one(&pool).await.unwrap();
    assert_eq!((true, 1, 3), after_profile);
    assert_eq!(Some(1), first.consent_revision);
    assert_eq!(
        "Updated",
        sqlx::query_scalar::<_, String>("SELECT first_name FROM users WHERE user_id = $1")
            .bind(id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap()
    );

    let mut tx = uow.begin().await.unwrap();
    let second = intents
        .record_user_transition(
            &mut tx,
            id,
            updated.version,
            &address,
            true,
            ConsentIntentSource::AuraDoubleOptIn,
            "doi:profile-proof",
            OffsetDateTime::now_utc(),
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_ne!(first.intent_id, second.intent_id);
    assert_eq!(Some(2), second.consent_revision);
    let after_second: (bool, i64, i64) = sqlx::query_as(
        "SELECT marketing_email_consent, marketing_email_consent_revision, version FROM users WHERE user_id = $1",
    ).bind(id.as_uuid()).fetch_one(&pool).await.unwrap();
    assert_eq!((true, 2, 4), after_second);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn old_signup_replay_after_revoke_remains_revoked() {
    let pool = get_postgres_client().await;
    let id = UserId::new();
    let address = email("withdrawn-signup@example.test");
    seed(&pool, id, &address).await;
    let uow = SqlxUnitOfWork::new(pool.clone());
    let repo = SqlxMarketingConsentIntentRepository::new();
    let worker = SqlxMarketingConsentIntentWorker::new();
    let mut tx = uow.begin().await.unwrap();
    let grant = repo
        .record_user_transition(
            &mut tx,
            id,
            UserStorageVersion::INITIAL,
            &address,
            true,
            ConsentIntentSource::CognitoSignup,
            "signup:withdrawn",
            OffsetDateTime::now_utc(),
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let mut tx = uow.begin().await.unwrap();
    let revoke = repo
        .record_user_transition(
            &mut tx,
            id,
            UserStorageVersion::try_from(2_i64).unwrap(),
            &address,
            false,
            ConsentIntentSource::UserWithdrawal,
            "withdraw:signup",
            OffsetDateTime::now_utc(),
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let mut tx = uow.begin().await.unwrap();
    let replay = repo
        .record_user_transition(
            &mut tx,
            id,
            UserStorageVersion::INITIAL,
            &address,
            true,
            ConsentIntentSource::CognitoSignup,
            "signup:withdrawn",
            OffsetDateTime::now_utc(),
        )
        .await
        .unwrap();
    assert_eq!(grant.intent_id, replay.intent_id);
    assert_eq!(ConsentIntentStatus::Superseded, replay.status);
    tx.commit().await.unwrap();
    let current: (bool, i64, i64) = sqlx::query_as(
        "SELECT marketing_email_consent, marketing_email_consent_revision, version FROM users WHERE user_id = $1",
    ).bind(id.as_uuid()).fetch_one(&pool).await.unwrap();
    assert_eq!((false, 2, 3), current);
    assert_eq!(
        2,
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM marketing_email_consent_sync_intents WHERE user_id = $1"
        )
        .bind(id.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap()
    );
    let mut tx = uow.begin().await.unwrap();
    let next = worker.claim_next(&mut tx).await.unwrap().unwrap();
    assert_eq!(revoke.intent_id, next.intent.intent_id);
    assert!(!next.intent.desired);
}

#[derive(Debug, PartialEq, Eq, sqlx::FromRow)]
struct PersistedCompletion {
    status: String,
    lease_token: Option<uuid::Uuid>,
    lease_expires_at: Option<OffsetDateTime>,
    completed_lease_token: Option<uuid::Uuid>,
    completed_at: Option<OffsetDateTime>,
    provider_contact_id: Option<String>,
    last_error_code: Option<String>,
    attempt_count: i32,
    updated: OffsetDateTime,
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn finalize_exact_retry_keeps_receipt_and_conflicting_completion_cannot_succeed() {
    let pool = get_postgres_client().await;
    let uow = SqlxUnitOfWork::new(pool.clone());
    let repo = SqlxMarketingConsentIntentRepository::new();
    let worker = SqlxMarketingConsentIntentWorker::new();
    let address = email("receipt@example.test");
    let mut tx = uow.begin().await.unwrap();
    repo.record_email_only_intent(
        &mut tx,
        &address,
        false,
        ConsentIntentSource::EmailOnlyWithdrawal,
        "withdraw:receipt",
        OffsetDateTime::now_utc(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let mut tx = uow.begin().await.unwrap();
    let claim = worker.claim_next(&mut tx).await.unwrap().unwrap();
    tx.commit().await.unwrap();

    let completed_at = OffsetDateTime::now_utc();
    let applied = ConsentIntentFinalization::Applied {
        provider_contact_id: Some("contact-receipt"),
    };
    let mut tx = uow.begin().await.unwrap();
    assert!(
        worker
            .finalize(&mut tx, &claim, applied, completed_at)
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    let persisted = async |pool: &sqlx::PgPool| -> PersistedCompletion {
        sqlx::query_as::<_, PersistedCompletion>("SELECT status, lease_token, lease_expires_at, completed_lease_token, completed_at, provider_contact_id, last_error_code, attempt_count, updated FROM marketing_email_consent_sync_intents WHERE intent_id = $1")
            .bind(claim.intent.intent_id.as_uuid()).fetch_one(pool).await.unwrap()
    };
    let receipt = persisted(&pool).await;
    assert_eq!("APPLIED", receipt.status);
    assert_eq!(Some(claim.lease_token), receipt.completed_lease_token);
    assert!(receipt.lease_token.is_none());
    assert!(receipt.lease_expires_at.is_none());
    assert_eq!(
        Some("contact-receipt".to_owned()),
        receipt.provider_contact_id
    );
    assert!(receipt.last_error_code.is_none());
    let mut tx = uow.begin().await.unwrap();
    assert!(
        worker
            .finalize(&mut tx, &claim, applied, completed_at)
            .await
            .unwrap()
    );
    assert!(
        !worker
            .finalize(
                &mut tx,
                &claim,
                applied,
                completed_at + Duration::seconds(1)
            )
            .await
            .unwrap()
    );
    assert!(
        !worker
            .finalize(
                &mut tx,
                &claim,
                ConsentIntentFinalization::Applied {
                    provider_contact_id: Some("different-contact")
                },
                completed_at
            )
            .await
            .unwrap()
    );
    assert!(
        !worker
            .finalize(
                &mut tx,
                &claim,
                ConsentIntentFinalization::Failed {
                    error_code: "provider_failed"
                },
                completed_at
            )
            .await
            .unwrap()
    );
    let mut wrong_token = claim.clone();
    wrong_token.lease_token = uuid::Uuid::now_v7();
    assert!(
        !worker
            .finalize(&mut tx, &wrong_token, applied, completed_at)
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    assert_eq!(receipt, persisted(&pool).await);
}
