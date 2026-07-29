use common::domain::Domain;
use common::postgres::SqlxUnitOfWork;
use common::transaction::{Transaction, UnitOfWork};
use common::{shop_id::ShopId, shop_name::ShopName};
use shop_core::affiliate_configuration::AffiliateConfiguration;
use shop_core::partner_status::ShopPartnerStatus;
use shop_core::shop::{NewShop, Shop, ShopContact, ShopPresentation, ShopifyIntegration};
use shop_core::shop_type::ShopType;
use shop_postgres::{SqlxShopDetailsReaderFactory, SqlxShopRepositoryFactory};
use shop_service::ports::{
    ShopDetailsReader, ShopDetailsReaderFactory, ShopRepository, ShopRepositoryFactory,
};
use shop_service::use_cases::queries::get_shop::GetShopRequest;
use std::collections::HashSet;
use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};
use url::Url;

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
#[serial_test::serial]
async fn should_read_shop_details_by_slug_and_shopify_domain() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let shops = SqlxShopRepositoryFactory::new();
    let details = SqlxShopDetailsReaderFactory::new();
    let shopify_domain = domain("shopify-details.example");
    let shop = sample_shop_with_shopify("postgres-details-lookup", shopify_domain.clone());

    let mut tx = begin(&unit_of_work).await;
    match shops.in_transaction(&mut tx).insert(&shop).await {
        Ok(()) => {}
        Err(error) => panic!("failed to insert shop: {error:?}"),
    }
    let by_slug = match details
        .in_transaction(&mut tx)
        .find_details(&GetShopRequest::BySlug(shop.slug_id().clone()))
        .await
    {
        Ok(Some(view)) => view,
        Ok(None) => panic!("missing shop details by slug"),
        Err(error) => panic!("failed to read shop details by slug: {error:?}"),
    };
    let by_shopify_domain = match details
        .in_transaction(&mut tx)
        .find_details(&GetShopRequest::ByShopifyDomain(shopify_domain))
        .await
    {
        Ok(Some(view)) => view,
        Ok(None) => panic!("missing shop details by shopify domain"),
        Err(error) => panic!("failed to read shop details by shopify domain: {error:?}"),
    };
    commit(tx).await;

    assert_eq!(shop.id(), by_slug.shop_id);
    assert_eq!(shop.id(), by_shopify_domain.shop_id);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
#[serial_test::serial]
async fn should_return_none_when_shop_details_rows_are_missing() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let details = SqlxShopDetailsReaderFactory::new();
    let missing_shop_id = common::shop_id::ShopId::new();

    let mut tx = begin(&unit_of_work).await;
    let details_by_id = match details
        .in_transaction(&mut tx)
        .find_details(&GetShopRequest::ById(missing_shop_id))
        .await
    {
        Ok(value) => value,
        Err(error) => panic!("failed to find missing details by id: {error:?}"),
    };
    let details_by_slug = match details
        .in_transaction(&mut tx)
        .find_details(&GetShopRequest::BySlug("missing-shop".into()))
        .await
    {
        Ok(value) => value,
        Err(error) => panic!("failed to find missing details by slug: {error:?}"),
    };
    let details_by_shopify_domain = match details
        .in_transaction(&mut tx)
        .find_details(&GetShopRequest::ByShopifyDomain(domain(
            "missing-shopify.example",
        )))
        .await
    {
        Ok(value) => value,
        Err(error) => panic!("failed to find missing details by shopify domain: {error:?}"),
    };
    commit(tx).await;

    assert!(details_by_id.is_none());
    assert!(details_by_slug.is_none());
    assert!(details_by_shopify_domain.is_none());
}

const BUSINESS_SCHEMA: test_api::Postgres = test_api::Postgres::new("migrations");

fn sample_shop_with_shopify(slug: &str, shopify_domain: Domain) -> Shop {
    Shop::create(new_shop(slug, Some(shopify_domain)))
}

fn domain(value: &str) -> Domain {
    match Domain::try_from(value) {
        Ok(domain) => domain,
        Err(error) => panic!("invalid test domain: {error}"),
    }
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

fn new_shop(slug: &str, shopify_domain: Option<Domain>) -> NewShop {
    NewShop {
        id: ShopId::new(),
        name: ShopName::from(slug),
        shop_type: ShopType::CommercialDealer,
        domains: HashSet::from([domain(&format!("{slug}.example"))]),
        shopify: shopify_domain.map(|domain| ShopifyIntegration {
            domain,
            currency: None,
            language: None,
        }),
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

fn url(value: &str) -> Url {
    match Url::parse(value) {
        Ok(url) => url,
        Err(error) => panic!("invalid test URL: {error}"),
    }
}
