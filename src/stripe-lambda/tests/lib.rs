use application::transaction::{Transaction, UnitOfWork};
use aws_lambda_events::eventbridge::EventBridgeEvent;
use lambda_runtime::{Context, LambdaEvent};
use platform_postgres::SqlxUnitOfWork;
use serde_email::Email;
use serde_json::{Value, json};
use sqlx::Row;
use stripe_lambda::{
    STRIPE_EVENT_TYPE_SUBSCRIPTION_CREATED, STRIPE_EVENT_TYPE_SUBSCRIPTION_DELETED,
    STRIPE_EVENT_TYPE_SUBSCRIPTION_UPDATED, StripeProductTierMap, handler,
};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use user_core::role::UserRole;
use user_core::tier::UserTier;
use user_core::user::{NewUser, User, UserAccount, UserPreferences, UserProfile};
use user_core::user_id::UserId;
use user_postgres::{SqlxUserRepositoryFactory, SqlxUserTierEntitlementsFactory};
use user_service::ports::{UserRepository, UserRepositoryFactory};
use user_service::use_cases::ApplyStripeSubscriptionHandler;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

fn tier_map() -> StripeProductTierMap {
    StripeProductTierMap {
        pro_product_id: "prod_pro".to_owned(),
        ultimate_product_id: "prod_ultimate".to_owned(),
    }
}

fn subscriptions(
    pool: sqlx::PgPool,
) -> ApplyStripeSubscriptionHandler<
    SqlxUnitOfWork,
    SqlxUserRepositoryFactory,
    SqlxUserTierEntitlementsFactory,
> {
    ApplyStripeSubscriptionHandler::new(
        SqlxUnitOfWork::new(pool),
        SqlxUserRepositoryFactory::new(),
        SqlxUserTierEntitlementsFactory::new(),
    )
}

fn event(event_type: &str, detail: Value) -> LambdaEvent<EventBridgeEvent<Value>> {
    let mut detail = detail;
    if let Some(object) = detail.as_object_mut() {
        object.insert("type".to_owned(), json!(event_type));
    }
    let mut payload = EventBridgeEvent::default();
    payload.id = Some(format!("event-{}", UserId::new()));
    payload.source = "aws.partner/stripe.com/test".to_owned();
    payload.detail = detail;
    LambdaEvent::new(payload, Context::default())
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
async fn should_apply_created_subscription_to_postgres_user() {
    let pool = get_postgres_client().await;
    let user_id = seed_user(&pool, UserTier::Free, None).await;
    let service = subscriptions(pool.clone());
    let payload = event(
        STRIPE_EVENT_TYPE_SUBSCRIPTION_CREATED,
        json!({"data": {"object": {
            "id": "sub_created", "customer": "cus_created",
            "metadata": {"userId": user_id.to_string()},
            "items": {"data": [{"price": {"product": "prod_pro"}}]}
        }}}),
    );

    if let Err(error) = handler(payload, &service, &tier_map()).await {
        panic!("handler must apply created subscription: {error}");
    }

    assert_eq!(
        ("PRO".to_owned(), Some("cus_created".to_owned()), 2),
        account(&pool, user_id).await
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_apply_updated_and_deleted_subscriptions_to_postgres_user() {
    let pool = get_postgres_client().await;
    let user_id = seed_user(&pool, UserTier::Pro, Some("cus_lifecycle")).await;
    let service = subscriptions(pool.clone());
    let updated = event(
        STRIPE_EVENT_TYPE_SUBSCRIPTION_UPDATED,
        json!({"data": {"object": {
            "id": "sub_updated", "customer": "cus_lifecycle",
            "items": {"data": [{"price": {"product": "prod_ultimate"}}]}
        }}}),
    );
    let deleted = event(
        STRIPE_EVENT_TYPE_SUBSCRIPTION_DELETED,
        json!({"data": {"object": {"id": "sub_deleted", "customer": "cus_lifecycle"}}}),
    );

    if let Err(error) = handler(updated, &service, &tier_map()).await {
        panic!("handler must apply updated subscription: {error}");
    }
    assert_eq!(
        ("ULTIMATE".to_owned(), Some("cus_lifecycle".to_owned()), 2),
        account(&pool, user_id).await
    );

    if let Err(error) = handler(deleted, &service, &tier_map()).await {
        panic!("handler must apply deleted subscription: {error}");
    }
    assert_eq!(
        ("FREE".to_owned(), Some("cus_lifecycle".to_owned()), 3),
        account(&pool, user_id).await
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_not_mutate_postgres_user_for_repeated_or_ignored_events() {
    let pool = get_postgres_client().await;
    let user_id = seed_user(&pool, UserTier::Pro, Some("cus_unchanged")).await;
    let service = subscriptions(pool.clone());
    let repeated = event(
        STRIPE_EVENT_TYPE_SUBSCRIPTION_UPDATED,
        json!({"data": {"object": {
            "customer": "cus_unchanged",
            "items": {"data": [{"price": {"product": "prod_pro"}}]}
        }}}),
    );
    let unknown_product = event(
        STRIPE_EVENT_TYPE_SUBSCRIPTION_UPDATED,
        json!({"data": {"object": {
            "customer": "cus_unchanged",
            "items": {"data": [{"price": {"product": "prod_unknown"}}]}
        }}}),
    );
    let unknown_user = event(
        STRIPE_EVENT_TYPE_SUBSCRIPTION_DELETED,
        json!({"data": {"object": {"customer": "cus_missing"}}}),
    );

    for payload in [repeated, unknown_product, unknown_user] {
        if let Err(error) = handler(payload, &service, &tier_map()).await {
            panic!("ignored Stripe event must not fail: {error}");
        }
    }

    assert_eq!(
        ("PRO".to_owned(), Some("cus_unchanged".to_owned()), 1),
        account(&pool, user_id).await
    );
}
