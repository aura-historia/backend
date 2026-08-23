use application::transaction::{Transaction, UnitOfWork};
use platform_postgres::SqlxUnitOfWork;
use shop_core::shop_id::ShopId;
use shop_partner_core::partner_shop_application::{
    NewPartnerShopApplication, PartnerShopApplication, PartnerShopApplicationPayload,
};
use shop_partner_core::partner_shop_application_id::PartnerShopApplicationId;
use shop_partner_core::partner_shop_application_state::PartnerShopApplicationState;
use shop_partner_postgres::SqlxPartnerShopApplicationRepositoryFactory;
use shop_partner_service::ports::{
    PartnerShopApplicationRepository, PartnerShopApplicationRepositoryError,
    PartnerShopApplicationRepositoryFactory, PartnerShopApplicationStorageVersion,
};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use user_core::user_id::UserId;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_insert_find_update_and_delete_partner_shop_application() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let applications = SqlxPartnerShopApplicationRepositoryFactory::new();
    let user_id = seed_user(&pool, "partner-app-repo-user").await;
    let shop_id = seed_shop(&pool, "partner-app-repo-shop").await;
    let mut application = new_application(user_id, shop_id);

    let mut tx = begin(&unit_of_work).await;
    applications
        .in_transaction(&mut tx)
        .insert(&application)
        .await
        .unwrap_or_else(|error| panic!("failed to insert application: {error:?}"));
    let by_user = applications
        .in_transaction(&mut tx)
        .find_by_user_and_id(user_id, application.id())
        .await
        .unwrap_or_else(|error| panic!("failed to find application by user and id: {error:?}"))
        .unwrap_or_else(|| panic!("missing application by user and id"));
    let by_id = applications
        .in_transaction(&mut tx)
        .find_by_id(application.id())
        .await
        .unwrap_or_else(|error| panic!("failed to find application by id: {error:?}"))
        .unwrap_or_else(|| panic!("missing application by id"));
    assert_eq!(application.id(), by_user.value.id());
    assert_eq!(application.id(), by_id.value.id());

    application
        .mark_in_review()
        .unwrap_or_else(|error| panic!("failed to mark application in review: {error}"));
    application
        .approve()
        .unwrap_or_else(|error| panic!("failed to approve application: {error}"));
    applications
        .in_transaction(&mut tx)
        .update(&application, by_id.version)
        .await
        .unwrap_or_else(|error| panic!("failed to update application: {error:?}"));
    let updated = applications
        .in_transaction(&mut tx)
        .find_by_id(application.id())
        .await
        .unwrap_or_else(|error| panic!("failed to find updated application: {error:?}"))
        .unwrap_or_else(|| panic!("missing updated application"));
    assert_eq!(
        PartnerShopApplicationState::Approved,
        updated.value.business_state()
    );
    assert_eq!(
        PartnerShopApplicationStorageVersion::INITIAL.next(),
        updated.version
    );

    applications
        .in_transaction(&mut tx)
        .delete(application.id(), updated.version)
        .await
        .unwrap_or_else(|error| panic!("failed to delete application: {error:?}"));
    let deleted = applications
        .in_transaction(&mut tx)
        .find_by_id(application.id())
        .await
        .unwrap_or_else(|error| panic!("failed to find deleted application: {error:?}"));
    commit(tx).await;

    assert!(deleted.is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_return_none_for_missing_partner_shop_application() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let applications = SqlxPartnerShopApplicationRepositoryFactory::new();
    let mut tx = begin(&unit_of_work).await;

    let result = applications
        .in_transaction(&mut tx)
        .find_by_id(PartnerShopApplicationId::new())
        .await
        .unwrap_or_else(|error| panic!("failed missing lookup: {error:?}"));
    commit(tx).await;

    assert!(result.is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_report_concurrency_conflict_for_stale_update_and_delete() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let applications = SqlxPartnerShopApplicationRepositoryFactory::new();
    let user_id = seed_user(&pool, "partner-app-conflict-user").await;
    let shop_id = seed_shop(&pool, "partner-app-conflict-shop").await;
    let mut application = new_application(user_id, shop_id);

    let mut tx = begin(&unit_of_work).await;
    applications
        .in_transaction(&mut tx)
        .insert(&application)
        .await
        .unwrap_or_else(|error| panic!("failed to insert application: {error:?}"));
    application
        .mark_in_review()
        .unwrap_or_else(|error| panic!("failed to mark application in review: {error}"));
    application
        .approve()
        .unwrap_or_else(|error| panic!("failed to approve application: {error}"));
    applications
        .in_transaction(&mut tx)
        .update(&application, PartnerShopApplicationStorageVersion::INITIAL)
        .await
        .unwrap_or_else(|error| panic!("failed to update application: {error:?}"));

    let stale_update = applications
        .in_transaction(&mut tx)
        .update(&application, PartnerShopApplicationStorageVersion::INITIAL)
        .await;
    let stale_delete = applications
        .in_transaction(&mut tx)
        .delete(
            application.id(),
            PartnerShopApplicationStorageVersion::INITIAL,
        )
        .await;
    commit(tx).await;

    assert!(matches!(
        stale_update,
        Err(PartnerShopApplicationRepositoryError::ConcurrencyConflict)
    ));
    assert!(matches!(
        stale_delete,
        Err(PartnerShopApplicationRepositoryError::ConcurrencyConflict)
    ));
}

fn new_application(user_id: UserId, shop_id: ShopId) -> PartnerShopApplication {
    PartnerShopApplication::create(NewPartnerShopApplication {
        id: PartnerShopApplicationId::new(),
        applicant_user_id: user_id,
        payload: PartnerShopApplicationPayload::New { shop_id },
    })
}

async fn seed_user(pool: &sqlx::PgPool, slug: &str) -> UserId {
    let user_id = UserId::new();
    sqlx::query("INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'FREE', 'USER')")
        .bind(uuid::Uuid::from(user_id))
        .bind(format!("{slug}@example.com"))
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("failed to seed user: {error}"));
    user_id
}

async fn seed_shop(pool: &sqlx::PgPool, slug: &str) -> ShopId {
    let shop_id = ShopId::new();
    sqlx::query(
        "INSERT INTO shops (shop_id, shop_slug_id, name, shop_type, partner_status, shop_domains) VALUES ($1, $2, $3, 'COMMERCIAL_DEALER', 'SCRAPED', $4)",
    )
    .bind(uuid::Uuid::from(shop_id))
    .bind(slug)
    .bind(slug)
    .bind(Vec::<String>::from([format!("{slug}.example")]))
    .execute(pool)
    .await
    .unwrap_or_else(|error| panic!("failed to seed shop: {error}"));
    shop_id
}

async fn begin(unit_of_work: &SqlxUnitOfWork) -> platform_postgres::SqlxTransaction {
    unit_of_work
        .begin()
        .await
        .unwrap_or_else(|error| panic!("failed to begin transaction: {error}"))
}

async fn commit(tx: platform_postgres::SqlxTransaction) {
    tx.commit()
        .await
        .unwrap_or_else(|error| panic!("failed to commit transaction: {error}"));
}
