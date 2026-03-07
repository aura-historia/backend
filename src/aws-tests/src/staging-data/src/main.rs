use aws_tests_common::get_cfn_output;
use common::{
    api::collection::PutCollectionData,
    language::domain::Language,
    pagination::cursor::Cursor,
    sort::{Sort, SortOrder},
    year::Year,
};
use fake::{
    Fake, Faker,
    rand::{self, seq::IndexedRandom},
};
use opensearch::indices::IndicesRefreshParts;
use product::{
    core::sort_product_field::SortProductField,
    data::put_data::PutProductData,
    dynamodb::{
        authenticity_record::AuthenticityRecord,
        condition_record::ConditionRecord,
        product_update_record::ProductRecordUpdate,
        provenance_record::ProvenanceRecord,
        repository::{ProductDynamoDbRepository, ProductDynamoDbRepositoryImpl},
        restoration_record::RestorationRecord,
    },
    opensearch::{
        product_update_document::ProductUpdateDocument,
        repository::{ProductOpenSearchRepository, ProductOpenSearchRepositoryImpl},
    },
};
use product_classification::category::{
    core::Category,
    dynamodb_repository::CategoryDynamoDbRepositoryImpl,
    opensearch_repository::CategoryOpenSearchRepositoryImpl,
    service::{CategoryService, CategoryServiceImpl},
};
use product_classification::period::{
    core::Period,
    dynamodb_repository::PeriodDynamoDbRepositoryImpl,
    opensearch_repository::PeriodOpenSearchRepositoryImpl,
    service::{PeriodService, PeriodServiceImpl},
};
use shop::data::{get_shop_data::GetShopData, post_shop_data::PostShopData};
use staging_tests::get_dynamodb_client;
use std::{collections::HashMap, time::Duration};
use time::OffsetDateTime;

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("aura-historia-staging-data")
        .build()
        .unwrap()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    staging_tests::reset().await;
    let shops = populate_shops().await;
    populate_products(shops).await;
    let _ = populate_categories().await;
    let _ = populate_periods().await;

    Ok(())
}

async fn populate_products(shops: Vec<GetShopData>) {
    println!("Populating products...");
    let stack = get_cfn_output();
    let put_products_url = format!("{}/api/v1/products", stack.api_gateway_endpoint_url);

    let shop_domains = shops
        .into_iter()
        .flat_map(|shop| shop.domains)
        .collect::<Vec<_>>();

    // create products
    let mut products = fake::vec![PutProductData; 142];
    for product in &mut products {
        let host = shop_domains.choose(&mut fake::rand::rng()).unwrap().clone();
        product.url.set_host(Some(host.as_str())).unwrap();
    }

    let mut payload = PutCollectionData {
        items: products.clone(),
    };
    let response = http_client()
        .put(&put_products_url)
        .json(&payload)
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());
    tokio::time::sleep(Duration::from_secs(30)).await;

    // put updates
    for i in 0..10 {
        for product in &mut payload.items {
            if rand::random_range(0..3) < 1 {
                product.state = Faker.fake();
            }
            if rand::random_range(0..3) < 2 {
                product.price = Some(Faker.fake());
            }
        }
        let collection = PutCollectionData {
            items: payload.items.clone(),
        };
        let response = http_client()
            .put(&put_products_url)
            .json(&collection)
            .send()
            .await
            .unwrap();
        assert_eq!(200, response.status());
        tokio::time::sleep(Duration::from_secs(30)).await;
        println!("Finished products' update-iteration {i}.");
    }
    refresh_index("products").await;
    println!("Populated products.");

    println!("Enriching products...");
    let dynamodb_repository = ProductDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &get_cfn_output().dynamodb_table_1_name,
    );
    let opensearch = common::opensearch::client::load_client()
        .await
        .expect("shouldn't fail loading OpenSearch-Client");
    let opensearch_repository = ProductOpenSearchRepositoryImpl::new(&opensearch);
    let categories = Category::load_categories();
    let periods = Period::load_periods();
    for product in opensearch_repository
        .search_product_documents(
            &Default::default(),
            &Sort {
                sort: SortProductField::Created,
                order: SortOrder::Asc,
            },
            &Some(Cursor {
                size: products.len() as u64,
                search_after: None,
            }),
        )
        .await
        .unwrap()
        .hits
        .hits
        .into_iter()
        .map(|hit| hit.source)
    {
        let category = categories.choose(&mut fake::rand::rng()).unwrap().clone();
        let period = periods.choose(&mut fake::rand::rng()).unwrap().clone();
        let origin_year_min = Some(Faker.fake::<Year>());
        let origin_year = Some(Faker.fake::<Year>());
        let origin_year_max = Some(Faker.fake::<Year>());
        let authenticity = Some(Faker.fake::<AuthenticityRecord>());
        let condition = Some(Faker.fake::<ConditionRecord>());
        let provenance = Some(Faker.fake::<ProvenanceRecord>());
        let restoration = Some(Faker.fake::<RestorationRecord>());
        let ddb_update = ProductRecordUpdate {
            event_id: None,
            price_native: None,
            price_eur: None,
            price_usd: None,
            price_gbp: None,
            price_aud: None,
            price_cad: None,
            price_nzd: None,
            state: None,
            category_id: Some(category.category_id.clone()),
            period_id: Some(period.period_id.clone()),
            category_name_de: Some(
                category
                    .display_name
                    .get(&Language::De)
                    .unwrap()
                    .to_string(),
            ),
            category_name_en: Some(
                category
                    .display_name
                    .get(&Language::En)
                    .unwrap()
                    .to_string(),
            ),
            category_name_fr: Some(
                category
                    .display_name
                    .get(&Language::Fr)
                    .unwrap()
                    .to_string(),
            ),
            category_name_es: Some(
                category
                    .display_name
                    .get(&Language::Es)
                    .unwrap()
                    .to_string(),
            ),
            category_name_it: Some(
                category
                    .display_name
                    .get(&Language::It)
                    .unwrap()
                    .to_string(),
            ),
            period_name_de: Some(period.display_name.get(&Language::De).unwrap().to_string()),
            period_name_en: Some(period.display_name.get(&Language::En).unwrap().to_string()),
            period_name_fr: Some(period.display_name.get(&Language::Fr).unwrap().to_string()),
            period_name_es: Some(period.display_name.get(&Language::Es).unwrap().to_string()),
            period_name_it: Some(period.display_name.get(&Language::It).unwrap().to_string()),
            title_de: None,
            title_en: None,
            title_fr: None,
            title_es: None,
            title_it: None,
            description_de: None,
            description_en: None,
            description_fr: None,
            description_es: None,
            description_it: None,
            images: None,
            text_embedding: None,
            origin_year_min,
            origin_year,
            origin_year_max,
            authenticity,
            condition,
            provenance,
            restoration,
            updated: OffsetDateTime::now_utc(),
        };
        let os_update = ProductUpdateDocument {
            event_id: None,
            price_eur: None,
            price_usd: None,
            price_gbp: None,
            price_aud: None,
            price_cad: None,
            price_nzd: None,
            state: None,
            category_id: Some(category.category_id),
            period_id: Some(period.period_id),
            category_name_de: Some(
                category
                    .display_name
                    .get(&Language::De)
                    .unwrap()
                    .to_string(),
            ),
            category_name_en: Some(
                category
                    .display_name
                    .get(&Language::En)
                    .unwrap()
                    .to_string(),
            ),
            category_name_fr: Some(
                category
                    .display_name
                    .get(&Language::Fr)
                    .unwrap()
                    .to_string(),
            ),
            category_name_es: Some(
                category
                    .display_name
                    .get(&Language::Es)
                    .unwrap()
                    .to_string(),
            ),
            category_name_it: Some(
                category
                    .display_name
                    .get(&Language::It)
                    .unwrap()
                    .to_string(),
            ),
            period_name_de: Some(period.display_name.get(&Language::De).unwrap().to_string()),
            period_name_en: Some(period.display_name.get(&Language::En).unwrap().to_string()),
            period_name_fr: Some(period.display_name.get(&Language::Fr).unwrap().to_string()),
            period_name_es: Some(period.display_name.get(&Language::Es).unwrap().to_string()),
            period_name_it: Some(period.display_name.get(&Language::It).unwrap().to_string()),
            title_de: None,
            title_en: None,
            title_fr: None,
            title_es: None,
            title_it: None,
            description_de: None,
            description_en: None,
            description_fr: None,
            description_es: None,
            description_it: None,
            images: None,
            text_embedding: None,
            origin_year_min,
            origin_year,
            origin_year_max,
            authenticity: authenticity.map(Into::into),
            condition: condition.map(Into::into),
            provenance: provenance.map(Into::into),
            restoration: restoration.map(Into::into),
            updated: OffsetDateTime::now_utc(),
        };
        dynamodb_repository
            .update_product_record(&product.shop_id, &product.shops_product_id, ddb_update)
            .await
            .unwrap();
        opensearch_repository
            .update_product_documents(HashMap::from_iter([(product.product_id, os_update)]))
            .await
            .unwrap();
    }
    refresh_index("products").await;
    println!("Enriched products.");
}

async fn populate_shops() -> Vec<GetShopData> {
    println!("Populating shops...");
    let stack = get_cfn_output();

    let http = http_client();
    let post_shop_url = format!("{}/api/v1/shops", stack.api_gateway_endpoint_url);
    let mut shops = vec![];
    for _ in 0..42 {
        let mut post_shop_data = Faker.fake::<PostShopData>();
        post_shop_data.domains.insert(Faker.fake());
        let shop = http
            .post(&post_shop_url)
            .json(&post_shop_data)
            .send()
            .await
            .unwrap()
            .json::<GetShopData>()
            .await
            .unwrap();
        shops.push(shop);
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    tokio::time::sleep(Duration::from_secs(30)).await;
    println!("Populated shops.");
    shops
}

async fn populate_categories() -> Vec<Category> {
    let dynamodb_repository = CategoryDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &get_cfn_output().dynamodb_table_1_name,
    );
    let opensearch = common::opensearch::client::load_client()
        .await
        .expect("shouldn't fail loading OpenSearch-Client");
    let opensearch_repository = CategoryOpenSearchRepositoryImpl::new(&opensearch);
    let category_service = CategoryServiceImpl::new(&dynamodb_repository, &opensearch_repository);

    let categories = Category::load_categories();
    for category in &categories {
        let _ = category_service
            .upsert_category(category.clone())
            .await
            .unwrap();
    }
    refresh_index("categories").await;
    categories
}

async fn populate_periods() -> Vec<Period> {
    let dynamodb_repository = PeriodDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &get_cfn_output().dynamodb_table_1_name,
    );
    let opensearch = common::opensearch::client::load_client()
        .await
        .expect("shouldn't fail loading OpenSearch-Client");
    let opensearch_repository = PeriodOpenSearchRepositoryImpl::new(&opensearch);
    let period_service = PeriodServiceImpl::new(&dynamodb_repository, &opensearch_repository);

    let periods = Period::load_periods();
    for period in &periods {
        let _ = period_service.upsert_period(period.clone()).await.unwrap();
    }
    refresh_index("periods").await;
    periods
}

pub async fn refresh_index(index: &str) {
    common::opensearch::client::load_client()
        .await
        .expect("shouldn't fail loading OpenSearch-Client")
        .indices()
        .refresh(IndicesRefreshParts::Index(&[index]))
        .send()
        .await
        .unwrap()
        .error_for_status_code()
        .unwrap();
}
