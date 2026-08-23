use application::transaction::{Transaction, UnitOfWork};
use platform_postgres::SqlxUnitOfWork;
use shop_core::shop_id::ShopId;
use shop_partner_core::partner_shop_application::{
    NewPartnerShopApplication, PartnerShopApplication, PartnerShopApplicationPayload,
};
use shop_partner_core::partner_shop_application_id::PartnerShopApplicationId;
use shop_partner_postgres::{
    SqlxPartnerShopApplicationReaderFactory, SqlxPartnerShopApplicationRepositoryFactory,
};
use shop_partner_service::ports::{
    PartnerShopApplicationReader, PartnerShopApplicationReaderFactory,
    PartnerShopApplicationRepository, PartnerShopApplicationRepositoryFactory,
};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use user_core::user_id::UserId;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_list_all_and_by_user_partner_shop_applications_in_created_order() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let repositories = SqlxPartnerShopApplicationRepositoryFactory::new();
    let readers = SqlxPartnerShopApplicationReaderFactory::new();
    let user_id = seed_user(&pool, "partner-app-reader-user").await;
    let other_user_id = seed_user(&pool, "partner-app-reader-other-user").await;
    let first = new_application(user_id, seed_shop(&pool, "partner-app-reader-a").await);
    let second = new_application(user_id, seed_shop(&pool, "partner-app-reader-b").await);
    let other = new_application(
        other_user_id,
        seed_shop(&pool, "partner-app-reader-c").await,
    );

    let mut tx = begin(&unit_of_work).await;
    for app in [&first, &second, &other] {
        repositories
            .in_transaction(&mut tx)
            .insert(app)
            .await
            .unwrap_or_else(|error| panic!("failed to insert application: {error:?}"));
    }
    let all = readers
        .in_transaction(&mut tx)
        .list_all()
        .await
        .unwrap_or_else(|error| panic!("failed to list all applications: {error:?}"));
    let by_user = readers
        .in_transaction(&mut tx)
        .list_by_user(user_id)
        .await
        .unwrap_or_else(|error| panic!("failed to list applications by user: {error:?}"));
    commit(tx).await;

    assert_eq!(3, all.len());
    assert_eq!(2, by_user.len());
    assert!(by_user.iter().all(|item| item.applicant_user_id == user_id));
}

fn new_application(user_id: UserId, shop_id: ShopId) -> PartnerShopApplication {
    PartnerShopApplication::create(NewPartnerShopApplication {
        id: PartnerShopApplicationId::new(),
        applicant_user_id: user_id,
        payload: PartnerShopApplicationPayload::Existing { shop_id },
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
