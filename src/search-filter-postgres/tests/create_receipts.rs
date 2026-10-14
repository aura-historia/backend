use application::{
    idempotency::{CommandReceiptError, CommandReceipts, CommandReceiptsFactory},
    operation_context::{CredentialCapability, OperationContext, Principal},
    transaction::{Transaction, TransactionError, UnitOfWork},
};
use embedding::{
    EmbeddingError, EmbeddingGenerator, EmbeddingImageUrl, EmbeddingText, EmbeddingVector,
};
use localization::Language;
use money::Currency;
use platform_postgres::{SqlxTransaction, SqlxUnitOfWork};
use product_listing_core::product_listing_search::ProductListingSearch;
use search_filter_postgres::{
    SqlxSearchFilterCreateReceiptsFactory, SqlxSearchFilterQuotaReaderFactory,
    SqlxSearchFilterRepositoryFactory,
};
use search_filter_service::{
    create_receipt::CreateSearchFilterIntent,
    ports::{SearchFilterRepository, SearchFilterRepositoryFactory},
    use_cases::{
        CreateSearchFilterCommand, CreateSearchFilterError, CreateSearchFilterHandler,
        CreateSearchFilterResult, CreateSearchFilterUseCase,
    },
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use tokio::sync::Barrier;
use user_core::user_id::UserId;
use user_postgres::SqlxUserTierEntitlementsFactory;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[derive(Clone, Default)]
struct Embeddings {
    calls: Arc<AtomicUsize>,
    barrier: Option<Arc<Barrier>>,
    owner: Option<(sqlx::PgPool, UserId)>,
}

struct FailingReceiptsFactory;
struct FailingReceipts<'tx>(&'tx mut SqlxTransaction);
impl CommandReceiptsFactory<SqlxTransaction, CreateSearchFilterIntent, CreateSearchFilterResult>
    for FailingReceiptsFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl CommandReceipts<CreateSearchFilterIntent, CreateSearchFilterResult> + 'tx {
        FailingReceipts(tx)
    }
}
#[async_trait::async_trait]
impl CommandReceipts<CreateSearchFilterIntent, CreateSearchFilterResult> for FailingReceipts<'_> {
    async fn find(
        &mut self,
        intent: &CreateSearchFilterIntent,
    ) -> Result<Option<CreateSearchFilterResult>, CommandReceiptError> {
        SqlxSearchFilterCreateReceiptsFactory
            .in_transaction(self.0)
            .find(intent)
            .await
    }
    async fn coordinate(
        &mut self,
        intent: &CreateSearchFilterIntent,
    ) -> Result<(), CommandReceiptError> {
        SqlxSearchFilterCreateReceiptsFactory
            .in_transaction(self.0)
            .coordinate(intent)
            .await
    }
    async fn complete(
        &mut self,
        intent: &CreateSearchFilterIntent,
        outcome: &CreateSearchFilterResult,
    ) -> Result<(), CommandReceiptError> {
        SqlxSearchFilterCreateReceiptsFactory
            .in_transaction(self.0)
            .complete(intent, outcome)
            .await?;
        Err(CommandReceiptError::Storage(
            application::error::static_error("injected failure after receipt insertion"),
        ))
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn failed_transaction_rolls_back_creation_and_receipt_together() {
    let pool = get_postgres_client().await;
    let user = owner(&pool, "FREE").await;
    let create = CreateSearchFilterHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxSearchFilterRepositoryFactory,
        Embeddings::default(),
        SqlxSearchFilterQuotaReaderFactory,
        SqlxUserTierEntitlementsFactory,
        FailingReceiptsFactory,
    );
    let error = create
        .execute(
            &context(Principal::User(user)),
            command(user, Some("rollback")),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        CreateSearchFilterError::Receipt(CommandReceiptError::Storage(_))
    ));
    assert_eq!((0, 0), counts(&pool, user).await);
}

struct LostCommitUnit {
    unit: SqlxUnitOfWork,
    begins: AtomicUsize,
}
struct LostCommitTransaction {
    inner: SqlxTransaction,
    lose_ack: bool,
}
#[async_trait::async_trait]
impl UnitOfWork for LostCommitUnit {
    type Tx = LostCommitTransaction;
    async fn begin(&self) -> Result<Self::Tx, TransactionError> {
        Ok(LostCommitTransaction {
            inner: self.unit.begin().await?,
            lose_ack: self.begins.fetch_add(1, Ordering::SeqCst) == 1,
        })
    }
}
#[async_trait::async_trait]
impl Transaction for LostCommitTransaction {
    async fn commit(self) -> Result<(), TransactionError> {
        self.inner.commit().await?;
        if self.lose_ack {
            Err(TransactionError::CommitFailed(
                application::error::static_error("commit succeeded, acknowledgement lost"),
            ))
        } else {
            Ok(())
        }
    }
}
struct LostCommitFactories;
impl SearchFilterRepositoryFactory<LostCommitTransaction> for LostCommitFactories {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut LostCommitTransaction,
    ) -> impl SearchFilterRepository + 'tx {
        SqlxSearchFilterRepositoryFactory.in_transaction(&mut tx.inner)
    }
}
impl search_filter_service::ports::SearchFilterQuotaReaderFactory<LostCommitTransaction>
    for LostCommitFactories
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut LostCommitTransaction,
    ) -> impl search_filter_service::ports::SearchFilterQuotaReader + 'tx {
        search_filter_service::ports::SearchFilterQuotaReaderFactory::in_transaction(
            &SqlxSearchFilterQuotaReaderFactory,
            &mut tx.inner,
        )
    }
}
impl user_service::ports::UserTierEntitlementsFactory<LostCommitTransaction>
    for LostCommitFactories
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut LostCommitTransaction,
    ) -> impl user_service::ports::UserTierEntitlements + 'tx {
        user_service::ports::UserTierEntitlementsFactory::in_transaction(
            &SqlxUserTierEntitlementsFactory,
            &mut tx.inner,
        )
    }
}
impl
    CommandReceiptsFactory<
        LostCommitTransaction,
        CreateSearchFilterIntent,
        CreateSearchFilterResult,
    > for LostCommitFactories
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut LostCommitTransaction,
    ) -> impl CommandReceipts<CreateSearchFilterIntent, CreateSearchFilterResult> + 'tx {
        SqlxSearchFilterCreateReceiptsFactory.in_transaction(&mut tx.inner)
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn ambiguous_commit_recovers_durable_evidence_without_repeating_provider_work() {
    let pool = get_postgres_client().await;
    let user = owner(&pool, "FREE").await;
    let embeddings = Embeddings::default();
    let create = CreateSearchFilterHandler::new(
        LostCommitUnit {
            unit: SqlxUnitOfWork::new(pool.clone()),
            begins: AtomicUsize::new(0),
        },
        LostCommitFactories,
        embeddings.clone(),
        LostCommitFactories,
        LostCommitFactories,
        LostCommitFactories,
    );
    let context = context(Principal::User(user));
    let command = command(user, Some("ambiguous-commit"));
    assert!(matches!(
        create.execute(&context, command.clone()).await,
        Err(CreateSearchFilterError::CommitTransactionFailed(_))
    ));
    assert_eq!((1, 1), counts(&pool, user).await);
    handler(&pool, embeddings.clone())
        .execute(&context, command)
        .await
        .unwrap();
    assert_eq!((1, 1), counts(&pool, user).await);
    assert_eq!(1, embeddings.calls.load(Ordering::SeqCst));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn invalid_receipt_identity_or_bound_content_fails_closed_without_provider_work() {
    let pool = get_postgres_client().await;
    let user = owner(&pool, "FREE").await;
    let embeddings = Embeddings::default();
    let create = handler(&pool, embeddings.clone());
    let context = context(Principal::User(user));
    let command = command(user, Some("corruption"));
    create.execute(&context, command.clone()).await.unwrap();
    let snapshot: serde_json::Value =
        sqlx::query_scalar("SELECT outcome FROM search_filter_create_receipts WHERE user_id=$1")
            .bind(user.into_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    for (field, value) in [
        (
            "user_search_filter_id",
            serde_json::json!(uuid::Uuid::new_v4().to_string()),
        ),
        ("notifications", serde_json::json!(false)),
    ] {
        let mut invalid = snapshot.clone();
        invalid[field] = value;
        sqlx::query("UPDATE search_filter_create_receipts SET outcome=$2 WHERE user_id=$1")
            .bind(user.into_uuid())
            .bind(invalid)
            .execute(&pool)
            .await
            .unwrap();
        assert!(matches!(
            create.execute(&context, command.clone()).await,
            Err(CreateSearchFilterError::Receipt(
                CommandReceiptError::InvalidPersistedState(_)
            ))
        ));
    }
    assert_eq!(1, embeddings.calls.load(Ordering::SeqCst));
    assert_eq!((1, 1), counts(&pool, user).await);
}
#[async_trait::async_trait]
impl EmbeddingGenerator for Embeddings {
    async fn embed_product(
        &self,
        _: &EmbeddingText,
        _: Option<&EmbeddingText>,
        _: Option<&EmbeddingImageUrl>,
    ) -> Result<EmbeddingVector, EmbeddingError> {
        unreachable!()
    }
    async fn embed_search_query(
        &self,
        _: &EmbeddingText,
    ) -> Result<EmbeddingVector, EmbeddingError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some((pool, owner)) = &self.owner {
            // Provider work must run with neither preparation nor write transactions holding
            // the owner lock. This query fails immediately if that boundary regresses.
            let mut tx = pool.begin().await.unwrap();
            sqlx::query("SELECT user_id FROM users WHERE user_id=$1 FOR UPDATE NOWAIT")
                .bind(owner.into_uuid())
                .execute(&mut *tx)
                .await
                .unwrap();
            tx.rollback().await.unwrap();
        }
        if let Some(barrier) = &self.barrier {
            barrier.wait().await;
        }
        EmbeddingVector::try_new(vec![1.0; embedding::EMBEDDING_DIMENSIONS])
    }
}

async fn owner(pool: &sqlx::PgPool, tier: &str) -> UserId {
    let id = UserId::new();
    sqlx::query("INSERT INTO users(user_id,email,tier,role) VALUES($1,$2,$3,'USER')")
        .bind(id.into_uuid())
        .bind(format!("{id}@example.test"))
        .bind(tier)
        .execute(pool)
        .await
        .unwrap();
    id
}
fn context(principal: Principal) -> OperationContext {
    OperationContext {
        command_key: None,
        principal,
        request_id: "receipt-test".into(),
        correlation_id: "receipt-test".into(),
    }
}
fn command(owner: UserId, key: Option<&str>) -> CreateSearchFilterCommand {
    CreateSearchFilterCommand {
        idempotency_key: key
            .map(|k| application::idempotency::IdempotencyKey::new(k.into()).unwrap()),
        user_id: owner,
        name: "original".into(),
        notifications: true,
        search: ProductListingSearch::new(Language::En, Currency::Eur)
            .with_product_listing_query("desk".try_into().unwrap()),
    }
}
fn handler(pool: &sqlx::PgPool, embeddings: Embeddings) -> impl CreateSearchFilterUseCase + use<> {
    CreateSearchFilterHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxSearchFilterRepositoryFactory,
        embeddings,
        SqlxSearchFilterQuotaReaderFactory,
        SqlxUserTierEntitlementsFactory,
        SqlxSearchFilterCreateReceiptsFactory,
    )
}
async fn counts(pool: &sqlx::PgPool, owner: UserId) -> (i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM search_filters WHERE user_id=$1), (SELECT count(*) FROM search_filter_create_receipts WHERE user_id=$1)")
        .bind(owner.into_uuid()).fetch_one(pool).await.unwrap()
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn completed_retry_skips_preparation_quota_and_later_edits_then_fences_deletion() {
    let pool = get_postgres_client().await;
    let user = owner(&pool, "FREE").await;
    let embeddings = Embeddings {
        owner: Some((pool.clone(), user)),
        ..Default::default()
    };
    let create = handler(&pool, embeddings.clone());
    let context = context(Principal::User(user));
    let command = command(user, Some("lost-response"));
    let first = create.execute(&context, command.clone()).await.unwrap();
    assert_eq!(
        first,
        create.execute(&context, command.clone()).await.unwrap()
    );
    let unit = SqlxUnitOfWork::new(pool.clone());
    let mut tx = unit.begin().await.unwrap();
    let persisted = SqlxSearchFilterRepositoryFactory
        .in_transaction(&mut tx)
        .find_by_id(first.filter.search_filter_id)
        .await
        .unwrap()
        .unwrap();
    let mut filter = persisted.filter;
    filter.change_notifications(false);
    SqlxSearchFilterRepositoryFactory
        .in_transaction(&mut tx)
        .update(&filter, persisted.version)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        first,
        create.execute(&context, command.clone()).await.unwrap()
    );
    let (notifications, version): (bool, i64) = sqlx::query_as(
        "SELECT notifications,version FROM search_filters WHERE user_search_filter_id=$1",
    )
    .bind(first.filter.search_filter_id.into_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((false, 2), (notifications, version));
    assert_eq!(1, embeddings.calls.load(Ordering::SeqCst));
    assert_eq!((1, 1), counts(&pool, user).await);
    let mut tx = unit.begin().await.unwrap();
    SqlxSearchFilterRepositoryFactory
        .in_transaction(&mut tx)
        .delete(first.filter.search_filter_id)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(matches!(
        create.execute(&context, command).await,
        Err(CreateSearchFilterError::Receipt(
            CommandReceiptError::ResultUnavailable
        ))
    ));
    assert_eq!((0, 1), counts(&pool, user).await);
    let scrubbed: bool = sqlx::query_scalar(
        "SELECT outcome IS NULL FROM search_filter_create_receipts WHERE user_id=$1",
    )
    .bind(user.into_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(scrubbed);
    sqlx::query("DELETE FROM users WHERE user_id=$1")
        .bind(user.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!((0, 0), counts(&pool, user).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn changed_intent_is_rejected_before_embedding_or_quota_work() {
    let pool = get_postgres_client().await;
    let user = owner(&pool, "FREE").await;
    let embeddings = Embeddings::default();
    let create = handler(&pool, embeddings.clone());
    let context = context(Principal::User(user));
    let mut command = command(user, Some("same-key"));
    create.execute(&context, command.clone()).await.unwrap();
    command.notifications = false;
    assert!(matches!(
        create.execute(&context, command).await,
        Err(CreateSearchFilterError::Receipt(
            CommandReceiptError::Mismatch
        ))
    ));
    assert_eq!(1, embeddings.calls.load(Ordering::SeqCst));
    assert_eq!((1, 1), counts(&pool, user).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn concurrent_first_attempts_can_prepare_twice_but_commit_only_once() {
    let pool = get_postgres_client().await;
    let user = owner(&pool, "FREE").await;
    let embeddings = Embeddings {
        barrier: Some(Arc::new(Barrier::new(2))),
        ..Default::default()
    };
    let create = handler(&pool, embeddings.clone());
    let peer = handler(&pool, embeddings.clone());
    let context = context(Principal::User(user));
    let command = command(user, Some("concurrent"));
    async fn retry_busy(
        create: &impl CreateSearchFilterUseCase,
        context: &OperationContext,
        command: CreateSearchFilterCommand,
    ) -> Result<CreateSearchFilterResult, CreateSearchFilterError> {
        loop {
            match create.execute(context, command.clone()).await {
                Err(CreateSearchFilterError::Receipt(CommandReceiptError::InProgress)) => {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await
                }
                result => return result,
            }
        }
    }
    let (a, b) = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        tokio::join!(
            retry_busy(&create, &context, command.clone()),
            retry_busy(&peer, &context, command.clone())
        )
    })
    .await
    .expect("concurrent preparations and command recovery must finish");
    for result in [&a, &b] {
        assert!(
            result.is_ok()
                || matches!(
                    result,
                    Err(CreateSearchFilterError::Receipt(
                        CommandReceiptError::InProgress
                    ))
                ),
            "{result:?}"
        );
    }
    assert!(a.is_ok() || b.is_ok());
    let replay = create.execute(&context, command).await.unwrap();
    for result in [a, b].into_iter().flatten() {
        assert_eq!(replay, result);
    }
    assert_eq!((1, 1), counts(&pool, user).await);
    assert_eq!(2, embeddings.calls.load(Ordering::SeqCst));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn owner_isolation_refreshed_credentials_and_current_scopes_precede_receipts() {
    let pool = get_postgres_client().await;
    let a = owner(&pool, "FREE").await;
    let b = owner(&pool, "FREE").await;
    let create = handler(&pool, Embeddings::default());
    let command_a = command(a, Some("shared-client-key"));
    let first = create
        .execute(&context(Principal::User(a)), command_a.clone())
        .await
        .unwrap();
    let delegated = Principal::DelegatedUser {
        user_id: a,
        capabilities: [CredentialCapability::SearchFiltersWrite].into(),
    };
    assert_eq!(
        first,
        create
            .execute(&context(delegated), command_a.clone())
            .await
            .unwrap()
    );
    let reduced = Principal::DelegatedUser {
        user_id: a,
        capabilities: Default::default(),
    };
    for principal in [reduced, Principal::User(b)] {
        assert!(matches!(
            create.execute(&context(principal), command_a.clone()).await,
            Err(CreateSearchFilterError::ActorMayNotManageSearchFilter)
        ));
    }
    assert!(matches!(
        create
            .execute(&context(Principal::Anonymous), command_a)
            .await,
        Err(CreateSearchFilterError::AuthenticatedActorRequired)
    ));
    let second = create
        .execute(
            &context(Principal::User(b)),
            command(b, Some("shared-client-key")),
        )
        .await
        .unwrap();
    assert_ne!(
        first.filter.search_filter_id,
        second.filter.search_filter_id
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn legacy_requests_remain_unkeyed_and_failed_new_commands_do_not_reserve_keys() {
    let pool = get_postgres_client().await;
    let user = owner(&pool, "FREE").await;
    let create = handler(&pool, Embeddings::default());
    let context = context(Principal::User(user));
    let first = create.execute(&context, command(user, None)).await.unwrap();
    let keyed = command(user, Some("quota-rejected"));
    assert!(matches!(
        create.execute(&context, keyed.clone()).await,
        Err(CreateSearchFilterError::SearchFilterQuotaExceeded { .. })
    ));
    assert_eq!((1, 0), counts(&pool, user).await);
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    SqlxSearchFilterRepositoryFactory
        .in_transaction(&mut tx)
        .delete(first.filter.search_filter_id)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    create.execute(&context, keyed).await.unwrap();
    assert_eq!((1, 1), counts(&pool, user).await);
}
