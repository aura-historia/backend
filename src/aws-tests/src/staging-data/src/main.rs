use aws_tests_common::get_cfn_output;
use common::{
    has_key::HasKey,
    language::domain::Language,
    pagination::cursor::Cursor,
    price::domain::FixedFxRate,
    product_id::ProductKey,
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
    service::{
        command_service::{CommandProductService, CommandProductServiceImpl},
        product_command::{CreateProductCommand, UpdateProductCommand},
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
use shop::{
    core::shop_type::ShopType,
    data::get_shop_data::GetShopData,
    dynamodb::repository::ShopDynamoDbRepositoryImpl,
    service::command_service::{CommandShopService, CommandShopServiceImpl},
};
use staging_tests::get_dynamodb_client;
use std::{collections::HashMap, time::Duration};
use time::OffsetDateTime;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    staging_tests::reset().await;
    let shops = populate_shops().await;
    populate_products(shops).await;
    let _ = populate_categories().await;
    let _ = populate_periods().await;

    Ok(())
}

async fn create_products(commands: Vec<CreateProductCommand>) {
    let stack = get_cfn_output();
    let dynamodb_client = get_dynamodb_client().await;
    let product_repository =
        ProductDynamoDbRepositoryImpl::new(dynamodb_client, &stack.dynamodb_table_1_name);
    let period_dynamodb_repository =
        PeriodDynamoDbRepositoryImpl::new(dynamodb_client, &stack.dynamodb_table_1_name);
    let period_opensearch_repository =
        PeriodOpenSearchRepositoryImpl::new(staging_tests::get_opensearch_client().await);
    let period_service =
        PeriodServiceImpl::new(&period_dynamodb_repository, &period_opensearch_repository);
    let category_dynamodb_repository =
        CategoryDynamoDbRepositoryImpl::new(dynamodb_client, &stack.dynamodb_table_1_name);
    let category_opensearch_repository =
        CategoryOpenSearchRepositoryImpl::new(staging_tests::get_opensearch_client().await);
    let category_service = CategoryServiceImpl::new(
        &category_dynamodb_repository,
        &category_opensearch_repository,
    );
    let fx_rate = FixedFxRate();
    let command_service = CommandProductServiceImpl::new(
        &product_repository,
        &fx_rate,
        &period_service,
        &category_service,
    );

    let result = command_service.create(commands).await;
    assert!(result.is_empty(), "Some products failed to create");
}

async fn update_products(commands: HashMap<ProductKey, UpdateProductCommand>) {
    let stack = get_cfn_output();
    let dynamodb_client = get_dynamodb_client().await;
    let product_repository =
        ProductDynamoDbRepositoryImpl::new(dynamodb_client, &stack.dynamodb_table_1_name);
    let period_dynamodb_repository =
        PeriodDynamoDbRepositoryImpl::new(dynamodb_client, &stack.dynamodb_table_1_name);
    let period_opensearch_repository =
        PeriodOpenSearchRepositoryImpl::new(staging_tests::get_opensearch_client().await);
    let period_service =
        PeriodServiceImpl::new(&period_dynamodb_repository, &period_opensearch_repository);
    let category_dynamodb_repository =
        CategoryDynamoDbRepositoryImpl::new(dynamodb_client, &stack.dynamodb_table_1_name);
    let category_opensearch_repository =
        CategoryOpenSearchRepositoryImpl::new(staging_tests::get_opensearch_client().await);
    let category_service = CategoryServiceImpl::new(
        &category_dynamodb_repository,
        &category_opensearch_repository,
    );
    let fx_rate = FixedFxRate();
    let command_service = CommandProductServiceImpl::new(
        &product_repository,
        &fx_rate,
        &period_service,
        &category_service,
    );

    let result = command_service.update(commands).await;
    assert!(result.is_empty(), "Some products failed to update");
}

async fn populate_products(shops: Vec<GetShopData>) {
    println!("Populating products...");

    let shop_names: Vec<_> = shops.iter().map(|s| s.name.clone()).collect();
    let shop_types: Vec<_> = shops.iter().map(|s| ShopType::from(s.shop_type)).collect();
    let shop_ids: Vec<_> = shops.iter().map(|s| s.shop_id).collect();

    // create products
    let mut products = fake::vec![CreateProductCommand; 142];
    for product in &mut products {
        let idx = rand::random_range(0..shop_ids.len());
        product.shop_id = shop_ids[idx];
        product.shop_name = shop_names[idx].clone();
        product.shop_type = shop_types[idx];
    }

    create_products(products.clone()).await;
    tokio::time::sleep(Duration::from_secs(30)).await;

    // put updates
    for i in 0..10 {
        let mut update_cmds: HashMap<ProductKey, UpdateProductCommand> = HashMap::new();
        for product in &products {
            let state = if rand::random_range(0..3) < 1 {
                Some(Faker.fake())
            } else {
                None
            };
            let native_price = if rand::random_range(0..3) < 2 {
                Some(Faker.fake())
            } else {
                None
            };
            if state.is_some() || native_price.is_some() {
                update_cmds.insert(
                    product.key(),
                    UpdateProductCommand {
                        native_price,
                        state,
                        native_price_estimate_min: None,
                        native_price_estimate_max: None,
                        url: None,
                        images: None,
                        auction_start: None,
                        auction_end: None,
                        origin_year: None,
                        authenticity: None,
                        condition: None,
                        provenance: None,
                        restoration: None,
                    },
                );
            }
        }
        update_products(update_cmds).await;
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
            price_cny: None,
            price_brl: None,
            price_pln: None,
            price_try: None,
            price_jpy: None,
            price_czk: None,
            price_rub: None,
            price_aed: None,
            price_sar: None,
            price_hkd: None,
            price_sgd: None,
            price_chf: None,
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
            price_estimate_min_native: None,
            price_estimate_min_eur: None,
            price_estimate_min_usd: None,
            price_estimate_min_gbp: None,
            price_estimate_min_aud: None,
            price_estimate_min_cad: None,
            price_estimate_min_nzd: None,
            price_estimate_min_cny: None,
            price_estimate_min_brl: None,
            price_estimate_min_pln: None,
            price_estimate_min_try: None,
            price_estimate_min_jpy: None,
            price_estimate_min_czk: None,
            price_estimate_min_rub: None,
            price_estimate_min_aed: None,
            price_estimate_min_sar: None,
            price_estimate_min_hkd: None,
            price_estimate_min_sgd: None,
            price_estimate_min_chf: None,
            price_estimate_max_native: None,
            price_estimate_max_eur: None,
            price_estimate_max_usd: None,
            price_estimate_max_gbp: None,
            price_estimate_max_aud: None,
            price_estimate_max_cad: None,
            price_estimate_max_nzd: None,
            price_estimate_max_cny: None,
            price_estimate_max_brl: None,
            price_estimate_max_pln: None,
            price_estimate_max_try: None,
            price_estimate_max_jpy: None,
            price_estimate_max_czk: None,
            price_estimate_max_rub: None,
            price_estimate_max_aed: None,
            price_estimate_max_sar: None,
            price_estimate_max_hkd: None,
            price_estimate_max_sgd: None,
            price_estimate_max_chf: None,
            url: None,
            auction_start: None,
            auction_end: None,
            embedding: None,
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
            price_cny: None,
            price_brl: None,
            price_pln: None,
            price_try: None,
            price_jpy: None,
            price_czk: None,
            price_rub: None,
            price_aed: None,
            price_sar: None,
            price_hkd: None,
            price_sgd: None,
            price_chf: None,
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
            price_estimate_min_eur: None,
            price_estimate_min_usd: None,
            price_estimate_min_gbp: None,
            price_estimate_min_aud: None,
            price_estimate_min_cad: None,
            price_estimate_min_nzd: None,
            price_estimate_min_cny: None,
            price_estimate_min_brl: None,
            price_estimate_min_pln: None,
            price_estimate_min_try: None,
            price_estimate_min_jpy: None,
            price_estimate_min_czk: None,
            price_estimate_min_rub: None,
            price_estimate_min_aed: None,
            price_estimate_min_sar: None,
            price_estimate_min_hkd: None,
            price_estimate_min_sgd: None,
            price_estimate_min_chf: None,
            price_estimate_max_eur: None,
            price_estimate_max_usd: None,
            price_estimate_max_gbp: None,
            price_estimate_max_aud: None,
            price_estimate_max_cad: None,
            price_estimate_max_nzd: None,
            price_estimate_max_cny: None,
            price_estimate_max_brl: None,
            price_estimate_max_pln: None,
            price_estimate_max_try: None,
            price_estimate_max_jpy: None,
            price_estimate_max_czk: None,
            price_estimate_max_rub: None,
            price_estimate_max_aed: None,
            price_estimate_max_sar: None,
            price_estimate_max_hkd: None,
            price_estimate_max_sgd: None,
            price_estimate_max_chf: None,
            url: None,
            auction_start: None,
            auction_end: None,
            embedding: None,
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
    let dynamodb_client = get_dynamodb_client().await;
    let shop_repository =
        ShopDynamoDbRepositoryImpl::new(dynamodb_client, &stack.dynamodb_table_1_name);
    let command_service = CommandShopServiceImpl::new(
        &shop_repository,
        &shop::service::geocoding_service::NoopGeocodingService,
    );

    let mut shops = vec![];
    for _ in 0..42 {
        let shop = command_service.create(Faker.fake()).await.unwrap();
        shops.push(GetShopData::from(shop));
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
