use application::transaction::UnitOfWork;
use platform_postgres::SqlxUnitOfWork;
use product_postgres::SqlxPartnerProductAuthorizerFactory;
use product_service::ports::{
    PartnerProductAuthorizationError, PartnerProductAuthorizer, PartnerProductAuthorizerFactory,
};
use shop_core::shop_id::ShopId;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use user_core::user_id::UserId;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_authorize_admin_for_existing_shop() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let authorizer = SqlxPartnerProductAuthorizerFactory::new();
    let admin_id = seed_user(&pool, "ADMIN").await;
    let shop_id = seed_shop(&pool, "admin-product-authorizer-shop", "SCRAPED").await;

    let transaction = unit_of_work.begin().await;
    assert!(
        transaction.is_ok(),
        "failed to begin authorization transaction"
    );
    if let Ok(mut tx) = transaction {
        let result = authorizer
            .in_transaction(&mut tx)
            .authorize(admin_id, shop_id)
            .await;
        assert!(matches!(result, Ok(())));
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_authorize_partnered_shop_member_and_reject_unrelated_user() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let authorizer = SqlxPartnerProductAuthorizerFactory::new();
    let partner_id = seed_user(&pool, "USER").await;
    let unrelated_user_id = seed_user(&pool, "USER").await;
    let shop_id = seed_shop(&pool, "partner-product-authorizer-shop", "PARTNERED").await;
    let membership =
        sqlx::query("INSERT INTO user_partner_shops (user_id, shop_id) VALUES ($1, $2)")
            .bind(uuid::Uuid::from(partner_id))
            .bind(uuid::Uuid::from(shop_id))
            .execute(&pool)
            .await;
    assert!(membership.is_ok(), "failed to seed partner-shop membership");

    let transaction = unit_of_work.begin().await;
    assert!(
        transaction.is_ok(),
        "failed to begin authorization transaction"
    );
    if let Ok(mut tx) = transaction {
        let result = authorizer
            .in_transaction(&mut tx)
            .authorize(partner_id, shop_id)
            .await;
        assert!(matches!(result, Ok(())));
    }

    let transaction = unit_of_work.begin().await;
    assert!(
        transaction.is_ok(),
        "failed to begin authorization transaction"
    );
    if let Ok(mut tx) = transaction {
        let result = authorizer
            .in_transaction(&mut tx)
            .authorize(unrelated_user_id, shop_id)
            .await;
        assert!(matches!(
            result,
            Err(PartnerProductAuthorizationError::Forbidden)
        ));
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_report_missing_shop() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let authorizer = SqlxPartnerProductAuthorizerFactory::new();
    let user_id = seed_user(&pool, "USER").await;

    let transaction = unit_of_work.begin().await;
    assert!(
        transaction.is_ok(),
        "failed to begin authorization transaction"
    );
    if let Ok(mut tx) = transaction {
        let result = authorizer
            .in_transaction(&mut tx)
            .authorize(user_id, ShopId::new())
            .await;
        assert!(matches!(
            result,
            Err(PartnerProductAuthorizationError::ShopNotFound)
        ));
    }
}

async fn seed_user(pool: &sqlx::PgPool, role: &str) -> UserId {
    let user_id = UserId::new();
    let result =
        sqlx::query("INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'FREE', $3)")
            .bind(uuid::Uuid::from(user_id))
            .bind(format!("{user_id}@example.test"))
            .bind(role)
            .execute(pool)
            .await;
    assert!(result.is_ok(), "failed to seed user");
    user_id
}

async fn seed_shop(pool: &sqlx::PgPool, slug: &str, partner_status: &str) -> ShopId {
    let shop_id = ShopId::new();
    let result = sqlx::query(
        r#"
        INSERT INTO shops (shop_id, shop_slug_id, name, shop_type, partner_status, shop_domains)
        VALUES ($1, $2, $3, 'COMMERCIAL_DEALER', $4, '{}')
        "#,
    )
    .bind(uuid::Uuid::from(shop_id))
    .bind(format!("{slug}-{shop_id}"))
    .bind(format!("{slug}-{shop_id}"))
    .bind(partner_status)
    .execute(pool)
    .await;
    assert!(result.is_ok(), "failed to seed shop");
    shop_id
}
