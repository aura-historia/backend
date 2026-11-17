use crate::BUSINESS_SCHEMA;

use application::transaction::{Transaction, UnitOfWork};
use aura_historia_api::{
    app,
    state::{AppState, StripeWebhooksState},
};
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use billing_stripe::{
    STRIPE_EVENT_TYPE_SUBSCRIPTION_CREATED, STRIPE_EVENT_TYPE_SUBSCRIPTION_DELETED,
    STRIPE_EVENT_TYPE_SUBSCRIPTION_UPDATED, STRIPE_WEBHOOK_API_VERSION,
    StripeSubscriptionWebhookVerifier,
};
use openssl::{hash::MessageDigest, pkey::PKey, sign::Signer};
use platform_postgres::SqlxUnitOfWork;
use serde_email::Email;
use serde_json::json;
use sqlx::Row;
use std::sync::{Arc, Mutex};
use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};
use tower::ServiceExt;
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
use user_service::use_cases::ApplyStripeSubscriptionHandler;

const SECRET: &str = "whsec_test_stripe_webhook";
const PATH: &str = "/v1/webhooks/stripe";

#[derive(Clone)]
struct Provider(Arc<Mutex<StripeCustomerSubscriptionState>>);

#[async_trait::async_trait]
impl StripeCustomerSubscriptionReader for Provider {
    async fn read(
        &self,
        _: &StripeCustomerId,
    ) -> Result<StripeCustomerSubscriptionState, StripeSubscriptionSyncError> {
        Ok(self
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone())
    }
}

impl Provider {
    fn new(user_id: Option<UserId>, tier: UserTier) -> Self {
        Self(Arc::new(Mutex::new(StripeCustomerSubscriptionState {
            user_id,
            tier,
        })))
    }

    fn set(&self, change: impl FnOnce(&mut StripeCustomerSubscriptionState)) {
        change(&mut self.0.lock().unwrap_or_else(|error| error.into_inner()));
    }
}

fn webhook_app(pool: sqlx::PgPool, provider: Provider) -> Router {
    app(
        AppState::new().with_stripe_webhooks(StripeWebhooksState::new(
            Arc::new(StripeSubscriptionWebhookVerifier::new(SECRET).expect("signing secret")),
            Arc::new(ApplyStripeSubscriptionHandler::new(
                SqlxUnitOfWork::new(pool),
                SqlxUserRepositoryFactory::new(),
                SqlxUserTierEntitlementsFactory::new(),
                SqlxStripeSubscriptionEventStoreFactory,
                provider,
            )),
        )),
    )
}

fn signed_event(id: &str, kind: &str) -> Request<Body> {
    let body = serde_json::to_vec(&json!({
        "id": id,
        "object": "event",
        "api_version": STRIPE_WEBHOOK_API_VERSION,
        "type": kind,
        "data": {"object": {"id": "sub_1", "object": "subscription", "customer": "cus_1"}},
    }))
    .expect("event JSON");
    let timestamp = time::OffsetDateTime::now_utc().unix_timestamp();
    let key = PKey::hmac(SECRET.as_bytes()).expect("HMAC key");
    let mut signer = Signer::new(MessageDigest::sha256(), &key).expect("HMAC signer");
    signer
        .update(format!("{timestamp}.").as_bytes())
        .expect("timestamp");
    signer.update(&body).expect("body");
    let signature: String = signer
        .sign_to_vec()
        .expect("HMAC")
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Request::builder()
        .method("POST")
        .uri(PATH)
        .header("stripe-signature", format!("t={timestamp},v1={signature}"))
        .body(Body::from(body))
        .expect("request")
}

async fn deliver(app: &Router, id: &str, kind: &str) -> StatusCode {
    app.clone()
        .oneshot(signed_event(id, kind))
        .await
        .expect("route response")
        .status()
}

async fn seed_user(pool: &sqlx::PgPool, tier: UserTier, customer: Option<&str>) -> UserId {
    let user_id = UserId::new();
    let email = Email::try_from(format!("stripe-{user_id}@example.com")).expect("test email");
    let user = User::create(NewUser {
        id: user_id,
        email,
        profile: UserProfile::default(),
        preferences: UserPreferences::default(),
        account: UserAccount {
            tier,
            role: UserRole::User,
            stripe_customer_id: customer.map(Into::into),
        },
    })
    .expect("test user");
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let users = SqlxUserRepositoryFactory::new();
    let mut transaction = UnitOfWork::begin(&unit_of_work)
        .await
        .expect("begin seed transaction");
    users
        .in_transaction(&mut transaction)
        .insert(&user)
        .await
        .expect("seed user");
    Transaction::commit(transaction)
        .await
        .expect("commit seeded user");
    user_id
}

async fn account(pool: &sqlx::PgPool, user_id: UserId) -> (String, Option<String>, i64) {
    let row = sqlx::query("SELECT tier, stripe_customer_id, version FROM users WHERE user_id = $1")
        .bind(uuid::Uuid::from(user_id))
        .fetch_one(pool)
        .await
        .expect("user account");
    (
        row.get("tier"),
        row.get("stripe_customer_id"),
        row.get("version"),
    )
}

async fn receipts(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM stripe_subscription_event_receipts")
        .fetch_one(pool)
        .await
        .expect("receipt count")
}

#[aura_integration_test(services = [crate::api_support::API_TEST_STATE, BUSINESS_SCHEMA], isolated = true)]
async fn associates_customer_and_records_receipt_with_tier_change() {
    let pool = get_postgres_client().await;
    let user_id = seed_user(&pool, UserTier::Free, None).await;
    let app = webhook_app(pool.clone(), Provider::new(Some(user_id), UserTier::Pro));

    assert_eq!(
        StatusCode::NO_CONTENT,
        deliver(&app, "evt_created", STRIPE_EVENT_TYPE_SUBSCRIPTION_CREATED).await
    );

    let actual = account(&pool, user_id).await;
    assert_eq!("PRO", actual.0);
    assert_eq!(Some("cus_1".to_owned()), actual.1);
    assert_eq!(1, receipts(&pool).await);
}

#[aura_integration_test(services = [crate::api_support::API_TEST_STATE, BUSINESS_SCHEMA], isolated = true)]
async fn delayed_events_and_replaced_subscription_deletion_use_current_state() {
    let pool = get_postgres_client().await;
    let user_id = seed_user(&pool, UserTier::Pro, Some("cus_1")).await;
    let provider = Provider::new(Some(user_id), UserTier::Free);
    let app = webhook_app(pool.clone(), provider.clone());

    for (id, kind) in [
        ("evt_deleted", STRIPE_EVENT_TYPE_SUBSCRIPTION_DELETED),
        ("evt_oldUpdate", STRIPE_EVENT_TYPE_SUBSCRIPTION_UPDATED),
    ] {
        assert_eq!(StatusCode::NO_CONTENT, deliver(&app, id, kind).await);
    }
    assert_eq!("FREE", account(&pool, user_id).await.0);

    provider.set(|state| state.tier = UserTier::Ultimate);
    for (id, kind) in [
        (
            "evt_newSubscription",
            STRIPE_EVENT_TYPE_SUBSCRIPTION_CREATED,
        ),
        ("evt_oldDeletion", STRIPE_EVENT_TYPE_SUBSCRIPTION_DELETED),
    ] {
        assert_eq!(StatusCode::NO_CONTENT, deliver(&app, id, kind).await);
    }
    assert_eq!("ULTIMATE", account(&pool, user_id).await.0);

    let before = account(&pool, user_id).await;
    provider.set(|state| state.tier = UserTier::Free);
    assert_eq!(
        StatusCode::NO_CONTENT,
        deliver(&app, "evt_deleted", STRIPE_EVENT_TYPE_SUBSCRIPTION_DELETED).await
    );
    assert_eq!(before, account(&pool, user_id).await);
}

#[aura_integration_test(services = [crate::api_support::API_TEST_STATE, BUSINESS_SCHEMA], isolated = true)]
async fn event_identity_conflicts_answer_400_without_mutating_user() {
    let pool = get_postgres_client().await;
    let user_id = seed_user(&pool, UserTier::Free, Some("cus_1")).await;
    let app = webhook_app(pool.clone(), Provider::new(Some(user_id), UserTier::Pro));
    assert_eq!(
        StatusCode::NO_CONTENT,
        deliver(&app, "evt_reused", STRIPE_EVENT_TYPE_SUBSCRIPTION_CREATED).await
    );
    let before = account(&pool, user_id).await;

    assert_eq!(
        StatusCode::BAD_REQUEST,
        deliver(&app, "evt_reused", STRIPE_EVENT_TYPE_SUBSCRIPTION_DELETED).await
    );

    assert_eq!(before, account(&pool, user_id).await);
}

#[aura_integration_test(services = [crate::api_support::API_TEST_STATE, BUSINESS_SCHEMA], isolated = true)]
async fn receipts_from_the_retired_eventbridge_consumer_dedupe_by_identity() {
    let pool = get_postgres_client().await;
    let user_id = seed_user(&pool, UserTier::Free, Some("cus_1")).await;
    // The deployed Lambda inserts without the scheme column and hashed the whole snapshot.
    sqlx::query(
        "INSERT INTO stripe_subscription_reconciliations (stripe_customer_id) VALUES ('cus_1')",
    )
    .execute(&pool)
    .await
    .expect("seed reconciliation");
    sqlx::query("INSERT INTO stripe_subscription_event_receipts (stripe_event_id, stripe_customer_id, stripe_subscription_id, fingerprint) VALUES ('evt_lambda', 'cus_1', $1, $2)")
        .bind("sub_1")
        .bind([7_u8; 32].as_slice())
        .execute(&pool)
        .await
        .expect("seed legacy receipt");
    let before = account(&pool, user_id).await;
    let app = webhook_app(pool.clone(), Provider::new(Some(user_id), UserTier::Pro));

    assert_eq!(
        StatusCode::NO_CONTENT,
        deliver(&app, "evt_lambda", STRIPE_EVENT_TYPE_SUBSCRIPTION_UPDATED).await
    );

    assert_eq!(before, account(&pool, user_id).await);
    assert_eq!(1, receipts(&pool).await);
}

#[aura_integration_test(services = [crate::api_support::API_TEST_STATE, BUSINESS_SCHEMA], isolated = true)]
async fn failed_application_answers_503_and_does_not_consume_receipt() {
    let pool = get_postgres_client().await;
    let provider = Provider::new(None, UserTier::Pro);
    let app = webhook_app(pool.clone(), provider.clone());

    assert_eq!(
        StatusCode::SERVICE_UNAVAILABLE,
        deliver(&app, "evt_retry", STRIPE_EVENT_TYPE_SUBSCRIPTION_CREATED).await
    );
    assert_eq!(0, receipts(&pool).await);

    let user_id = seed_user(&pool, UserTier::Free, None).await;
    provider.set(|state| state.user_id = Some(user_id));
    assert_eq!(
        StatusCode::NO_CONTENT,
        deliver(&app, "evt_retry", STRIPE_EVENT_TYPE_SUBSCRIPTION_CREATED).await
    );
    assert_eq!("PRO", account(&pool, user_id).await.0);
}

#[aura_integration_test(services = [crate::api_support::API_TEST_STATE, BUSINESS_SCHEMA], isolated = true)]
async fn unchanged_tier_still_fences_an_older_in_flight_provider_read() {
    let pool = get_postgres_client().await;
    let user_id = seed_user(&pool, UserTier::Pro, Some("cus_1")).await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let events = SqlxStripeSubscriptionEventStoreFactory;
    let older = StripeSubscriptionEvent {
        event_id: "evt_older".to_owned(),
        subscription_id: "sub_1".to_owned(),
        customer_id: "cus_1".into(),
        fingerprint: [1; 32],
    };
    let mut transaction = unit_of_work.begin().await.expect("begin");
    let StripeSubscriptionEventObservation::Pending { revision } = events
        .in_transaction(&mut transaction)
        .observe(&older)
        .await
        .expect("observe older event")
    else {
        panic!("the older event must be new");
    };
    transaction.commit().await.expect("commit observation");
    let before = account(&pool, user_id).await;
    let app = webhook_app(pool.clone(), Provider::new(Some(user_id), UserTier::Pro));

    assert_eq!(
        StatusCode::NO_CONTENT,
        deliver(&app, "evt_newer", STRIPE_EVENT_TYPE_SUBSCRIPTION_UPDATED).await
    );

    assert_eq!(before, account(&pool, user_id).await);
    let mut transaction = unit_of_work.begin().await.expect("begin");
    assert!(matches!(
        events
            .in_transaction(&mut transaction)
            .complete(&older, revision)
            .await,
        Err(StripeSubscriptionSyncError::ConcurrentReconciliation)
    ));
}
