use async_trait::async_trait;
use common::error::boxed::static_error;
use common::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use common::postgres::SqlxUnitOfWork;
use common::transaction::{Transaction, UnitOfWork};
use common::{
    partner_shop_application_id::PartnerShopApplicationId, shop_id::ShopId, shop_name::ShopName,
    user_id::UserId,
};
use notification_service::ports::notification_creator::NotificationCreationOutcome;
use notification_service::use_cases::commands::create_notifications::{
    CreateNotificationsCommand, CreateNotificationsError, CreateNotificationsResult,
    CreateNotificationsUseCase,
};

use shop_core::partner_status::ShopPartnerStatus;
use shop_core::shop::{NewShop, Shop, ShopContact, ShopPresentation};
use shop_core::shop_type::ShopType;
use shop_partner_core::partner_shop_application::{
    NewPartnerShopApplication, PartnerShopApplication, PartnerShopApplicationPayload,
};
use shop_partner_postgres::{
    SqlxPartnerShopApplicationRepositoryFactory, SqlxUserPartnerShopMembershipRepositoryFactory,
};
use shop_partner_service::ports::{
    PartnerShopApplicationRepository, PartnerShopApplicationRepositoryFactory,
    UserPartnerShopMembershipRepository, UserPartnerShopMembershipRepositoryError,
    UserPartnerShopMembershipRepositoryFactory,
};
use shop_partner_service::use_cases::{
    AdminDecidePartnerShopApplicationCommand, AdminDecidePartnerShopApplicationHandler,
    AdminDecidePartnerShopApplicationUseCase, AdminMarkPartnerShopApplicationInReviewCommand,
    AdminUpdatePartnerShopApplicationHandler, AdminUpdatePartnerShopApplicationUseCase,
    PartnerShopApplicationDecision, WithdrawPartnerShopApplicationCommand,
    WithdrawPartnerShopApplicationHandler, WithdrawPartnerShopApplicationUseCase,
};
use shop_postgres::SqlxShopRepositoryFactory;
use shop_service::ports::{ShopRepository, ShopRepositoryFactory};
use std::collections::HashSet;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use user_postgres::SqlxUserAdminReaderFactory;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[derive(Clone, Copy, Default)]
struct FakeNotificationService;

#[derive(Clone, Copy, Default)]
struct FailingMembershipFactory;

struct FailingMembershipRepository;

impl UserPartnerShopMembershipRepositoryFactory<common::postgres::SqlxTransaction>
    for FailingMembershipFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        _: &'tx mut common::postgres::SqlxTransaction,
    ) -> impl UserPartnerShopMembershipRepository + 'tx {
        FailingMembershipRepository
    }
}

#[async_trait]
impl UserPartnerShopMembershipRepository for FailingMembershipRepository {
    async fn grant(
        &mut self,
        _: UserId,
        _: ShopId,
    ) -> Result<(), UserPartnerShopMembershipRepositoryError> {
        Err(UserPartnerShopMembershipRepositoryError::Internal {
            source: static_error("forced membership write failure"),
        })
    }
}

#[async_trait]
impl CreateNotificationsUseCase for FakeNotificationService {
    async fn execute(
        &self,
        command: CreateNotificationsCommand,
    ) -> Result<CreateNotificationsResult, CreateNotificationsError> {
        Ok(CreateNotificationsResult {
            outcomes: command
                .intents
                .into_iter()
                .map(|_| NotificationCreationOutcome::Duplicate)
                .collect(),
        })
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_approve_new_application_publish_shop_and_grant_membership() {
    let pool = get_postgres_client().await;
    let user_id = seed_user(&pool, "approve").await;
    let shop_id = seed_drafted_shop(&pool, "approve").await;
    let application = seed_new_application(&pool, user_id, shop_id).await;

    mark_in_review(&pool, application.id()).await;
    let result = decide(
        &pool,
        application.id(),
        PartnerShopApplicationDecision::Approve,
    )
    .await;

    assert!(result.is_ok(), "approval failed: {result:?}");
    assert_eq!(
        "APPROVED",
        application_business_state(&pool, application.id()).await
    );
    assert_eq!(
        ("PARTNERED".to_owned(), "PUBLISHED".to_owned()),
        shop_state(&pool, shop_id).await
    );
    assert!(membership_exists(&pool, user_id, shop_id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_approve_existing_application_publish_shop_and_grant_membership() {
    let pool = get_postgres_client().await;
    let user_id = seed_user(&pool, "approve-existing").await;
    let shop_id = seed_drafted_shop(&pool, "approve-existing").await;
    let application = seed_existing_application(&pool, user_id, shop_id).await;

    mark_in_review(&pool, application.id()).await;
    let result = decide(
        &pool,
        application.id(),
        PartnerShopApplicationDecision::Approve,
    )
    .await;

    assert!(result.is_ok(), "approval failed: {result:?}");
    assert_eq!(
        "APPROVED",
        application_business_state(&pool, application.id()).await
    );
    assert_eq!(
        ("PARTNERED".to_owned(), "PUBLISHED".to_owned()),
        shop_state(&pool, shop_id).await
    );
    assert!(membership_exists(&pool, user_id, shop_id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_rollback_approval_when_membership_grant_fails() {
    let pool = get_postgres_client().await;
    let user_id = seed_user(&pool, "rollback").await;
    let shop_id = seed_drafted_shop(&pool, "rollback").await;
    let application = seed_new_application(&pool, user_id, shop_id).await;
    mark_in_review(&pool, application.id()).await;

    let result = AdminDecidePartnerShopApplicationHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxPartnerShopApplicationRepositoryFactory::new(),
        SqlxShopRepositoryFactory::new(),
        FailingMembershipFactory,
        SqlxUserAdminReaderFactory::new(),
        FakeNotificationService,
    )
    .execute(
        &system_context(),
        AdminDecidePartnerShopApplicationCommand {
            application_id: application.id(),
            decision: PartnerShopApplicationDecision::Approve,
        },
    )
    .await;

    assert!(result.is_err(), "approval unexpectedly succeeded");
    assert_eq!(
        "IN_REVIEW",
        application_business_state(&pool, application.id()).await
    );
    assert_eq!(
        ("SCRAPED".to_owned(), "DRAFTED".to_owned()),
        shop_state(&pool, shop_id).await
    );
    assert!(!membership_exists(&pool, user_id, shop_id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_reject_new_application_and_discard_shop() {
    let pool = get_postgres_client().await;
    let user_id = seed_user(&pool, "reject").await;
    let shop_id = seed_drafted_shop(&pool, "reject").await;
    let application = seed_new_application(&pool, user_id, shop_id).await;

    mark_in_review(&pool, application.id()).await;
    let result = decide(
        &pool,
        application.id(),
        PartnerShopApplicationDecision::Reject,
    )
    .await;

    assert!(result.is_ok(), "rejection failed: {result:?}");
    assert_eq!(
        "REJECTED",
        application_business_state(&pool, application.id()).await
    );
    assert_eq!(
        ("SCRAPED".to_owned(), "DISCARDED".to_owned()),
        shop_state(&pool, shop_id).await
    );
    assert!(!membership_exists(&pool, user_id, shop_id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_reject_existing_application_without_changing_shop() {
    let pool = get_postgres_client().await;
    let user_id = seed_user(&pool, "reject-existing").await;
    let shop_id = seed_drafted_shop(&pool, "reject-existing").await;
    let application = seed_existing_application(&pool, user_id, shop_id).await;

    mark_in_review(&pool, application.id()).await;
    let result = decide(
        &pool,
        application.id(),
        PartnerShopApplicationDecision::Reject,
    )
    .await;

    assert!(result.is_ok(), "rejection failed: {result:?}");
    assert_eq!(
        "REJECTED",
        application_business_state(&pool, application.id()).await
    );
    assert_eq!(
        ("SCRAPED".to_owned(), "DRAFTED".to_owned()),
        shop_state(&pool, shop_id).await
    );
    assert!(!membership_exists(&pool, user_id, shop_id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_withdraw_new_application_and_discard_shop() {
    let pool = get_postgres_client().await;
    let user_id = seed_user(&pool, "withdraw").await;
    let shop_id = seed_drafted_shop(&pool, "withdraw").await;
    let application = seed_new_application(&pool, user_id, shop_id).await;

    let result = WithdrawPartnerShopApplicationHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxPartnerShopApplicationRepositoryFactory::new(),
        SqlxShopRepositoryFactory::new(),
    )
    .execute(
        &system_context(),
        WithdrawPartnerShopApplicationCommand {
            user_id,
            application_id: application.id(),
        },
    )
    .await;

    assert!(result.is_ok(), "withdrawal failed: {result:?}");
    assert_eq!(
        "WITHDRAWN",
        application_business_state(&pool, application.id()).await
    );
    assert_eq!(
        ("SCRAPED".to_owned(), "DISCARDED".to_owned()),
        shop_state(&pool, shop_id).await
    );
    assert!(!membership_exists(&pool, user_id, shop_id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_allow_only_one_of_concurrent_opposite_decisions() {
    let pool = get_postgres_client().await;
    let user_id = seed_user(&pool, "concurrent").await;
    let shop_id = seed_drafted_shop(&pool, "concurrent").await;
    let application = seed_new_application(&pool, user_id, shop_id).await;
    mark_in_review(&pool, application.id()).await;

    let approve_pool = pool.clone();
    let reject_pool = pool.clone();
    let application_id = application.id();
    let (approve, reject) = tokio::join!(
        decide(
            &approve_pool,
            application_id,
            PartnerShopApplicationDecision::Approve
        ),
        decide(
            &reject_pool,
            application_id,
            PartnerShopApplicationDecision::Reject
        )
    );

    assert!(
        approve.is_ok() || reject.is_ok(),
        "both decisions failed: {approve:?}, {reject:?}"
    );
    assert_eq!(
        1,
        usize::from(approve.is_ok()) + usize::from(reject.is_ok()),
        "opposite decisions must not both commit"
    );
    let state = application_business_state(&pool, application.id()).await;
    assert!(matches!(state.as_str(), "APPROVED" | "REJECTED"));
}

async fn decide(
    pool: &sqlx::PgPool,
    application_id: PartnerShopApplicationId,
    decision: PartnerShopApplicationDecision,
) -> Result<
    shop_partner_service::use_cases::AdminDecidePartnerShopApplicationResult,
    shop_partner_service::use_cases::AdminDecidePartnerShopApplicationError,
> {
    AdminDecidePartnerShopApplicationHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxPartnerShopApplicationRepositoryFactory::new(),
        SqlxShopRepositoryFactory::new(),
        SqlxUserPartnerShopMembershipRepositoryFactory::new(),
        SqlxUserAdminReaderFactory::new(),
        FakeNotificationService,
    )
    .execute(
        &system_context(),
        AdminDecidePartnerShopApplicationCommand {
            application_id,
            decision,
        },
    )
    .await
}

async fn mark_in_review(pool: &sqlx::PgPool, application_id: PartnerShopApplicationId) {
    let result = AdminUpdatePartnerShopApplicationHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxPartnerShopApplicationRepositoryFactory::new(),
        SqlxUserAdminReaderFactory::new(),
    )
    .mark_in_review(
        &system_context(),
        AdminMarkPartnerShopApplicationInReviewCommand { application_id },
    )
    .await;

    assert!(
        result.is_ok(),
        "marking application in review failed: {result:?}"
    );
}

async fn seed_new_application(
    pool: &sqlx::PgPool,
    user_id: UserId,
    shop_id: ShopId,
) -> PartnerShopApplication {
    seed_application(
        pool,
        user_id,
        PartnerShopApplicationPayload::New { shop_id },
    )
    .await
}

async fn seed_existing_application(
    pool: &sqlx::PgPool,
    user_id: UserId,
    shop_id: ShopId,
) -> PartnerShopApplication {
    seed_application(
        pool,
        user_id,
        PartnerShopApplicationPayload::Existing { shop_id },
    )
    .await
}

async fn seed_application(
    pool: &sqlx::PgPool,
    user_id: UserId,
    payload: PartnerShopApplicationPayload,
) -> PartnerShopApplication {
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let applications = SqlxPartnerShopApplicationRepositoryFactory::new();
    let application = PartnerShopApplication::create(NewPartnerShopApplication {
        id: PartnerShopApplicationId::new(),
        applicant_user_id: user_id,
        payload,
    });
    let mut transaction = match unit_of_work.begin().await {
        Ok(transaction) => transaction,
        Err(error) => panic!("failed to begin application seed transaction: {error}"),
    };
    let persisted = match applications
        .in_transaction(&mut transaction)
        .insert(&application)
        .await
    {
        Ok(persisted) => persisted,
        Err(error) => panic!("failed to seed partner application: {error:?}"),
    };
    if let Err(error) = transaction.commit().await {
        panic!("failed to commit application seed transaction: {error}");
    }

    persisted.value
}

async fn seed_user(pool: &sqlx::PgPool, flow: &str) -> UserId {
    let user_id = UserId::new();
    let result = sqlx::query(
        "INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'FREE', 'USER')",
    )
    .bind(uuid::Uuid::from(user_id))
    .bind(format!(
        "partner-application-flow-{flow}-{user_id}@example.test"
    ))
    .execute(pool)
    .await;

    if let Err(error) = result {
        panic!("failed to seed user: {error}");
    }

    user_id
}

async fn seed_drafted_shop(pool: &sqlx::PgPool, flow: &str) -> ShopId {
    let shop = Shop::create(NewShop {
        id: ShopId::new(),
        name: ShopName::from(format!("Partner application flow {flow}").as_str()),
        shop_type: ShopType::CommercialDealer,
        domains: HashSet::from([domain(format!("{flow}.example").as_str())]),
        shopify: None,
        woocommerce: None,
        presentation: ShopPresentation::default(),
        address: None,
        contact: ShopContact::default(),
        partner_status: ShopPartnerStatus::Scraped,
        affiliate_configuration: None,
    });
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let shops = SqlxShopRepositoryFactory::new();
    let mut transaction = match unit_of_work.begin().await {
        Ok(transaction) => transaction,
        Err(error) => panic!("failed to begin shop seed transaction: {error}"),
    };
    if let Err(error) = shops.in_transaction(&mut transaction).insert(&shop).await {
        panic!("failed to seed drafted shop: {error:?}");
    }
    if let Err(error) = transaction.commit().await {
        panic!("failed to commit shop seed transaction: {error}");
    }

    shop.id()
}

async fn application_business_state(
    pool: &sqlx::PgPool,
    application_id: PartnerShopApplicationId,
) -> String {
    match sqlx::query_scalar(
        "SELECT business_state FROM partner_shop_applications WHERE partner_shop_application_id = $1::uuid",
    )
    .bind(application_id.to_string())
    .fetch_one(pool)
    .await
    {
        Ok(state) => state,
        Err(error) => panic!("failed to read partner application state: {error}"),
    }
}

async fn shop_state(pool: &sqlx::PgPool, shop_id: ShopId) -> (String, String) {
    match sqlx::query_as("SELECT partner_status, lifecycle FROM shops WHERE shop_id = $1")
        .bind(uuid::Uuid::from(shop_id))
        .fetch_one(pool)
        .await
    {
        Ok(state) => state,
        Err(error) => panic!("failed to read shop state: {error}"),
    }
}

async fn membership_exists(pool: &sqlx::PgPool, user_id: UserId, shop_id: ShopId) -> bool {
    match sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM user_partner_shops WHERE user_id = $1 AND shop_id = $2)",
    )
    .bind(uuid::Uuid::from(user_id))
    .bind(uuid::Uuid::from(shop_id))
    .fetch_one(pool)
    .await
    {
        Ok(exists) => exists,
        Err(error) => panic!("failed to read partner shop membership: {error}"),
    }
}

fn domain(value: &str) -> common::domain::Domain {
    match common::domain::Domain::try_from(value) {
        Ok(domain) => domain,
        Err(error) => panic!("invalid test domain: {error}"),
    }
}

fn system_context() -> OperationContext {
    OperationContext {
        principal: Principal::System,
        request_id: RequestId::from("partner-shop-application-flow-test"),
        correlation_id: CorrelationId::from("partner-shop-application-flow-test"),
    }
}
