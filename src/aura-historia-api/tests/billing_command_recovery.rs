use application::{
    error::static_error,
    external_operation::ExternalAttempt,
    idempotency::{CommandIntent, CommandReceiptError, IdempotencyKey},
    operation_context::{OperationContext, Principal},
    transaction::{Transaction, TransactionError, UnitOfWork},
};
use billing_service::{ports::*, use_cases::*};
use platform_postgres::{SqlxTransaction, SqlxUnitOfWork};
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use url::Url;
use user_core::{stripe_customer_id::StripeCustomerId, user_id::UserId};
use user_postgres::{SqlxUserAccountReaderFactory, SqlxUserRepositoryFactory};
use user_service::use_cases::{AssociateUserStripeCustomerIdHandler, GetOwnUserHandler};
const SCHEMA: Postgres = Postgres::new("migrations");
#[derive(Default)]
struct ProviderState {
    customers: HashMap<String, CreateStripeCustomerRequest>,
    sessions: HashMap<String, CreateStripeCheckoutSessionRequest>,
    customer_calls: usize,
    session_calls: usize,
    lose_customer: bool,
    lose_session: bool,
}
#[derive(Clone, Default)]
struct Provider(Arc<Mutex<ProviderState>>);
#[async_trait::async_trait]
impl StripeCustomerCreator for Provider {
    async fn create_customer(
        &self,
        request: CreateStripeCustomerRequest,
    ) -> Result<StripeCustomerId, StripeBillingError> {
        let mut state = self.0.lock().unwrap();
        state.customer_calls += 1;
        let key = request
            .idempotency_key
            .clone()
            .expect("stable customer step key");
        if let Some(old) = state.customers.get(&key) {
            assert_eq!(old, &request);
        } else {
            state.customers.insert(key, request.clone());
        }
        if std::mem::take(&mut state.lose_customer) {
            return Err(StripeBillingError::TemporarilyUnavailable {
                source: static_error("accepted customer; response lost"),
            });
        }
        Ok(StripeCustomerId::from(format!(
            "cus_{}",
            request.user_id.as_uuid()
        )))
    }
}
#[async_trait::async_trait]
impl StripeCheckoutSessionCreator for Provider {
    fn redirects(&self) -> Option<CheckoutRedirects> {
        Some(CheckoutRedirects {
            success: Url::parse("https://example.test/success").unwrap(),
            cancel: Url::parse("https://example.test/cancel").unwrap(),
        })
    }
    async fn create_checkout_session(
        &self,
        request: CreateStripeCheckoutSessionRequest,
    ) -> Result<Url, StripeBillingError> {
        let mut state = self.0.lock().unwrap();
        state.session_calls += 1;
        let key = request
            .idempotency_key
            .clone()
            .expect("stable session step key");
        if let Some(old) = state.sessions.get(&key) {
            assert_eq!(old, &request);
        } else {
            state.sessions.insert(key.clone(), request);
        }
        if std::mem::take(&mut state.lose_session) {
            return Err(StripeBillingError::TemporarilyUnavailable {
                source: static_error("accepted session; response lost"),
            });
        }
        Ok(Url::parse(&format!("https://checkout.example.test/{key}")).unwrap())
    }
}
#[derive(Clone)]
struct LostAckUnit {
    unit: SqlxUnitOfWork,
    begins: Arc<AtomicUsize>,
    fail_at: usize,
}
struct LostAckTx {
    inner: SqlxTransaction,
    lose: bool,
}
#[async_trait::async_trait]
impl UnitOfWork for LostAckUnit {
    type Tx = LostAckTx;
    async fn begin(&self) -> Result<Self::Tx, TransactionError> {
        Ok(LostAckTx {
            inner: self.unit.begin().await?,
            lose: self.begins.fetch_add(1, Ordering::SeqCst) + 1 == self.fail_at,
        })
    }
}
#[async_trait::async_trait]
impl Transaction for LostAckTx {
    async fn commit(self) -> Result<(), TransactionError> {
        self.inner.commit().await?;
        if self.lose {
            Err(TransactionError::CommitFailed(static_error(
                "commit succeeded; acknowledgement lost",
            )))
        } else {
            Ok(())
        }
    }
    async fn claim_external_operation(
        &mut self,
        intent: &CommandIntent,
        initial: Value,
        scope: &str,
        resumable: bool,
    ) -> Result<ExternalAttempt, CommandReceiptError> {
        self.inner
            .claim_external_operation(intent, initial, scope, resumable)
            .await
    }
    async fn checkpoint_external_operation(
        &mut self,
        intent: &CommandIntent,
        attempt: i64,
        state: Value,
        completed: bool,
    ) -> Result<(), CommandReceiptError> {
        self.inner
            .checkpoint_external_operation(intent, attempt, state, completed)
            .await
    }
}
fn handler<T: UnitOfWork + 'static>(
    pool: &sqlx::PgPool,
    provider: Provider,
    price: &str,
    operations: T,
) -> impl CreateBillingCheckoutSessionUseCase {
    let unit = SqlxUnitOfWork::new(pool.clone());
    CreateBillingCheckoutSessionHandler::new(
        GetOwnUserHandler::new(unit.clone(), SqlxUserAccountReaderFactory::new()),
        AssociateUserStripeCustomerIdHandler::new(unit, SqlxUserRepositoryFactory::new()),
        provider.clone(),
        provider,
        BillingPriceIds {
            pro_monthly: price.into(),
            pro_yearly: "year".into(),
            ultimate_monthly: "ultimate".into(),
            ultimate_yearly: "ultimate_year".into(),
        },
    )
    .with_operations(operations)
}
async fn owner(pool: &sqlx::PgPool) -> UserId {
    let id = UserId::new();
    sqlx::query("INSERT INTO users(user_id,email,tier,role) VALUES($1,$2,'FREE','USER')")
        .bind(id.into_uuid())
        .bind(format!("{id}@example.test"))
        .execute(pool)
        .await
        .unwrap();
    id
}
fn context(user: UserId) -> OperationContext {
    OperationContext {
        command_key: Some(IdempotencyKey::new("checkout-logical-action".into()).unwrap()),
        principal: Principal::User(user),
        request_id: "billing-test".into(),
        correlation_id: "billing-test".into(),
    }
}
fn command() -> CreateBillingCheckoutSessionCommand {
    CreateBillingCheckoutSessionCommand {
        plan: BillingPlan::Pro,
        cycle: BillingCycle::Monthly,
        idempotency_key: None,
    }
}
async fn release_lease(pool: &sqlx::PgPool, user: UserId) {
    sqlx::query("UPDATE external_command_operations SET lease_until=clock_timestamp()-interval '1 second' WHERE private_owner=$1").bind(user.into_uuid()).execute(pool).await.unwrap();
}
#[aura_integration_test(services=[SCHEMA])]
async fn every_lost_local_acknowledgement_resumes_frozen_steps_without_an_extra_customer_or_session()
 {
    let pool = get_postgres_client().await;
    // Admission, frozen initialization, customer checkpoint, association checkpoint, URL finalization.
    for fail_at in 1..=5 {
        let user = owner(&pool).await;
        let provider = Provider::default();
        let unit = SqlxUnitOfWork::new(pool.clone());
        let ctx = context(user);
        let lost = LostAckUnit {
            unit: unit.clone(),
            begins: Arc::new(AtomicUsize::new(0)),
            fail_at,
        };
        assert!(
            handler(&pool, provider.clone(), "original-price", lost)
                .execute(&ctx, command())
                .await
                .is_err(),
            "boundary {fail_at}"
        );
        release_lease(&pool, user).await;
        let retried = handler(&pool, provider.clone(), "changed-live-price", unit.clone())
            .execute(&ctx, command())
            .await
            .unwrap();
        let replay = handler(&pool, provider.clone(), "another-live-price", unit)
            .execute(&ctx, command())
            .await
            .unwrap();
        assert_eq!(retried, replay);
        let state = provider.0.lock().unwrap();
        assert_eq!(1, state.customers.len(), "boundary {fail_at}");
        assert_eq!(1, state.sessions.len());
        assert_eq!(1, state.customer_calls);
        assert_eq!(1, state.session_calls);
        assert_eq!(
            "original-price",
            state.sessions.values().next().unwrap().price_id
        );
    }
}
#[aura_integration_test(services=[SCHEMA])]
async fn ambiguous_provider_acceptance_keeps_stable_step_keys_and_finite_recovery() {
    let pool = get_postgres_client().await;
    let user = owner(&pool).await;
    let provider = Provider::default();
    {
        let mut state = provider.0.lock().unwrap();
        state.lose_customer = true;
        state.lose_session = true;
    }
    let unit = SqlxUnitOfWork::new(pool.clone());
    let ctx = context(user);
    assert!(
        handler(&pool, provider.clone(), "frozen-price", unit.clone())
            .execute(&ctx, command())
            .await
            .is_err()
    );
    let busy = handler(&pool, provider.clone(), "new-price", unit.clone())
        .execute(&ctx, command())
        .await;
    assert!(matches!(
        busy,
        Err(CreateBillingCheckoutSessionError::Receipt(
            CommandReceiptError::InProgress
        ))
    ));
    release_lease(&pool, user).await;
    assert!(
        handler(&pool, provider.clone(), "new-price", unit.clone())
            .execute(&ctx, command())
            .await
            .is_err()
    );
    release_lease(&pool, user).await;
    let outcome = handler(&pool, provider.clone(), "new-price", unit.clone())
        .execute(&ctx, command())
        .await
        .unwrap();
    assert_eq!(
        outcome,
        handler(&pool, provider.clone(), "new-price", unit.clone())
            .execute(&ctx, command())
            .await
            .unwrap()
    );
    {
        let state = provider.0.lock().unwrap();
        assert_eq!(1, state.customers.len());
        assert_eq!(1, state.sessions.len());
        assert_eq!(2, state.customer_calls);
        assert_eq!(2, state.session_calls);
    }
    // An unresolved operation outside the provider key horizon never invokes a provider again.
    let another = owner(&pool).await;
    let ctx = context(another);
    let failing = Provider::default();
    failing.0.lock().unwrap().lose_customer = true;
    assert!(
        handler(&pool, failing.clone(), "price", unit.clone())
            .execute(&ctx, command())
            .await
            .is_err()
    );
    sqlx::query("UPDATE external_command_operations SET admitted_at=clock_timestamp()-interval '24 hours',lease_until=clock_timestamp()-interval '1 second' WHERE private_owner=$1").bind(another.into_uuid()).execute(&pool).await.unwrap();
    assert!(matches!(
        handler(&pool, failing.clone(), "price", unit)
            .execute(&ctx, command())
            .await,
        Err(CreateBillingCheckoutSessionError::Receipt(
            CommandReceiptError::ResultUnavailable
        ))
    ));
    assert_eq!(1, failing.0.lock().unwrap().customer_calls);
}

#[aura_integration_test(services=[SCHEMA])]
async fn cleanup_preserves_unresolved_evidence_and_expired_attempts_cannot_finalize() {
    use application::external_operation::{ExternalOperations, TransactionalExternalOperations};

    let pool = get_postgres_client().await;
    let user = owner(&pool).await;
    let unit = SqlxUnitOfWork::new(pool.clone());
    let operations = TransactionalExternalOperations(unit.clone());
    let intent = CommandIntent::new(
        &context(user),
        "CREATE_BILLING_CHECKOUT",
        user.to_string(),
        Some(user),
        Value::Null,
    )
    .unwrap();
    let evidence = serde_json::json!({"customer":"cus_reconcile"});
    let claim = operations
        .claim(&intent, evidence.clone(), &format!("BILLING:{user}"), true)
        .await
        .unwrap();
    release_lease(&pool, user).await;
    assert!(matches!(
        operations
            .checkpoint(&intent, claim.attempt, Value::Null, true)
            .await,
        Err(CommandReceiptError::InProgress)
    ));

    sqlx::query("UPDATE external_command_operations SET evidence_expires_at=clock_timestamp()-interval '1 second' WHERE private_owner=$1")
        .bind(user.into_uuid()).execute(&pool).await.unwrap();
    let system = OperationContext {
        command_key: None,
        principal: Principal::System,
        request_id: "cleanup-test".into(),
        correlation_id: "cleanup-test".into(),
    };
    let cleanup = application::command_cleanup::CommandReceiptCleanupHandler(unit);
    assert_eq!((0, 0), cleanup.execute(&system, 1).await.unwrap());
    let retained: Value =
        sqlx::query_scalar("SELECT state FROM external_command_operations WHERE private_owner=$1")
            .bind(user.into_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(evidence, retained);
    assert!(matches!(
        operations
            .claim(&intent, Value::Null, &format!("BILLING:{user}"), true)
            .await,
        Err(CommandReceiptError::ResultUnavailable)
    ));

    // Once reconciliation resolves the operation, cleanup erases the state but retains its fence.
    sqlx::query("UPDATE external_command_operations SET completed=true WHERE private_owner=$1")
        .bind(user.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!((0, 1), cleanup.execute(&system, 1).await.unwrap());
    assert_eq!((0, 0), cleanup.execute(&system, 1).await.unwrap());
    let retained: Option<Value> =
        sqlx::query_scalar("SELECT state FROM external_command_operations WHERE private_owner=$1")
            .bind(user.into_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(retained.is_none());
    assert!(matches!(
        operations
            .claim(&intent, Value::Null, &format!("BILLING:{user}"), true)
            .await,
        Err(CommandReceiptError::ResultUnavailable)
    ));
}
