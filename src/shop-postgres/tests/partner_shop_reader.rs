use application::transaction::{Transaction, UnitOfWork};
use platform_postgres::SqlxUnitOfWork;
use shop_core::affiliate_configuration::AffiliateConfiguration;
use shop_core::domain::Domain;
use shop_core::partner_status::ShopPartnerStatus;
use shop_core::shop::{NewShop, Shop, ShopContact, ShopPresentation};
use shop_core::shop_id::ShopId;
use shop_core::shop_name::ShopName;
use shop_core::shop_type::ShopType;
use shop_postgres::{
    SqlxPartnerShopReaderFactory, SqlxPartnerShopRepositoryFactory, SqlxShopRepositoryFactory,
};
use shop_service::ports::{
    PartnerShopReader, PartnerShopReaderFactory, PartnerShopRepository,
    PartnerShopRepositoryFactory, ShopRepository, ShopRepositoryFactory,
};
use shop_service::use_cases::queries::check_user_partner_shop::CheckUserPartnerShopRequest;
use std::collections::HashSet;
use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};
use url::Url;
use user_core::user_id::UserId;

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_read_granted_partner_shop() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let shops = SqlxShopRepositoryFactory::new();
    let partner_shops = SqlxPartnerShopRepositoryFactory::new();
    let partner_reader = SqlxPartnerShopReaderFactory::new();
    let user_id = UserId::new();
    let shop = sample_shop("postgres-partner");
    seed_user(&pool, user_id).await;

    let mut tx = begin(&unit_of_work).await;
    match shops.in_transaction(&mut tx).insert(&shop).await {
        Ok(_) => {}
        Err(error) => panic!("failed to insert shop: {error:?}"),
    }
    match partner_shops
        .in_transaction(&mut tx)
        .grant(user_id, shop.id())
        .await
    {
        Ok(_) => {}
        Err(error) => panic!("failed to grant partner shop: {error:?}"),
    }
    let is_partner = match partner_reader
        .in_transaction(&mut tx)
        .is_user_partner_of_shop(&CheckUserPartnerShopRequest {
            user_id,
            shop_id: shop.id(),
        })
        .await
    {
        Ok(is_partner) => is_partner,
        Err(error) => panic!("failed to read partner shop: {error:?}"),
    };
    let summaries = match partner_reader
        .in_transaction(&mut tx)
        .list_summaries_for_user(user_id)
        .await
    {
        Ok(summaries) => summaries,
        Err(error) => panic!("failed to list partner shops: {error:?}"),
    };
    commit(tx).await;

    assert!(is_partner);
    assert_eq!(1, summaries.len());
    assert_eq!(shop.id(), summaries[0].shop_id);
    assert_eq!(shop.name(), &summaries[0].name);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_return_false_when_partner_shop_row_is_missing() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let partner_reader = SqlxPartnerShopReaderFactory::new();

    let mut tx = begin(&unit_of_work).await;
    let is_partner = match partner_reader
        .in_transaction(&mut tx)
        .is_user_partner_of_shop(&CheckUserPartnerShopRequest {
            user_id: UserId::new(),
            shop_id: ShopId::new(),
        })
        .await
    {
        Ok(value) => value,
        Err(error) => panic!("failed to read missing partner shop: {error:?}"),
    };
    commit(tx).await;

    assert!(!is_partner);
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
