use application::operation_context::{OperationContext, Principal};
use application::transaction::{Transaction, UnitOfWork};
use platform_postgres::SqlxUnitOfWork;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use user_core::user_id::UserId;
use user_postgres::{
    SqlxCognitoUserIdentityReader, SqlxMarketingConsentIntentRepository,
    SqlxUserAdminReaderFactory, SqlxUserCognitoIdentityRegistryFactory,
    SqlxUserIdentityDeletionStoreFactory, SqlxUserRepositoryFactory,
};
use user_service::ports::user_identity_deletion::{
    UserIdentityDeleter, UserIdentityDeletionClaimOutcome, UserIdentityDeletionError,
    UserIdentityDeletionStore, UserIdentityDeletionStoreFactory, UserIdentityUsername,
};
use user_service::ports::{
    CognitoIdentity, CognitoIssuer, CognitoSubject, CognitoUserIdentityReader,
    UserCognitoIdentityRegistry, UserCognitoIdentityRegistryFactory,
};
use user_service::use_cases::commands::delete_user_identity::{
    DeleteUserIdentityHandler, DeleteUserIdentityResult, DeleteUserIdentityUseCase,
};
use user_service::use_cases::{
    DeleteUserCommand, DeleteUserHandler, DeleteUserUseCase, RegisterCognitoUserCommand,
    RegisterCognitoUserHandler, RegisterCognitoUserUseCase,
};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
const ISSUER: &str = "https://cognito-idp.eu-central-1.amazonaws.com/eu-central-1_test";

fn identity(subject: &str) -> CognitoIdentity {
    CognitoIdentity {
        issuer: CognitoIssuer::try_from(ISSUER).unwrap(),
        subject: CognitoSubject::try_from(subject).unwrap(),
    }
}

fn context(principal: Principal) -> OperationContext {
    OperationContext {
        principal,
        request_id: "test-delete".into(),
        correlation_id: "test-delete".into(),
    }
}

async fn seed(pool: &sqlx::PgPool, email: &str, subject: &str) -> UserId {
    let id = UserId::new();
    sqlx::query("INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'FREE', 'USER')")
        .bind(id.as_uuid())
        .bind(email)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO user_cognito_identities (issuer, subject, user_id) VALUES ($1, $2, $3)",
    )
    .bind(ISSUER)
    .bind(subject)
    .bind(id.as_uuid())
    .execute(pool)
    .await
    .unwrap();
    id
}

struct Provider {
    pool: sqlx::PgPool,
    fail: AtomicBool,
    missing: AtomicBool,
    resolves: AtomicUsize,
    deletes: AtomicUsize,
}
#[async_trait::async_trait]
impl UserIdentityDeleter for Provider {
    async fn resolve_username(
        &self,
        _: &CognitoIdentity,
    ) -> Result<Option<UserIdentityUsername>, UserIdentityDeletionError> {
        self.resolves.fetch_add(1, Ordering::SeqCst);
        if self.missing.swap(false, Ordering::SeqCst) {
            return Ok(None);
        }
        Ok(Some(UserIdentityUsername::try_from(
            "provider-username".to_owned(),
        )?))
    }
    async fn delete_identity(
        &self,
        identity: &CognitoIdentity,
        username: &UserIdentityUsername,
    ) -> Result<(), UserIdentityDeletionError> {
        self.deletes.fetch_add(1, Ordering::SeqCst);
        assert!(
            SqlxCognitoUserIdentityReader::new(self.pool.clone())
                .find_identity(identity)
                .await
                .unwrap()
                .is_none()
        );
        // Both the lease and exact provider target must commit before provider I/O.
        let claimed: bool = sqlx::query_scalar("SELECT version > 0 AND lease_expires_at > now() AND username = $3 FROM user_identity_deletions WHERE issuer = $1 AND subject = $2")
            .bind(identity.issuer.as_str()).bind(identity.subject.as_str()).bind(username.as_str()).fetch_one(&self.pool).await.unwrap();
        assert!(claimed);
        if self.fail.swap(false, Ordering::SeqCst) {
            Err(UserIdentityDeletionError(application::error::static_error(
                "provider response lost after deletion",
            )))
        } else {
            Ok(())
        }
    }
}

#[async_trait::async_trait]
impl UserIdentityDeleter for &Provider {
    async fn resolve_username(
        &self,
        identity: &CognitoIdentity,
    ) -> Result<Option<UserIdentityUsername>, UserIdentityDeletionError> {
        (**self).resolve_username(identity).await
    }
    async fn delete_identity(
        &self,
        identity: &CognitoIdentity,
        username: &UserIdentityUsername,
    ) -> Result<(), UserIdentityDeletionError> {
        (**self).delete_identity(identity, username).await
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn deletion_survives_provider_failure_and_fences_old_registration_but_allows_new_subject() {
    let pool = get_postgres_client().await;
    let uow = SqlxUnitOfWork::new(pool.clone());
    let id = seed(&pool, "deleted@example.test", "old-subject").await;
    DeleteUserHandler::new(
        uow.clone(),
        SqlxMarketingConsentIntentRepository::new(),
        SqlxUserAdminReaderFactory::new(),
        SqlxUserIdentityDeletionStoreFactory,
    )
    .execute(
        &context(Principal::User(id)),
        DeleteUserCommand { user_id: id },
    )
    .await
    .unwrap();
    let provider = Provider {
        pool: pool.clone(),
        fail: AtomicBool::new(true),
        missing: AtomicBool::new(false),
        resolves: AtomicUsize::new(0),
        deletes: AtomicUsize::new(0),
    };
    let worker = DeleteUserIdentityHandler::new(
        uow.clone(),
        SqlxUserIdentityDeletionStoreFactory,
        &provider,
    );
    assert_eq!(
        id.into_uuid(),
        sqlx::query_scalar::<_, uuid::Uuid>("SELECT user_id FROM user_identity_deletion_requests")
            .fetch_one(&pool)
            .await
            .unwrap()
    );
    let error = worker.execute(id).await.unwrap_err();
    assert!(std::error::Error::source(&error).is_some());
    let completed: Option<time::OffsetDateTime> =
        sqlx::query_scalar("SELECT completed_at FROM user_identity_deletions")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(completed.is_none());
    assert_eq!(
        DeleteUserIdentityResult::Deferred,
        worker.execute(id).await.unwrap()
    );
    sqlx::query(
        "UPDATE user_identity_deletions SET lease_expires_at = now() - interval '1 second'",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        DeleteUserIdentityResult::Completed,
        worker.execute(id).await.unwrap()
    );
    assert_eq!(1, provider.resolves.load(Ordering::SeqCst));
    assert_eq!(2, provider.deletes.load(Ordering::SeqCst));
    assert!(sqlx::query_scalar::<_, bool>("SELECT completed_at IS NOT NULL AND lease_expires_at IS NULL FROM user_identity_deletions").fetch_one(&pool).await.unwrap());

    let registration = RegisterCognitoUserHandler::new(
        uow.clone(),
        SqlxUserRepositoryFactory::new(),
        SqlxUserCognitoIdentityRegistryFactory::new(),
        SqlxMarketingConsentIntentRepository::new(),
    );
    let mut command = RegisterCognitoUserCommand {
        identity: identity("old-subject"),
        email: serde_email::Email::try_from("deleted@example.test").unwrap(),
        initial_first_name: None,
        initial_last_name: None,
        initial_language: None,
        signup_consent: None,
    };
    assert!(
        registration
            .execute(&context(Principal::System), command.clone())
            .await
            .is_err()
    );
    command.identity = identity("new-subject");
    let new_account = registration
        .execute(&context(Principal::System), command)
        .await
        .unwrap();
    assert_ne!(id, new_account.user_id);

    // Duplicate queue delivery after completion performs no provider I/O.
    assert_eq!(
        DeleteUserIdentityResult::Completed,
        worker.execute(id).await.unwrap()
    );
    assert_eq!(2, provider.deletes.load(Ordering::SeqCst));
    assert!(
        SqlxCognitoUserIdentityReader::new(pool)
            .find_identity(&identity("new-subject"))
            .await
            .unwrap()
            .is_some()
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn deletion_intent_rolls_back_with_account_and_claims_are_exclusive_and_fenced() {
    let pool = get_postgres_client().await;
    let uow = SqlxUnitOfWork::new(pool.clone());
    let id = seed(&pool, "rollback@example.test", "rollback-subject").await;
    let store = SqlxUserIdentityDeletionStoreFactory;
    let mut tx = uow.begin().await.unwrap();
    store
        .in_transaction(&mut tx)
        .request_for_user(id)
        .await
        .unwrap();
    sqlx::query("DELETE FROM users WHERE user_id = $1")
        .bind(id.as_uuid())
        .execute(tx.connection())
        .await
        .unwrap();
    drop(tx);
    assert_eq!(
        0_i64,
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM user_identity_deletion_requests")
            .fetch_one(&pool)
            .await
            .unwrap()
    );
    assert_eq!(
        0_i64,
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM user_identity_deletions")
            .fetch_one(&pool)
            .await
            .unwrap()
    );
    assert!(
        SqlxCognitoUserIdentityReader::new(pool.clone())
            .find_identity(&identity("rollback-subject"))
            .await
            .unwrap()
            .is_some()
    );
    DeleteUserHandler::new(
        uow.clone(),
        SqlxMarketingConsentIntentRepository::new(),
        SqlxUserAdminReaderFactory::new(),
        store,
    )
    .execute(
        &context(Principal::User(id)),
        DeleteUserCommand { user_id: id },
    )
    .await
    .unwrap();
    let mut tx = uow.begin().await.unwrap();
    let UserIdentityDeletionClaimOutcome::Claimed(first) =
        store.in_transaction(&mut tx).claim(id).await.unwrap()
    else {
        panic!("expected claim")
    };
    tx.commit().await.unwrap();
    let mut tx = uow.begin().await.unwrap();
    assert!(matches!(
        store.in_transaction(&mut tx).claim(id).await.unwrap(),
        UserIdentityDeletionClaimOutcome::Deferred
    ));
    tx.commit().await.unwrap();
    sqlx::query(
        "UPDATE user_identity_deletions SET lease_expires_at = now() - interval '1 second'",
    )
    .execute(&pool)
    .await
    .unwrap();
    let mut tx = uow.begin().await.unwrap();
    let UserIdentityDeletionClaimOutcome::Claimed(second) =
        store.in_transaction(&mut tx).claim(id).await.unwrap()
    else {
        panic!("expected claim")
    };
    tx.commit().await.unwrap();
    assert!(second.version > first.version);
    let username = UserIdentityUsername::try_from("provider-username".to_owned()).unwrap();
    let mut tx = uow.begin().await.unwrap();
    assert!(
        store
            .in_transaction(&mut tx)
            .save_username(&first, &username)
            .await
            .is_err()
    );
    assert!(
        store
            .in_transaction(&mut tx)
            .complete(&first)
            .await
            .is_err()
    );
    drop(tx);
    let mut tx = uow.begin().await.unwrap();
    store
        .in_transaction(&mut tx)
        .save_username(&second, &username)
        .await
        .unwrap();
    store
        .in_transaction(&mut tx)
        .complete(&second)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let replacement = seed(&pool, "replacement@example.test", "replacement-subject").await;
    let registry = SqlxUserCognitoIdentityRegistryFactory::new();
    let mut tx = uow.begin().await.unwrap();
    assert!(
        registry
            .in_transaction(&mut tx)
            .bind(&identity("rollback-subject"), replacement)
            .await
            .is_err()
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn admin_deletion_records_the_same_provider_request_and_unbound_users_need_none() {
    let pool = get_postgres_client().await;
    let uow = SqlxUnitOfWork::new(pool.clone());
    let admin = seed(&pool, "admin@example.test", "admin-subject").await;
    sqlx::query("UPDATE users SET role = 'ADMIN' WHERE user_id = $1")
        .bind(admin.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    let target = seed(&pool, "target@example.test", "target-subject").await;
    let handler = DeleteUserHandler::new_admin_only(
        uow,
        SqlxMarketingConsentIntentRepository::new(),
        SqlxUserAdminReaderFactory::new(),
        SqlxUserIdentityDeletionStoreFactory,
    );
    handler
        .execute(
            &context(Principal::User(admin)),
            DeleteUserCommand { user_id: target },
        )
        .await
        .unwrap();
    assert_eq!(
        "target-subject",
        sqlx::query_scalar::<_, String>("SELECT subject FROM user_identity_deletions")
            .fetch_one(&pool)
            .await
            .unwrap()
    );
    let unbound = seed(&pool, "unbound@example.test", "unbound-subject").await;
    sqlx::query("DELETE FROM user_cognito_identities WHERE user_id = $1")
        .bind(unbound.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    handler
        .execute(
            &context(Principal::User(admin)),
            DeleteUserCommand { user_id: unbound },
        )
        .await
        .unwrap();
    assert_eq!(
        1_i64,
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM user_identity_deletions")
            .fetch_one(&pool)
            .await
            .unwrap()
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn eventually_consistent_initial_absence_keeps_work_unfinished_until_observed() {
    let pool = get_postgres_client().await;
    let uow = SqlxUnitOfWork::new(pool.clone());
    let id = seed(&pool, "late@example.test", "late-subject").await;
    DeleteUserHandler::new(
        uow.clone(),
        SqlxMarketingConsentIntentRepository::new(),
        SqlxUserAdminReaderFactory::new(),
        SqlxUserIdentityDeletionStoreFactory,
    )
    .execute(
        &context(Principal::User(id)),
        DeleteUserCommand { user_id: id },
    )
    .await
    .unwrap();
    let provider = Provider {
        pool: pool.clone(),
        fail: AtomicBool::new(false),
        missing: AtomicBool::new(true),
        resolves: AtomicUsize::new(0),
        deletes: AtomicUsize::new(0),
    };
    let worker =
        DeleteUserIdentityHandler::new(uow, SqlxUserIdentityDeletionStoreFactory, &provider);
    assert!(worker.execute(id).await.is_err());
    assert_eq!(0, provider.deletes.load(Ordering::SeqCst));
    assert!(
        sqlx::query_scalar::<_, bool>(
            "SELECT completed_at IS NULL AND username IS NULL FROM user_identity_deletions"
        )
        .fetch_one(&pool)
        .await
        .unwrap()
    );
    sqlx::query(
        "UPDATE user_identity_deletions SET lease_expires_at = now() - interval '1 second'",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        DeleteUserIdentityResult::Completed,
        worker.execute(id).await.unwrap()
    );
    assert_eq!(2, provider.resolves.load(Ordering::SeqCst));
    assert_eq!(1, provider.deletes.load(Ordering::SeqCst));
}
