use application::transaction::{Transaction, UnitOfWork};
use aws_lambda_events::eventbridge::EventBridgeEvent;
use lambda_runtime::{Context, LambdaEvent};
use platform_postgres::SqlxUnitOfWork;
use serde_email::Email;
use serde_json::{Value, json};
use sqlx::Row;
use std::sync::{Arc, Mutex};
use stripe_lambda::{
    STRIPE_EVENT_TYPE_SUBSCRIPTION_CREATED, STRIPE_EVENT_TYPE_SUBSCRIPTION_DELETED,
    STRIPE_EVENT_TYPE_SUBSCRIPTION_UPDATED, handler,
};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use user_core::{
    role::UserRole,
    stripe_customer_id::StripeCustomerId,
    tier::UserTier,
    user::{NewUser, User, UserAccount, UserPreferences, UserProfile},
    user_id::UserId,
};
use user_postgres::{
    SqlxStripeSubscriptionEventStoreFactory, SqlxUserRepositoryFactory,
    SqlxUserTierEntitlementsFactory,
};
use user_service::ports::stripe_subscription_sync::*;
use user_service::ports::{UserRepository, UserRepositoryFactory};
use user_service::use_cases::{ApplyStripeSubscriptionHandler, ApplyStripeSubscriptionUseCase};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[derive(Clone)]
struct Provider(Arc<Mutex<StripeCustomerSubscriptionState>>);
#[async_trait::async_trait]
impl StripeCustomerSubscriptionReader for Provider {
    async fn read(
        &self,
        _: &StripeCustomerId,
    ) -> Result<StripeCustomerSubscriptionState, StripeSubscriptionSyncError> {
        Ok(self.0.lock().unwrap().clone())
    }
}
fn subscriptions(pool: sqlx::PgPool, provider: Provider) -> impl ApplyStripeSubscriptionUseCase {
    ApplyStripeSubscriptionHandler::new(
        SqlxUnitOfWork::new(pool),
        SqlxUserRepositoryFactory::new(),
        SqlxUserTierEntitlementsFactory::new(),
        SqlxStripeSubscriptionEventStoreFactory,
        provider,
    )
}
fn provider(user_id: Option<UserId>, tier: UserTier) -> Provider {
    Provider(Arc::new(Mutex::new(StripeCustomerSubscriptionState {
        user_id,
        tier,
    })))
}
fn event(id: &str, kind: &str) -> LambdaEvent<EventBridgeEvent<Value>> {
    let mut envelope = EventBridgeEvent::default();
    envelope.id = Some(format!("delivery-{}", UserId::new()));
    envelope.source = "aws.partner/stripe.com/test".to_owned();
    envelope.detail =
        json!({"id": id, "type": kind, "data": {"object": {"id": "sub_1", "customer": "cus_1"}}});
    LambdaEvent::new(envelope, Context::default())
}
async fn seed_user(pool: &sqlx::PgPool, tier: UserTier, customer: Option<&str>) -> UserId {
    let user_id = UserId::new();
    let email = match Email::try_from(format!("stripe-{}@example.com", user_id)) {
        Ok(email) => email,
        Err(error) => panic!("test email must be valid: {error}"),
    };
    let user = match User::create(NewUser {
        id: user_id,
        email,
        profile: UserProfile::default(),
        preferences: UserPreferences::default(),
        account: UserAccount {
            tier,
            role: UserRole::User,
            stripe_customer_id: customer.map(Into::into),
        },
    }) {
        Ok(user) => user,
        Err(error) => panic!("test user must be valid: {error}"),
    };
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let users = SqlxUserRepositoryFactory::new();
    let mut transaction = match UnitOfWork::begin(&unit_of_work).await {
        Ok(transaction) => transaction,
        Err(error) => panic!("failed to begin test transaction: {error}"),
    };
    if let Err(error) = users.in_transaction(&mut transaction).insert(&user).await {
        panic!("failed to seed user: {error}");
    }
    if let Err(error) = Transaction::commit(transaction).await {
        panic!("failed to commit seeded user: {error}");
    }
    user_id
}

async fn account(pool: &sqlx::PgPool, user_id: UserId) -> (String, Option<String>, i64) {
    let row =
        match sqlx::query("SELECT tier, stripe_customer_id, version FROM users WHERE user_id = $1")
            .bind(uuid::Uuid::from(user_id))
            .fetch_one(pool)
            .await
        {
            Ok(row) => row,
            Err(error) => panic!("failed to load user account: {error}"),
        };
    (
        row.get("tier"),
        row.get("stripe_customer_id"),
        row.get("version"),
    )
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn associates_customer_and_records_receipt_with_tier_change() {
    let pool = get_postgres_client().await;
    let user_id = seed_user(&pool, UserTier::Free, None).await;
    let service = subscriptions(pool.clone(), provider(Some(user_id), UserTier::Pro));
    handler(
        event("evt_created", STRIPE_EVENT_TYPE_SUBSCRIPTION_CREATED),
        &service,
    )
    .await
    .unwrap();
    let actual = account(&pool, user_id).await;
    assert_eq!("PRO", actual.0);
    assert_eq!(Some("cus_1".to_owned()), actual.1);
    let receipts: i64 =
        sqlx::query_scalar("SELECT count(*) FROM stripe_subscription_event_receipts")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(1, receipts);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn delayed_events_and_replaced_subscription_deletion_use_current_state() {
    let pool = get_postgres_client().await;
    let user_id = seed_user(&pool, UserTier::Pro, Some("cus_1")).await;
    let state = provider(Some(user_id), UserTier::Free);
    let service = subscriptions(pool.clone(), state.clone());
    handler(
        event("evt_deleted", STRIPE_EVENT_TYPE_SUBSCRIPTION_DELETED),
        &service,
    )
    .await
    .unwrap();
    handler(
        event("evt_oldUpdate", STRIPE_EVENT_TYPE_SUBSCRIPTION_UPDATED),
        &service,
    )
    .await
    .unwrap();
    assert_eq!("FREE", account(&pool, user_id).await.0);
    state.0.lock().unwrap().tier = UserTier::Ultimate;
    handler(
        event(
            "evt_newSubscription",
            STRIPE_EVENT_TYPE_SUBSCRIPTION_CREATED,
        ),
        &service,
    )
    .await
    .unwrap();
    handler(
        event("evt_oldDeletion", STRIPE_EVENT_TYPE_SUBSCRIPTION_DELETED),
        &service,
    )
    .await
    .unwrap();
    assert_eq!("ULTIMATE", account(&pool, user_id).await.0);
    let before = account(&pool, user_id).await;
    state.0.lock().unwrap().tier = UserTier::Free;
    handler(
        event("evt_deleted", STRIPE_EVENT_TYPE_SUBSCRIPTION_DELETED),
        &service,
    )
    .await
    .unwrap();
    assert_eq!(before, account(&pool, user_id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn event_identity_conflicts_fail_without_mutating_user() {
    let pool = get_postgres_client().await;
    let user_id = seed_user(&pool, UserTier::Free, Some("cus_1")).await;
    let service = subscriptions(pool.clone(), provider(Some(user_id), UserTier::Pro));
    handler(
        event("evt_reused", STRIPE_EVENT_TYPE_SUBSCRIPTION_CREATED),
        &service,
    )
    .await
    .unwrap();
    let before = account(&pool, user_id).await;
    assert!(
        handler(
            event("evt_reused", STRIPE_EVENT_TYPE_SUBSCRIPTION_DELETED),
            &service
        )
        .await
        .is_err()
    );
    assert_eq!(before, account(&pool, user_id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn failed_application_does_not_consume_receipt() {
    let pool = get_postgres_client().await;
    let state = provider(None, UserTier::Pro);
    let service = subscriptions(pool.clone(), state.clone());
    assert!(
        handler(
            event("evt_retry", STRIPE_EVENT_TYPE_SUBSCRIPTION_CREATED),
            &service
        )
        .await
        .is_err()
    );
    let receipts: i64 =
        sqlx::query_scalar("SELECT count(*) FROM stripe_subscription_event_receipts")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(0, receipts);
    let user_id = seed_user(&pool, UserTier::Free, None).await;
    state.0.lock().unwrap().user_id = Some(user_id);
    handler(
        event("evt_retry", STRIPE_EVENT_TYPE_SUBSCRIPTION_CREATED),
        &service,
    )
    .await
    .unwrap();
    assert_eq!("PRO", account(&pool, user_id).await.0);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn unchanged_tier_still_fences_an_older_in_flight_provider_read() {
    let pool = get_postgres_client().await;
    let user_id = seed_user(&pool, UserTier::Pro, Some("cus_1")).await;
    let uow = SqlxUnitOfWork::new(pool.clone());
    let factory = SqlxStripeSubscriptionEventStoreFactory;
    let older = StripeSubscriptionEvent {
        event_id: "evt_older".to_owned(),
        subscription_id: "sub_1".to_owned(),
        customer_id: "cus_1".into(),
        fingerprint: [1; 32],
    };
    let mut tx = uow.begin().await.unwrap();
    let observed = factory
        .in_transaction(&mut tx)
        .observe(&older)
        .await
        .unwrap();
    let StripeSubscriptionEventObservation::Pending { revision } = observed else {
        panic!("new event");
    };
    tx.commit().await.unwrap();
    let before = account(&pool, user_id).await;
    let service = subscriptions(pool.clone(), provider(Some(user_id), UserTier::Pro));
    handler(
        event("evt_newer", STRIPE_EVENT_TYPE_SUBSCRIPTION_UPDATED),
        &service,
    )
    .await
    .unwrap();
    assert_eq!(before, account(&pool, user_id).await);
    let mut tx = uow.begin().await.unwrap();
    assert!(matches!(
        factory
            .in_transaction(&mut tx)
            .complete(&older, revision)
            .await,
        Err(StripeSubscriptionSyncError::ConcurrentReconciliation)
    ));
}
