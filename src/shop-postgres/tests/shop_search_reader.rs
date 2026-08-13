use common::domain::Domain;
use common::pagination::cursor::Cursor;
use common::postgres::SqlxUnitOfWork;
use common::query::range_query::RangeQuery;
use common::query::text_query::TextQuery;
use common::shop_id::ShopId;
use common::shop_name::ShopName;
use common::sort::{Sort, SortOrder};
use common::transaction::{Transaction, UnitOfWork};
use shop_core::address::{GeoAddress, StructuredAddress};
use shop_core::affiliate_configuration::AffiliateConfiguration;
use shop_core::continent::Continent;
use shop_core::partner_status::ShopPartnerStatus;
use shop_core::shop::{NewShop, Shop, ShopAddress, ShopContact, ShopPresentation};
use shop_core::shop_search::ShopSearch;
use shop_core::shop_type::ShopType;
use shop_core::sort_shop_field::SortShopField;
use shop_postgres::{SqlxShopRepositoryFactory, SqlxShopSearchReaderFactory};
use shop_service::ports::{
    ShopRepository, ShopRepositoryFactory, ShopSearchReader, ShopSearchReaderFactory,
};
use shop_service::use_cases::queries::search_shops::SearchShopsRequest;
use std::collections::HashSet;
use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};
use time::{Duration, OffsetDateTime};
use url::Url;

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_search_shops_in_postgres() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let shops = SqlxShopRepositoryFactory::new();
    let search = SqlxShopSearchReaderFactory::new();
    let matching = sample_shop("postgres-search-match");
    let other = sample_shop("postgres-other");

    let mut tx = begin(&unit_of_work).await;
    for shop in [&matching, &other] {
        match shops.in_transaction(&mut tx).insert(shop).await {
            Ok(_) => {}
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
            cursor: Some(Cursor::<ShopId> {
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

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_exclude_non_published_shops_from_public_search() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let shops = SqlxShopRepositoryFactory::new();
    let search = SqlxShopSearchReaderFactory::new();
    let published = sample_shop("postgres-public-visibility-published");
    let drafted = Shop::create(new_shop("postgres-public-visibility-drafted"));
    let discarded = Shop::create(new_shop("postgres-public-visibility-discarded"));

    let mut tx = begin(&unit_of_work).await;
    for shop in [&published, &drafted, &discarded] {
        if let Err(error) = shops.in_transaction(&mut tx).insert(shop).await {
            panic!("failed to insert public visibility shop: {error:?}");
        }
    }
    commit(tx).await;
    set_shop_lifecycle(&pool, discarded.id(), "DISCARDED").await;

    let mut tx = begin(&unit_of_work).await;
    let result = match search
        .in_transaction(&mut tx)
        .search(&SearchShopsRequest {
            search: ShopSearch {
                shop_name_query: Some(text_query("postgres-public-visibility")),
                ..Default::default()
            },
            sort: None,
            cursor: None,
        })
        .await
    {
        Ok(result) => result,
        Err(error) => panic!("failed to search public shops: {error:?}"),
    };
    commit(tx).await;

    assert_eq!(
        vec![published.id()],
        result
            .items
            .into_iter()
            .map(|item| item.shop_id)
            .collect::<Vec<_>>()
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_page_shop_search_with_shop_id_cursor() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let shops = SqlxShopRepositoryFactory::new();
    let search = SqlxShopSearchReaderFactory::new();
    let first = sample_shop("postgres-cursor-a");
    let second = sample_shop("postgres-cursor-b");

    let mut tx = begin(&unit_of_work).await;
    for shop in [&first, &second] {
        match shops.in_transaction(&mut tx).insert(shop).await {
            Ok(_) => {}
            Err(error) => panic!("failed to insert shop: {error:?}"),
        }
    }
    let first_page = match search
        .in_transaction(&mut tx)
        .search(&SearchShopsRequest {
            search: ShopSearch {
                shop_name_query: Some(text_query("postgres-cursor")),
                ..Default::default()
            },
            sort: Some(Sort {
                sort: SortShopField::Name,
                order: SortOrder::Asc,
            }),
            cursor: Some(Cursor::<ShopId> {
                size: 1,
                search_after: None,
            }),
        })
        .await
    {
        Ok(result) => result,
        Err(error) => panic!("failed to search first page: {error:?}"),
    };
    let second_page = match search
        .in_transaction(&mut tx)
        .search(&SearchShopsRequest {
            search: ShopSearch {
                shop_name_query: Some(text_query("postgres-cursor")),
                ..Default::default()
            },
            sort: Some(Sort {
                sort: SortShopField::Name,
                order: SortOrder::Asc,
            }),
            cursor: Some(Cursor::<ShopId> {
                size: 1,
                search_after: first_page.cursor.search_after,
            }),
        })
        .await
    {
        Ok(result) => result,
        Err(error) => panic!("failed to search second page: {error:?}"),
    };
    commit(tx).await;

    assert_eq!(Some(first.id()), first_page.cursor.search_after);
    assert_eq!(first.id(), first_page.items[0].shop_id);
    assert_eq!(second.id(), second_page.items[0].shop_id);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_return_empty_search_when_no_shop_matches() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let search = SqlxShopSearchReaderFactory::new();

    let mut tx = begin(&unit_of_work).await;
    let result = match search
        .in_transaction(&mut tx)
        .search(&SearchShopsRequest {
            search: ShopSearch {
                shop_name_query: Some(text_query("no-shop-should-match-this-query")),
                ..Default::default()
            },
            sort: None,
            cursor: None,
        })
        .await
    {
        Ok(result) => result,
        Err(error) => panic!("failed to search empty shops: {error:?}"),
    };
    commit(tx).await;

    assert!(result.items.is_empty());
    assert_eq!(None, result.cursor.search_after);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_filter_search_by_type_partner_country_continent_and_dates() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let shops = SqlxShopRepositoryFactory::new();
    let search = SqlxShopSearchReaderFactory::new();
    let matching = search_shop(
        "postgres-filter-match",
        ShopType::AuctionHouse,
        ShopPartnerStatus::Partnered,
        isocountry::CountryCode::DEU,
    );
    let wrong_type = search_shop(
        "postgres-filter-wrong-type",
        ShopType::Marketplace,
        ShopPartnerStatus::Partnered,
        isocountry::CountryCode::DEU,
    );
    let wrong_status = search_shop(
        "postgres-filter-wrong-status",
        ShopType::AuctionHouse,
        ShopPartnerStatus::Scraped,
        isocountry::CountryCode::DEU,
    );
    let wrong_country = search_shop(
        "postgres-filter-wrong-country",
        ShopType::AuctionHouse,
        ShopPartnerStatus::Partnered,
        isocountry::CountryCode::USA,
    );
    let now = OffsetDateTime::now_utc();

    let mut tx = begin(&unit_of_work).await;
    for shop in [&matching, &wrong_type, &wrong_status, &wrong_country] {
        match shops.in_transaction(&mut tx).insert(shop).await {
            Ok(_) => {}
            Err(error) => panic!("failed to insert search filter shop: {error:?}"),
        }
    }
    commit(tx).await;
    set_shop_timestamps(&pool, matching.id(), now, now).await;
    set_shop_timestamps(&pool, wrong_type.id(), now, now).await;
    set_shop_timestamps(&pool, wrong_status.id(), now, now).await;
    set_shop_timestamps(&pool, wrong_country.id(), now, now).await;

    let mut tx = begin(&unit_of_work).await;
    let result = match search
        .in_transaction(&mut tx)
        .search(&SearchShopsRequest {
            search: ShopSearch {
                shop_name_query: Some(text_query("postgres-filter")),
                shop_type_query: std::collections::HashSet::from([ShopType::AuctionHouse]).into(),
                partner_status_query: std::collections::HashSet::from([
                    ShopPartnerStatus::Partnered,
                ])
                .into(),
                countries: std::collections::HashSet::from([isocountry::CountryCode::DEU]).into(),
                continents: std::collections::HashSet::from([Continent::Europe]).into(),
                created: Some(RangeQuery {
                    min: Some(now - Duration::days(1)),
                    max: Some(now + Duration::days(1)),
                }),
                updated: Some(RangeQuery {
                    min: Some(now - Duration::days(1)),
                    max: Some(now + Duration::days(1)),
                }),
            },
            sort: Some(Sort {
                sort: SortShopField::Updated,
                order: SortOrder::Desc,
            }),
            cursor: Some(Cursor::<ShopId> {
                size: 10,
                search_after: None,
            }),
        })
        .await
    {
        Ok(result) => result,
        Err(error) => panic!("failed to search filtered shops: {error:?}"),
    };
    commit(tx).await;

    assert_eq!(1, result.items.len());
    assert_eq!(matching.id(), result.items[0].shop_id);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_sort_search_by_created_desc() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let shops = SqlxShopRepositoryFactory::new();
    let search = SqlxShopSearchReaderFactory::new();
    let older = sample_shop("postgres-created-sort-older");
    let newer = sample_shop("postgres-created-sort-newer");
    let now = OffsetDateTime::now_utc();

    let mut tx = begin(&unit_of_work).await;
    for shop in [&older, &newer] {
        match shops.in_transaction(&mut tx).insert(shop).await {
            Ok(_) => {}
            Err(error) => panic!("failed to insert created sort shop: {error:?}"),
        }
    }
    commit(tx).await;
    set_shop_timestamps(&pool, older.id(), now - Duration::days(2), now).await;
    set_shop_timestamps(&pool, newer.id(), now - Duration::days(1), now).await;

    let mut tx = begin(&unit_of_work).await;
    let result = match search
        .in_transaction(&mut tx)
        .search(&SearchShopsRequest {
            search: ShopSearch {
                shop_name_query: Some(text_query("postgres-created-sort")),
                ..Default::default()
            },
            sort: Some(Sort {
                sort: SortShopField::Created,
                order: SortOrder::Desc,
            }),
            cursor: Some(Cursor::<ShopId> {
                size: 10,
                search_after: None,
            }),
        })
        .await
    {
        Ok(result) => result,
        Err(error) => panic!("failed to sort by created: {error:?}"),
    };
    commit(tx).await;

    assert_eq!(newer.id(), result.items[0].shop_id);
    assert_eq!(older.id(), result.items[1].shop_id);
}

const BUSINESS_SCHEMA: test_api::Postgres = test_api::Postgres::new("migrations");

fn sample_shop(slug: &str) -> Shop {
    let mut shop = Shop::create(new_shop(slug));
    let _ = shop.publish();
    shop
}

fn search_shop(
    slug: &str,
    shop_type: ShopType,
    partner_status: ShopPartnerStatus,
    country: isocountry::CountryCode,
) -> Shop {
    let mut input = new_shop(slug);
    input.shop_type = shop_type;
    input.partner_status = partner_status;
    input.address = Some(ShopAddress {
        structured: StructuredAddress {
            addressline: Some("Main 1".to_owned()),
            addressline_extra: None,
            locality: Some("Berlin".to_owned()),
            region: None,
            postal_code: Some("10115".to_owned()),
            country: Some(country),
            continent: Some(Continent::from(country)),
        },
        geo: Some(GeoAddress {
            lat: 52.5,
            lon: 13.4,
        }),
    });
    let mut shop = Shop::create(input);
    let _ = shop.publish();
    shop
}

fn text_query(value: &str) -> TextQuery<0> {
    match TextQuery::try_from(value) {
        Ok(query) => query,
        Err(error) => panic!("invalid test text query: {error}"),
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

async fn set_shop_lifecycle(pool: &sqlx::PgPool, shop_id: ShopId, lifecycle: &str) {
    if let Err(error) = sqlx::query("UPDATE shops SET lifecycle = $1 WHERE shop_id = $2")
        .bind(lifecycle)
        .bind(uuid::Uuid::from(shop_id))
        .execute(pool)
        .await
    {
        panic!("failed to set shop lifecycle: {error}");
    }
}

async fn set_shop_timestamps(
    pool: &sqlx::PgPool,
    shop_id: ShopId,
    created: OffsetDateTime,
    updated: OffsetDateTime,
) {
    let result = sqlx::query("UPDATE shops SET created = $1, updated = $2 WHERE shop_id = $3")
        .bind(created)
        .bind(updated)
        .bind(uuid::Uuid::from(shop_id))
        .execute(pool)
        .await;

    if let Err(error) = result {
        panic!("failed to set shop timestamps: {error}");
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
