use common::currency::domain::Currency;
use common::distance::domain::{Distance, DistanceUnit, GeoDistanceQuery};
use common::event_id::EventId;
use common::language::domain::Language;
use common::pagination::cursor::Cursor;
use common::price::domain::{MonetaryAmount, Price};
use common::product_id::ProductId;
use common::product_lifecycle::domain::ProductLifecycle;
use common::product_slug_id::ProductSlugId;
use common::product_state::domain::ProductState;

use common::query::range_query::RangeQuery;
use common::seller_slug_id::SellerSlugId;
use common::shop_id::ShopId;
use common::shop_name::ShopName;
use common::shop_slug_id::ShopSlugId;
use common::shops_product_id::ShopsProductId;
use common::sort::{Sort, SortOrder};
use opensearch::IndexParts;
use product_core::product_search::ProductSearch;
use product_core::sort_product_field::SortProductField;
use product_service::ports::ProductSearchReader;
use product_service::use_cases::queries::search_products::{
    SearchProductsRequest, SearchProductsResult,
};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::io::{Error as IoError, ErrorKind};
use test_api::{
    IntegrationTestService, OpenSearch, aura_integration_test, get_opensearch_client, refresh_index,
};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use time::macros::datetime;

const PRODUCTS_INDEX: &str = "products";

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[aura_integration_test(services = [OpenSearch()])]
async fn should_search_products_when_text_query_matches() {
    assert_ok(should_search_products_when_text_query_matches_impl().await);
}

#[aura_integration_test(services = [OpenSearch()])]
async fn should_search_products_when_any_product_query_matches() {
    assert_ok(should_search_products_when_any_product_query_matches_impl().await);
}

#[aura_integration_test(services = [OpenSearch()])]
async fn should_filter_products_when_structural_filters_are_given() {
    assert_ok(should_filter_products_when_structural_filters_are_given_impl().await);
}

#[aura_integration_test(services = [OpenSearch()])]
async fn should_filter_products_when_location_filters_are_given() {
    assert_ok(should_filter_products_when_location_filters_are_given_impl().await);
}

#[aura_integration_test(services = [OpenSearch()])]
async fn should_filter_products_when_auction_ranges_or_empty_query_are_given() {
    assert_ok(should_filter_products_when_auction_ranges_or_empty_query_are_given_impl().await);
}

#[aura_integration_test(services = [OpenSearch()])]
async fn should_page_products_when_sorted_by_price() {
    assert_ok(should_page_products_when_sorted_by_price_impl().await);
}

async fn should_search_products_when_text_query_matches_impl() -> TestResult {
    let product = product_doc(ProductSeed {
        title_en: "Renaissance walnut cabinet".to_owned(),
        title_de: Some("Renaissance Nussbaum Kabinett".to_owned()),
        price_usd: Some(125),
        ..ProductSeed::new("basic-text")
    })?;
    index_products([product.document.clone()]).await?;

    let result = search(
        ProductSearch::new(Language::De, Currency::Usd)
            .with_product_query("Renaissance Nussbaum".try_into()?),
        None,
        None,
    )
    .await?;

    assert_eq!(Some(1), result.total);
    assert_eq!(vec![product.product_id], product_ids(&result));
    assert_eq!(product.product_id, result.items[0].product_id);
    assert_eq!(product.shop_id, result.items[0].shop_id);
    assert_eq!(product.seller_id, result.items[0].seller_id);
    assert_eq!(product.shops_product_id, result.items[0].shops_product_id);
    assert_eq!(
        ShopName::from(product.shop_name.as_str()),
        result.items[0].shop_name
    );
    assert_eq!(product.shop_slug_id, result.items[0].shop_slug_id);
    assert_eq!(
        Some(Language::De),
        result.items[0]
            .title
            .as_ref()
            .map(|title| title.localization)
    );
    assert_eq!(
        Some("Renaissance Nussbaum Kabinett"),
        result.items[0]
            .title
            .as_ref()
            .map(|title| title.payload.as_ref())
    );
    assert_eq!(
        Some(Price::new(MonetaryAmount::from(125_u64), Currency::Usd)),
        result.items[0].price
    );
    assert_eq!(ProductState::Available, result.items[0].state);
    assert_eq!(ProductLifecycle::Active, result.items[0].lifecycle);
    assert_eq!(product.updated, result.items[0].updated);
    Ok(())
}

async fn should_search_products_when_any_product_query_matches_impl() -> TestResult {
    let madonna = product_doc(ProductSeed {
        title_en: "Madonna oil painting renaissance artwork".to_owned(),
        ..ProductSeed::new("madonna")
    })?;
    let virgin_mary = product_doc(ProductSeed {
        title_en: "Virgin Mary oil painting antique icon".to_owned(),
        ..ProductSeed::new("virgin-mary")
    })?;
    let unrelated = product_doc(ProductSeed {
        title_en: "Bronze garden sculpture".to_owned(),
        ..ProductSeed::new("unrelated")
    })?;
    index_products([
        madonna.document.clone(),
        virgin_mary.document.clone(),
        unrelated.document,
    ])
    .await?;

    let result = search(
        ProductSearch::new(Language::En, Currency::Eur)
            .with_product_query("Madonna oil painting".try_into()?)
            .with_product_query("Virgin Mary oil painting".try_into()?),
        None,
        None,
    )
    .await?;

    assert_eq!(
        HashSet::from([madonna.product_id, virgin_mary.product_id]),
        product_ids(&result).into_iter().collect()
    );
    Ok(())
}

async fn should_filter_products_when_structural_filters_are_given_impl() -> TestResult {
    let target = product_doc(ProductSeed {
        title_en: "Imperial filter test cabinet".to_owned(),
        title_de: Some("Imperial Filter Test Kabinett".to_owned()),
        shop_name: "Sotheby's".to_owned(),
        seller_name: "Imperial Antiques".to_owned(),
        shop_slug_id: ShopSlugId::from("sothebys"),
        seller_slug_id: SellerSlugId::from("imperial-antiques"),
        shop_type: "AUCTION_HOUSE",
        state: "LISTED",
        price_eur: Some(550),
        ..ProductSeed::new("structural-target")
    })?;
    let excluded_shop = product_doc(ProductSeed {
        title_en: target.title_en.clone(),
        title_de: target.title_de.clone(),
        shop_name: "Blocked Shop".to_owned(),
        seller_name: target.seller_name.clone(),
        shop_slug_id: target.shop_slug_id.clone(),
        seller_slug_id: target.seller_slug_id.clone(),
        shop_type: target.shop_type,
        state: target.state,
        price_eur: target.price_eur,
        ..ProductSeed::new("structural-excluded-shop")
    })?;
    let wrong_price = product_doc(ProductSeed {
        title_en: target.title_en.clone(),
        title_de: target.title_de.clone(),
        shop_name: target.shop_name.clone(),
        seller_name: target.seller_name.clone(),
        shop_slug_id: target.shop_slug_id.clone(),
        seller_slug_id: target.seller_slug_id.clone(),
        shop_type: target.shop_type,
        state: target.state,
        price_eur: Some(2_000),
        ..ProductSeed::new("structural-wrong-price")
    })?;
    let deleted = product_doc(ProductSeed {
        title_en: target.title_en.clone(),
        lifecycle: "DELETED",
        ..ProductSeed::new("structural-deleted")
    })?;
    index_products([
        target.document.clone(),
        excluded_shop.document,
        wrong_price.document,
        deleted.document,
    ])
    .await?;

    let search_filter = ProductSearch::new(Language::De, Currency::Eur)
        .with_product_query("Imperial Filter Test".try_into()?)
        .with_shop_name_query(HashSet::from([ShopName::from("Sotheby's")]).into())
        .with_exclude_shop_name_query(HashSet::from([ShopName::from("Blocked Shop")]).into())
        .with_seller_name_query(HashSet::from([ShopName::from("Imperial Antiques")]).into())
        .with_shop_slug_id_query(HashSet::from([target.shop_slug_id.clone()]).into())
        .with_seller_slug_id_query(HashSet::from([target.seller_slug_id.clone()]).into())
        .with_shop_type_query(HashSet::from([shop_core::shop_type::ShopType::AuctionHouse]).into())
        .with_state_query(HashSet::from([ProductState::Listed]).into())
        .with_price_query(RangeQuery {
            min: Some(MonetaryAmount::from(500_u64)),
            max: Some(MonetaryAmount::from(600_u64)),
        });

    let result = search(
        search_filter,
        None,
        Some(Cursor {
            size: 20,
            search_after: None,
        }),
    )
    .await?;

    assert_eq!(Some(1), result.total);
    assert_eq!(vec![target.product_id], product_ids(&result));

    let excluded = search(
        ProductSearch::new(Language::En, Currency::Eur)
            .with_exclude_product_id_query(HashSet::from([target.product_id]).into()),
        None,
        Some(Cursor {
            size: 20,
            search_after: None,
        }),
    )
    .await?;
    assert!(!product_ids(&excluded).contains(&target.product_id));
    Ok(())
}

async fn should_filter_products_when_location_filters_are_given_impl() -> TestResult {
    let berlin = product_doc(ProductSeed {
        title_en: "Location filter porcelain vase".to_owned(),
        country: Some("DE"),
        continent: Some("EUROPE"),
        geo_address: Some("52.5200,13.4050"),
        ..ProductSeed::new("location-berlin")
    })?;
    let new_york = product_doc(ProductSeed {
        title_en: "Location filter porcelain vase".to_owned(),
        country: Some("US"),
        continent: Some("NORTH_AMERICA"),
        geo_address: Some("40.7128,-74.0060"),
        ..ProductSeed::new("location-new-york")
    })?;
    index_products([berlin.document.clone(), new_york.document]).await?;

    let country_result = search(
        ProductSearch::new(Language::En, Currency::Eur)
            .with_country_query(HashSet::from([isocountry::CountryCode::DEU]).into()),
        None,
        Some(Cursor {
            size: 20,
            search_after: None,
        }),
    )
    .await?;
    let continent_result = search(
        ProductSearch::new(Language::En, Currency::Eur)
            .with_continent_query(HashSet::from([geo::core::continent::Continent::Europe]).into()),
        None,
        Some(Cursor {
            size: 20,
            search_after: None,
        }),
    )
    .await?;
    let geo_result = search(
        ProductSearch::new(Language::En, Currency::Eur).with_geo_address_distance_query(
            GeoDistanceQuery {
                lat: 52.5200,
                lon: 13.4050,
                distance: Distance {
                    amount: 50.0,
                    unit: DistanceUnit::Kilometers,
                },
            },
        ),
        None,
        Some(Cursor {
            size: 20,
            search_after: None,
        }),
    )
    .await?;

    assert_eq!(vec![berlin.product_id], product_ids(&country_result));
    assert_eq!(vec![berlin.product_id], product_ids(&continent_result));
    assert_eq!(vec![berlin.product_id], product_ids(&geo_result));
    Ok(())
}

async fn should_filter_products_when_auction_ranges_or_empty_query_are_given_impl() -> TestResult {
    let january = product_doc(ProductSeed {
        title_en: "Auction range test item".to_owned(),
        auction_start: Some(datetime!(2026-01-15 10:00 UTC)),
        auction_end: Some(datetime!(2026-01-15 14:00 UTC)),
        ..ProductSeed::new("auction-january")
    })?;
    let june = product_doc(ProductSeed {
        title_en: "Auction range test item".to_owned(),
        auction_start: Some(datetime!(2026-06-20 10:00 UTC)),
        auction_end: Some(datetime!(2026-06-20 14:00 UTC)),
        ..ProductSeed::new("auction-june")
    })?;
    let no_auction = product_doc(ProductSeed {
        title_en: "Auction range test item".to_owned(),
        auction_start: None,
        auction_end: None,
        ..ProductSeed::new("auction-none")
    })?;
    index_products([
        january.document.clone(),
        june.document.clone(),
        no_auction.document,
    ])
    .await?;

    let by_start = search(
        ProductSearch::new(Language::En, Currency::Eur)
            .with_product_query("Auction range".try_into()?)
            .with_auction_start_query(RangeQuery {
                min: Some(datetime!(2026-01-01 0:00 UTC)),
                max: Some(datetime!(2026-01-31 23:59 UTC)),
            }),
        None,
        Some(Cursor {
            size: 20,
            search_after: None,
        }),
    )
    .await?;
    let by_end_without_query = search(
        ProductSearch {
            language: Language::En,
            currency: Currency::Eur,
            auction_end_query: Some(RangeQuery {
                min: Some(datetime!(2026-06-01 0:00 UTC)),
                max: None,
            }),
            ..ProductSearch::new(Language::En, Currency::Eur)
        },
        None,
        Some(Cursor {
            size: 20,
            search_after: None,
        }),
    )
    .await?;

    assert_eq!(vec![january.product_id], product_ids(&by_start));
    assert_eq!(vec![june.product_id], product_ids(&by_end_without_query));
    Ok(())
}

async fn should_page_products_when_sorted_by_price_impl() -> TestResult {
    let products = [100_u64, 200, 300, 400]
        .into_iter()
        .enumerate()
        .map(|(index, price)| {
            product_doc(ProductSeed {
                title_en: "Price sorted page test".to_owned(),
                price_usd: Some(price),
                ..ProductSeed::new(format!("price-page-{index}").as_str())
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    index_products(products.iter().map(|product| product.document.clone())).await?;

    let search_filter = ProductSearch::new(Language::En, Currency::Usd)
        .with_product_query("Price sorted page test".try_into()?);
    let sort = Sort {
        sort: SortProductField::Price,
        order: SortOrder::Asc,
    };
    let first_page = search(
        search_filter.clone(),
        Some(sort),
        Some(Cursor {
            size: 2,
            search_after: None,
        }),
    )
    .await?;
    let second_page = search(
        search_filter,
        Some(sort),
        Some(Cursor {
            size: 2,
            search_after: first_page.cursor.search_after.clone(),
        }),
    )
    .await?;

    let expected_first_page = products
        .iter()
        .take(2)
        .map(|product| product.product_id)
        .collect::<Vec<_>>();
    let expected_second_page = products
        .iter()
        .skip(2)
        .take(2)
        .map(|product| product.product_id)
        .collect::<Vec<_>>();

    assert_eq!(expected_first_page, product_ids(&first_page));
    assert_eq!(expected_second_page, product_ids(&second_page));
    assert!(first_page.cursor.search_after.is_some());
    Ok(())
}

async fn search(
    search: ProductSearch,
    sort: Option<Sort<SortProductField>>,
    cursor: Option<Cursor<Value>>,
) -> Result<SearchProductsResult, product_service::ports::ProductSearchReadError> {
    let reader = product_opensearch::OpenSearchProductSearchReader::new(
        get_opensearch_client().await.clone(),
    );
    reader
        .search(&SearchProductsRequest {
            search,
            sort,
            cursor,
        })
        .await
}

async fn index_products(products: impl IntoIterator<Item = Value>) -> TestResult {
    let client = get_opensearch_client().await;
    for product in products {
        let id = product_id_string(&product)?;
        let response = client
            .index(IndexParts::IndexId(PRODUCTS_INDEX, &id))
            .body(product)
            .send()
            .await?;
        let status = response.status_code();
        let payload = response.text().await?;
        if !status.is_success() {
            return Err(IoError::new(
                ErrorKind::InvalidData,
                format!("failed indexing product {id}: HTTP {status}: {payload}"),
            )
            .into());
        }
    }
    refresh_index(PRODUCTS_INDEX).await;
    Ok(())
}

fn product_ids(result: &SearchProductsResult) -> Vec<ProductId> {
    result.items.iter().map(|item| item.product_id).collect()
}

fn product_id_string(product: &Value) -> Result<String, IoError> {
    product
        .get("productId")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| IoError::new(ErrorKind::InvalidData, "productId missing"))
}

fn assert_ok(result: TestResult) {
    assert_eq!(Ok(()), result.map_err(|err| err.to_string()));
}

#[derive(Clone)]
struct ProductSeed {
    product_id: ProductId,
    product_slug_id: ProductSlugId,
    shop_id: ShopId,
    seller_id: ShopId,
    shops_product_id: ShopsProductId,
    shop_slug_id: ShopSlugId,
    seller_slug_id: SellerSlugId,
    event_id: EventId,
    title_en: String,
    title_de: Option<String>,
    shop_name: String,
    seller_name: String,
    shop_type: &'static str,
    state: &'static str,
    lifecycle: &'static str,
    price_eur: Option<u64>,
    price_usd: Option<u64>,
    country: Option<&'static str>,
    continent: Option<&'static str>,
    geo_address: Option<&'static str>,
    auction_start: Option<OffsetDateTime>,
    auction_end: Option<OffsetDateTime>,
    created: OffsetDateTime,
    updated: OffsetDateTime,
}

struct IndexedProduct {
    product_id: ProductId,
    shop_id: ShopId,
    seller_id: ShopId,
    shops_product_id: ShopsProductId,
    shop_slug_id: ShopSlugId,
    seller_slug_id: SellerSlugId,
    title_en: String,
    title_de: Option<String>,
    shop_name: String,
    seller_name: String,
    shop_type: &'static str,
    state: &'static str,
    price_eur: Option<u64>,
    updated: OffsetDateTime,
    document: Value,
}

impl ProductSeed {
    fn new(slug: &str) -> Self {
        Self {
            product_id: ProductId::new(),
            product_slug_id: ProductSlugId::from(slug),
            shop_id: ShopId::new(),
            seller_id: ShopId::new(),
            shops_product_id: ShopsProductId::from(slug),
            shop_slug_id: ShopSlugId::from("default-shop"),
            seller_slug_id: SellerSlugId::from("default-seller"),
            event_id: EventId::new(),
            title_en: "Default product title".to_owned(),
            title_de: None,
            shop_name: "Default Shop".to_owned(),
            seller_name: "Default Seller".to_owned(),
            shop_type: "COMMERCIAL_DEALER",
            state: "AVAILABLE",
            lifecycle: "ACTIVE",
            price_eur: Some(100),
            price_usd: Some(110),
            country: None,
            continent: None,
            geo_address: None,
            auction_start: None,
            auction_end: None,
            created: datetime!(2025-01-01 0:00 UTC),
            updated: datetime!(2025-01-02 0:00 UTC),
        }
    }
}

fn product_doc(seed: ProductSeed) -> Result<IndexedProduct, time::error::Format> {
    let auction_start = seed
        .auction_start
        .map(|value| value.format(&Rfc3339))
        .transpose()?;
    let auction_end = seed
        .auction_end
        .map(|value| value.format(&Rfc3339))
        .transpose()?;
    let created = seed.created.format(&Rfc3339)?;
    let updated = seed.updated.format(&Rfc3339)?;

    let document = json!({
        "productId": seed.product_id,
        "productSlugId": seed.product_slug_id,
        "shopSlugId": seed.shop_slug_id,
        "sellerSlugId": seed.seller_slug_id,
        "eventId": seed.event_id,
        "shopId": seed.shop_id,
        "sellerId": seed.seller_id,
        "shopsProductId": seed.shops_product_id,
        "shopName": seed.shop_name,
        "sellerName": seed.seller_name,
        "shopType": seed.shop_type,
        "structuredAddressCountry": seed.country,
        "structuredAddressContinent": seed.continent,
        "geoAddress": seed.geo_address,
        "title": {
            "text": seed.title_en,
            "language": "EN"
        },
        "titleDe": seed.title_de,
        "titleEn": seed.title_en,
        "priceEur": seed.price_eur,
        "priceUsd": seed.price_usd,
        "state": seed.state,
        "lifecycle": seed.lifecycle,
        "url": format!("https://shop.example/products/{}", seed.product_slug_id),
        "viewUrl": format!("https://aura.example/products/{}", seed.product_slug_id),
        "auctionStart": auction_start,
        "auctionEnd": auction_end,
        "created": created,
        "updated": updated
    });

    Ok(IndexedProduct {
        product_id: seed.product_id,
        shop_id: seed.shop_id,
        seller_id: seed.seller_id,
        shops_product_id: seed.shops_product_id,
        shop_slug_id: seed.shop_slug_id,
        seller_slug_id: seed.seller_slug_id,
        title_en: seed.title_en,
        title_de: seed.title_de,
        shop_name: seed.shop_name,
        seller_name: seed.seller_name,
        shop_type: seed.shop_type,
        state: seed.state,
        price_eur: seed.price_eur,
        updated: seed.updated,
        document,
    })
}
