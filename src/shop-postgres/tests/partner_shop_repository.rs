use application::transaction::{Transaction, UnitOfWork};
use common::domain::Domain;
use common::{shop_id::ShopId, shop_name::ShopName, user_id::UserId};
use platform_postgres::SqlxUnitOfWork;
use shop_core::affiliate_configuration::AffiliateConfiguration;
use shop_core::partner_status::ShopPartnerStatus;
use shop_core::shop::{NewShop, Shop, ShopContact, ShopPresentation};
use shop_core::shop_type::ShopType;
use shop_postgres::{
    SqlxPartnerShopReaderFactory, SqlxPartnerShopRepositoryFactory, SqlxShopRepositoryFactory,
};
use shop_service::ports::{
    PartnerShopReader, PartnerShopReaderFactory, PartnerShopRepository, PartnerShopRepositoryError,
    PartnerShopRepositoryFactory, ShopRepository, ShopRepositoryFactory,
};
use shop_service::use_cases::queries::check_user_partner_shop::CheckUserPartnerShopRequest;
use std::collections::HashSet;
use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};
use url::Url;

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_report_missing_user_when_granting_partner_shop() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let shops = SqlxShopRepositoryFactory::new();
    let partner_shops = SqlxPartnerShopRepositoryFactory::new();
    let shop = sample_shop("postgres-missing-user");

    let mut tx = begin(&unit_of_work).await;
    match shops.in_transaction(&mut tx).insert(&shop).await {
        Ok(_) => {}
        Err(error) => panic!("failed to insert shop: {error:?}"),
    }
    let result = partner_shops
        .in_transaction(&mut tx)
        .grant(UserId::new(), shop.id())
        .await;

    assert!(matches!(
        result,
        Err(PartnerShopRepositoryError::UserNotFound { source }) if !source.to_string().is_empty()
    ));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_report_missing_shop_when_granting_partner_shop() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let partner_shops = SqlxPartnerShopRepositoryFactory::new();
    let user_id = UserId::new();
    seed_user(&pool, user_id).await;

    let mut tx = begin(&unit_of_work).await;
    let result = partner_shops
        .in_transaction(&mut tx)
        .grant(user_id, ShopId::new())
        .await;

    assert!(matches!(
        result,
        Err(PartnerShopRepositoryError::ShopNotFound { source }) if !source.to_string().is_empty()
    ));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_grant_partner_shop_idempotently() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let shops = SqlxShopRepositoryFactory::new();
    let partner_shops = SqlxPartnerShopRepositoryFactory::new();
    let partner_reader = SqlxPartnerShopReaderFactory::new();
    let user_id = UserId::new();
    let shop = sample_shop("postgres-partner-idempotent");
    seed_user(&pool, user_id).await;

    let mut tx = begin(&unit_of_work).await;
    match shops.in_transaction(&mut tx).insert(&shop).await {
        Ok(_) => {}
        Err(error) => panic!("failed to insert shop: {error:?}"),
    }
    for _ in 0..2 {
        match partner_shops
            .in_transaction(&mut tx)
            .grant(user_id, shop.id())
            .await
        {
            Ok(_) => {}
            Err(error) => panic!("failed to grant partner shop idempotently: {error:?}"),
        }
    }
    let is_partner = match partner_reader
        .in_transaction(&mut tx)
        .is_user_partner_of_shop(&CheckUserPartnerShopRequest {
            user_id,
            shop_id: shop.id(),
        })
        .await
    {
        Ok(value) => value,
        Err(error) => panic!("failed to read partner shop: {error:?}"),
    };
    commit(tx).await;

    assert!(is_partner);
}

const BUSINESS_SCHEMA: test_api::Postgres = test_api::Postgres::new("migrations");

fn sample_shop(slug: &str) -> Shop {
    Shop::create(new_shop(slug))
}

async fn begin(unit_of_work: &SqlxUnitOfWork) -> platform_postgres::SqlxTransaction {
    match unit_of_work.begin().await {
        Ok(tx) => tx,
        Err(error) => panic!("failed to begin transaction: {error}"),
    }
}

async fn commit(tx: platform_postgres::SqlxTransaction) {
    if let Err(error) = tx.commit().await {
        panic!("failed to commit transaction: {error}");
    }
}

async fn seed_user(pool: &sqlx::PgPool, user_id: UserId) {
    let result = sqlx::query(
        r#"
        INSERT INTO users (
            user_id, email, tier, role
        ) VALUES ($1, $2, 'FREE', 'USER')
        "#,
    )
    .bind(uuid::Uuid::from(user_id))
    .bind(format!("{user_id}@example.com"))
    .execute(pool)
    .await;

    if let Err(error) = result {
        panic!("failed to seed user: {error}");
    }
}

fn new_shop(slug: &str) -> NewShop {
    NewShop {
        id: ShopId::new(),
        name: ShopName::from(slug),
        shop_type: ShopType::CommercialDealer,
        domains: HashSet::from([domain(&format!("{slug}.example"))]),
        shopify: None,
        woocommerce: None,
        presentation: ShopPresentation {
            url: Some(url(&format!("https://example.com/{slug}"))),
            image: Some(url(&format!("https://example.com/{slug}.jpg"))),
        },
        address: None,
        contact: ShopContact::default(),
        partner_status: ShopPartnerStatus::Scraped,
        affiliate_configuration: Some(AffiliateConfiguration::Partnerize {
            camref: "1110lF73C".to_owned(),
        }),
    }
}

fn domain(value: &str) -> Domain {
    match Domain::try_from(value) {
        Ok(domain) => domain,
        Err(error) => panic!("invalid test domain: {error}"),
    }
}

fn url(value: &str) -> Url {
    match Url::parse(value) {
        Ok(url) => url,
        Err(error) => panic!("invalid test URL: {error}"),
    }
}
