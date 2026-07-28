use common::domain::Domain;
use common::operation_context::Principal;
use common::pagination::cursor::Cursor;
use common::postgres::SqlxUnitOfWork;
use common::query::text_query::TextQuery;
use common::transaction::{Transaction, UnitOfWork};
use common::versioned::Versioned;
use common::write_metadata::WriteMetadata;
use common::{shop_id::ShopId, shop_name::ShopName, user_id::UserId};
use serde_json::Value;
use shop_core::affiliate_configuration::AffiliateConfiguration;
use shop_core::partner_status::ShopPartnerStatus;
use shop_core::shop::{NewShop, Shop, ShopContact, ShopPresentation};
use shop_core::shop_search::ShopSearch;
use shop_core::shop_type::ShopType;
use shop_postgres::{
    SqlxPartnerShopReaderFactory, SqlxPartnerShopRepositoryFactory, SqlxShopDetailsReaderFactory,
    SqlxShopRepositoryFactory, SqlxShopSearchReaderFactory,
};
use shop_service::ports::{
    PartnerShopReader, PartnerShopReaderFactory, PartnerShopRepository, PartnerShopRepositoryError,
    PartnerShopRepositoryFactory, ShopDetailsReader, ShopDetailsReaderFactory, ShopRepository,
    ShopRepositoryError, ShopRepositoryFactory, ShopSearchReader, ShopSearchReaderFactory,
};
use shop_service::use_cases::queries::check_user_partner_shop::CheckUserPartnerShopRequest;
use shop_service::use_cases::queries::get_shop::GetShopRequest;
use shop_service::use_cases::queries::search_shops::SearchShopsRequest;
use std::collections::HashSet;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use url::Url;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_persist_shop_without_persisting_view_url_and_derive_details_view_url() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let shops = SqlxShopRepositoryFactory::new();
    let details = SqlxShopDetailsReaderFactory::new();
    let metadata = system_metadata();
    let shop = sample_shop("postgres-no-view-url");

    let mut tx = begin(&unit_of_work).await;
    match shops.in_transaction(&mut tx).insert(&shop, &metadata).await {
        Ok(()) => {}
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
async fn should_update_shop_with_optimistic_concurrency() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let shops = SqlxShopRepositoryFactory::new();
    let metadata = system_metadata();
    let shop = sample_shop("postgres-concurrency");

    let mut tx = begin(&unit_of_work).await;
    match shops.in_transaction(&mut tx).insert(&shop, &metadata).await {
        Ok(()) => {}
        Err(error) => panic!("failed to insert shop: {error:?}"),
    }
    commit(tx).await;

    let mut tx = begin(&unit_of_work).await;
    let Versioned {
        value: mut loaded,
        version,
    } = match shops.in_transaction(&mut tx).find_by_id(shop.id()).await {
        Ok(Some(loaded)) => loaded,
        Ok(None) => panic!("missing shop"),
        Err(error) => panic!("failed to load shop: {error:?}"),
    };
    loaded.change_partner_status(ShopPartnerStatus::Partnered);
    match shops
        .in_transaction(&mut tx)
        .update(&loaded, version, &metadata)
        .await
    {
        Ok(()) => {}
        Err(error) => panic!("failed to update shop: {error:?}"),
    }
    commit(tx).await;

    let mut tx = begin(&unit_of_work).await;
    let stale_result = shops
        .in_transaction(&mut tx)
        .update(&loaded, version, &metadata)
        .await;

    assert!(matches!(
        stale_result,
        Err(ShopRepositoryError::ConcurrencyConflict)
    ));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_grant_and_read_partner_shop() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let shops = SqlxShopRepositoryFactory::new();
    let partner_shops = SqlxPartnerShopRepositoryFactory::new();
    let partner_reader = SqlxPartnerShopReaderFactory::new();
    let metadata = system_metadata();
    let user_id = UserId::new();
    let shop = sample_shop("postgres-partner");
    seed_user(&pool, user_id).await;

    let mut tx = begin(&unit_of_work).await;
    match shops.in_transaction(&mut tx).insert(&shop, &metadata).await {
        Ok(()) => {}
        Err(error) => panic!("failed to insert shop: {error:?}"),
    }
    match partner_shops
        .in_transaction(&mut tx)
        .grant(user_id, shop.id(), &metadata)
        .await
    {
        Ok(()) => {}
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
    commit(tx).await;

    assert!(is_partner);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_report_missing_user_when_granting_partner_shop() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let shops = SqlxShopRepositoryFactory::new();
    let partner_shops = SqlxPartnerShopRepositoryFactory::new();
    let metadata = system_metadata();
    let shop = sample_shop("postgres-missing-user");

    let mut tx = begin(&unit_of_work).await;
    match shops.in_transaction(&mut tx).insert(&shop, &metadata).await {
        Ok(()) => {}
        Err(error) => panic!("failed to insert shop: {error:?}"),
    }
    let result = partner_shops
        .in_transaction(&mut tx)
        .grant(UserId::new(), shop.id(), &metadata)
        .await;

    assert!(matches!(
        result,
        Err(PartnerShopRepositoryError::UserNotFound)
    ));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_search_shops_in_postgres() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let shops = SqlxShopRepositoryFactory::new();
    let search = SqlxShopSearchReaderFactory::new();
    let metadata = system_metadata();
    let matching = sample_shop("postgres-search-match");
    let other = sample_shop("postgres-other");

    let mut tx = begin(&unit_of_work).await;
    for shop in [&matching, &other] {
        match shops.in_transaction(&mut tx).insert(shop, &metadata).await {
            Ok(()) => {}
            Err(error) => panic!("failed to insert shop: {error:?}"),
        }
    }
    let result = match search
        .in_transaction(&mut tx)
        .search(&SearchShopsRequest {
            search: ShopSearch {
                shop_name_query: Some(text_query("match")),
                ..Default::default()
            },
            sort: None,
            cursor: Some(Cursor::<Value> {
                size: 10,
                search_after: None,
            }),
        })
        .await
    {
        Ok(result) => result,
        Err(error) => panic!("failed to search shops: {error:?}"),
    };
    commit(tx).await;

    assert_eq!(1, result.items.len());
    assert_eq!(matching.id(), result.items[0].shop_id);
}

fn sample_shop(slug: &str) -> Shop {
    Shop::create(NewShop {
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
    })
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

fn text_query(value: &str) -> TextQuery<0> {
    match TextQuery::try_from(value) {
        Ok(query) => query,
        Err(error) => panic!("invalid test text query: {error}"),
    }
}

fn system_metadata() -> WriteMetadata {
    match WriteMetadata::try_from(&Principal::System) {
        Ok(metadata) => metadata,
        Err(error) => panic!("failed to build system metadata: {error}"),
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

async fn seed_user(pool: &sqlx::PgPool, user_id: UserId) {
    let result = sqlx::query(
        r#"
        INSERT INTO users (
            user_id, email, tier, role, created_by, updated_by
        ) VALUES ($1, $2, 'FREE', 'USER', 'shop-postgres-test', 'shop-postgres-test')
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
