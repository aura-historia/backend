use common::domain::Domain;
use common::postgres::SqlxUnitOfWork;
use common::transaction::{Transaction, UnitOfWork};
use common::{shop_id::ShopId, shop_name::ShopName};
use shop_core::affiliate_configuration::AffiliateConfiguration;
use shop_core::partner_status::ShopPartnerStatus;
use shop_core::shop::{NewShop, Shop, ShopContact, ShopPresentation};
use shop_core::shop_type::ShopType;
use shop_postgres::{SqlxShopDetailsReaderFactory, SqlxShopRepositoryFactory};
use shop_service::ports::{
    ShopDetailsReader, ShopDetailsReaderFactory, ShopRepository, ShopRepositoryError,
    ShopRepositoryFactory,
};
use shop_service::use_cases::queries::get_shop::GetShopRequest;
use std::collections::HashSet;
use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};
use url::Url;

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_persist_shop_without_persisting_view_url_and_derive_details_view_url() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let shops = SqlxShopRepositoryFactory::new();
    let details = SqlxShopDetailsReaderFactory::new();
    let shop = sample_shop("postgres-no-view-url");

    let mut tx = begin(&unit_of_work).await;
    match shops.in_transaction(&mut tx).insert(&shop).await {
        Ok(_) => {}
        Err(error) => panic!("failed to insert shop: {error:?}"),
    }
    commit(tx).await;

    let persisted_view_url = match sqlx::query_scalar::<_, Option<String>>(
        "SELECT view_url FROM shops WHERE shop_id = $1",
    )
    .bind(uuid::Uuid::from(shop.id()))
    .fetch_one(&pool)
    .await
    {
        Ok(value) => value,
        Err(error) => panic!("failed to load persisted view_url: {error}"),
    };
    assert_eq!(None, persisted_view_url);

    let mut tx = begin(&unit_of_work).await;
    let view = match details
        .in_transaction(&mut tx)
        .find_details(&GetShopRequest::ById(shop.id()))
        .await
    {
        Ok(Some(view)) => view,
        Ok(None) => panic!("missing shop details"),
        Err(error) => panic!("failed to read shop details: {error:?}"),
    };
    commit(tx).await;

    assert_eq!(shop.id(), view.shop_id);
    assert_eq!(
        Some(
            "https://prf.hn/click/camref:1110lF73C/pubref:aurahistoria/destination:https%3A%2F%2Fexample.com%2Fpostgres-no-view-url"
        ),
        view.view_url.as_ref().map(Url::as_str)
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_find_shop_by_slug() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let shops = SqlxShopRepositoryFactory::new();
    let shop = sample_shop("postgres-find-by-slug");

    let mut tx = begin(&unit_of_work).await;
    match shops.in_transaction(&mut tx).insert(&shop).await {
        Ok(_) => {}
        Err(error) => panic!("failed to insert shop: {error:?}"),
    }
    let loaded = match shops
        .in_transaction(&mut tx)
        .find_by_slug(shop.slug_id())
        .await
    {
        Ok(Some(loaded)) => loaded,
        Ok(None) => panic!("missing shop by slug"),
        Err(error) => panic!("failed to find shop by slug: {error:?}"),
    };
    commit(tx).await;

    assert_eq!(shop.id(), loaded.shop.id());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_update_shop_with_optimistic_concurrency() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let shops = SqlxShopRepositoryFactory::new();
    let shop = sample_shop("postgres-concurrency");

    let mut tx = begin(&unit_of_work).await;
    match shops.in_transaction(&mut tx).insert(&shop).await {
        Ok(_) => {}
        Err(error) => panic!("failed to insert shop: {error:?}"),
    }
    commit(tx).await;

    let mut tx = begin(&unit_of_work).await;
    let shop_service::ports::StoredShop {
        shop: mut loaded,
        version,
        ..
    } = match shops.in_transaction(&mut tx).find_by_id(shop.id()).await {
        Ok(Some(loaded)) => loaded,
        Ok(None) => panic!("missing shop"),
        Err(error) => panic!("failed to load shop: {error:?}"),
    };
    loaded.change_partner_status(ShopPartnerStatus::Partnered);
    match shops.in_transaction(&mut tx).update(&loaded, version).await {
        Ok(_) => {}
        Err(error) => panic!("failed to update shop: {error:?}"),
    }
    commit(tx).await;

    let mut tx = begin(&unit_of_work).await;
    let stale_result = shops.in_transaction(&mut tx).update(&loaded, version).await;

    assert!(matches!(
        stale_result,
        Err(ShopRepositoryError::ConcurrencyConflict)
    ));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_return_none_when_shop_row_is_missing() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let shops = SqlxShopRepositoryFactory::new();

    let mut tx = begin(&unit_of_work).await;
    let by_id = match shops
        .in_transaction(&mut tx)
        .find_by_id(common::shop_id::ShopId::new())
        .await
    {
        Ok(value) => value,
        Err(error) => panic!("failed to find missing shop: {error:?}"),
    };
    commit(tx).await;

    assert!(by_id.is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_report_slug_conflict_when_inserting_duplicate_slug() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let shops = SqlxShopRepositoryFactory::new();
    let first = sample_shop("postgres-duplicate-slug");
    let second = sample_shop("postgres-duplicate-slug");

    let mut tx = begin(&unit_of_work).await;
    match shops.in_transaction(&mut tx).insert(&first).await {
        Ok(_) => {}
        Err(error) => panic!("failed to insert first shop: {error:?}"),
    }
    let result = shops.in_transaction(&mut tx).insert(&second).await;

    assert!(matches!(
        result,
        Err(ShopRepositoryError::SlugConflict { source }) if !source.to_string().is_empty()
    ));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_roll_back_shop_when_transaction_is_not_committed() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let shops = SqlxShopRepositoryFactory::new();
    let shop = sample_shop("postgres-rollback");

    let mut tx = begin(&unit_of_work).await;
    match shops.in_transaction(&mut tx).insert(&shop).await {
        Ok(_) => {}
        Err(error) => panic!("failed to insert rollback shop: {error:?}"),
    }
    drop(tx);

    let mut tx = begin(&unit_of_work).await;
    let loaded = match shops.in_transaction(&mut tx).find_by_id(shop.id()).await {
        Ok(value) => value,
        Err(error) => panic!("failed to load rollback shop: {error:?}"),
    };
    commit(tx).await;

    assert!(loaded.is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_report_update_conflict_when_shop_row_is_missing() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let shops = SqlxShopRepositoryFactory::new();
    let shop = sample_shop("postgres-update-missing");

    let mut tx = begin(&unit_of_work).await;
    let result = shops
        .in_transaction(&mut tx)
        .update(&shop, shop_service::ports::ShopStorageVersion::INITIAL)
        .await;

    assert!(matches!(
        result,
        Err(ShopRepositoryError::ConcurrencyConflict)
    ));
}

const BUSINESS_SCHEMA: test_api::Postgres = test_api::Postgres::new("migrations");

fn sample_shop(slug: &str) -> Shop {
    let mut shop = Shop::create(new_shop(slug));
    let _ = shop.publish();
    shop
}

async fn begin(unit_of_work: &SqlxUnitOfWork) -> common::postgres::SqlxTransaction {
    match unit_of_work.begin().await {
        Ok(tx) => tx,
        Err(error) => panic!("failed to begin transaction: {error}"),
    }
}

async fn commit(tx: common::postgres::SqlxTransaction) {
    if let Err(error) = tx.commit().await {
        panic!("failed to commit transaction: {error}");
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
