use application::transaction::{Transaction, UnitOfWork};
use aws_lambda_events::cognito::CognitoEventUserPoolsPostConfirmation;
use cognito_post_confirmation::handler;
use lambda_runtime::{Context, LambdaEvent};
use platform_postgres::{SqlxTransaction, SqlxUnitOfWork};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use user_core::role::UserRole;
use user_core::tier::UserTier;
use user_core::user_id::UserId;
use user_postgres::SqlxUserRepositoryFactory;
use user_service::ports::{UserRepository, UserRepositoryFactory};
use user_service::use_cases::CreateUserHandler;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_create_default_postgres_user_when_cognito_confirms_signup() {
    let user_id = UserId::new();
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let users = SqlxUserRepositoryFactory::new();
    let service = CreateUserHandler::new(unit_of_work.clone(), users);

    let response = match handler(
        post_confirmation_event(user_id, "ada@example.com"),
        &service,
    )
    .await
    {
        Ok(response) => response,
        Err(error) => panic!("handler failed: {error}"),
    };
    let mut tx = begin(&unit_of_work).await;
    let stored = match SqlxUserRepositoryFactory::new()
        .in_transaction(&mut tx)
        .find_by_id(user_id)
        .await
    {
        Ok(Some(user)) => user,
        Ok(None) => panic!("created user missing from Postgres"),
        Err(error) => panic!("failed to read created user: {error:?}"),
    };
    commit(tx).await;

    assert_eq!(user_id.to_string(), response.request.user_attributes["sub"]);
    assert_eq!("ada@example.com", stored.value.email().to_string());
    assert_eq!(UserTier::Free, stored.value.account().tier);
    assert_eq!(UserRole::User, stored.value.account().role);
    assert!(stored.value.profile().first_name.is_none());
    assert!(stored.value.preferences().language.is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_be_idempotent_when_cognito_redelivers_same_confirmation() {
    let user_id = UserId::new();
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let service = CreateUserHandler::new(unit_of_work.clone(), SqlxUserRepositoryFactory::new());

    for _ in 0..2 {
        if let Err(error) = handler(
            post_confirmation_event(user_id, "ada@example.com"),
            &service,
        )
        .await
        {
            panic!("handler failed on Cognito redelivery: {error}");
        }
    }

    let mut tx = begin(&unit_of_work).await;
    let user = match SqlxUserRepositoryFactory::new()
        .in_transaction(&mut tx)
        .find_by_id(user_id)
        .await
    {
        Ok(Some(user)) => user,
        Ok(None) => panic!("redelivered user missing from Postgres"),
        Err(error) => panic!("failed to read redelivered user: {error:?}"),
    };
    commit(tx).await;

    assert_eq!("ada@example.com", user.value.email().to_string());
    assert_eq!(1, user.version.into_inner());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_not_mutate_user_when_cognito_redelivery_has_different_email() {
    let user_id = UserId::new();
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let service = CreateUserHandler::new(unit_of_work.clone(), SqlxUserRepositoryFactory::new());

    if let Err(error) = handler(
        post_confirmation_event(user_id, "ada@example.com"),
        &service,
    )
    .await
    {
        panic!("initial handler call failed: {error}");
    }
    assert!(
        handler(
            post_confirmation_event(user_id, "grace@example.com"),
            &service
        )
        .await
        .is_err()
    );

    let mut tx = begin(&unit_of_work).await;
    let user = match SqlxUserRepositoryFactory::new()
        .in_transaction(&mut tx)
        .find_by_id(user_id)
        .await
    {
        Ok(Some(user)) => user,
        Ok(None) => panic!("initial user missing from Postgres"),
        Err(error) => panic!("failed to read user: {error:?}"),
    };
    commit(tx).await;

    assert_eq!("ada@example.com", user.value.email().to_string());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_reject_different_cognito_user_with_existing_email() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let service = CreateUserHandler::new(unit_of_work.clone(), SqlxUserRepositoryFactory::new());
    let first_user_id = UserId::new();
    let second_user_id = UserId::new();

    if let Err(error) = handler(
        post_confirmation_event(first_user_id, "ada@example.com"),
        &service,
    )
    .await
    {
        panic!("initial handler call failed: {error}");
    }
    assert!(
        handler(
            post_confirmation_event(second_user_id, "ada@example.com"),
            &service
        )
        .await
        .is_err()
    );

    let mut tx = begin(&unit_of_work).await;
    let second_user = match SqlxUserRepositoryFactory::new()
        .in_transaction(&mut tx)
        .find_by_id(second_user_id)
        .await
    {
        Ok(user) => user,
        Err(error) => panic!("failed to read conflicting user: {error:?}"),
    };
    commit(tx).await;

    assert!(second_user.is_none());
}

fn post_confirmation_event(
    user_id: UserId,
    email: &str,
) -> LambdaEvent<CognitoEventUserPoolsPostConfirmation> {
    let payload = match serde_json::from_value(serde_json::json!({
        "version": "1",
        "triggerSource": "PostConfirmation_ConfirmSignUp",
        "region": "eu-central-1",
        "userPoolId": "pool-id",
        "userName": user_id.to_string(),
        "callerContext": {},
        "request": {
            "userAttributes": {
                "sub": user_id.to_string(),
                "email": email
            },
            "clientMetadata": {}
        },
        "response": {}
    })) {
        Ok(payload) => payload,
        Err(error) => panic!("invalid test Cognito event: {error}"),
    };
    let mut context = Context::default();
    context.request_id = "lambda-request-id".to_owned();

    LambdaEvent { payload, context }
}

async fn begin(unit_of_work: &SqlxUnitOfWork) -> SqlxTransaction {
    match unit_of_work.begin().await {
        Ok(tx) => tx,
        Err(error) => panic!("failed to begin transaction: {error}"),
    }
}

async fn commit(tx: SqlxTransaction) {
    if let Err(error) = tx.commit().await {
        panic!("failed to commit transaction: {error}");
    }
}
