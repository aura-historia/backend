use aws_tests_common::get_cfn_output;
use common::{
    api::collection::PutCollectionData,
    language::data::LocalizedTextData,
    product_state::domain::ProductState,
    sort::{Sort, SortOrder},
};
use fake::{Fake, Faker};
use opensearch::{GetParts, IndexParts, params::Refresh};
use product::core::sort_product_field::SortProductField;
use product::data::{product_state_data::ProductStateData, put_data::PutProductData};
use product::dynamodb::{
    product_record::ProductRecord,
    product_state_record::ProductStateRecord,
    repository::{ProductDynamoDbRepository, ProductDynamoDbRepositoryImpl},
};
use product::opensearch::{
    product_document::ProductDocument,
    repository::{ProductOpenSearchRepository, ProductOpenSearchRepositoryImpl},
};
use product::{core::product_search::ProductSearch, dynamodb::product_record::mk_pk};
use serde::de::DeserializeOwned;
use shop::core::shop::Shop;
use shop::dynamodb::{
    repository::{ShopDynamoDbRepository, ShopDynamoDbRepositoryImpl},
    shop_record::ShopRecord,
};
use staging_tests::{get_dynamodb_client, get_opensearch_client, staging_test};
use std::time::{Duration, Instant};

pub async fn read_by_id<T: DeserializeOwned>(index: &str, id: impl Into<String>) -> T {
    let get_response = get_opensearch_client()
        .await
        .get(GetParts::IndexId(index, &id.into()))
        .send()
        .await
        .unwrap();
    assert!(get_response.status_code().is_success());

    let response_doc: serde_json::Value = get_response.json().await.unwrap();
    serde_json::from_value(response_doc["_source"].clone()).unwrap()
}

pub async fn refresh_index(index: &str) {
    get_opensearch_client()
        .await
        .index(IndexParts::Index(index))
        .refresh(Refresh::True)
        .send()
        .await
        .unwrap();
}

async fn prepare_test_shop() -> Shop {
    let stack = get_cfn_output();
    let shop = Faker.fake::<Shop>();

    let dynamodb_repository =
        ShopDynamoDbRepositoryImpl::new(get_dynamodb_client().await, &stack.dynamodb_table_1_name);
    let mut shop_records = ShopRecord::clone_from_shop_as_shop_domain_records(&shop);
    shop_records.push(ShopRecord::from_shop_as_shop_id_record(shop.clone()));
    let _ = dynamodb_repository
        .put_shop_records_transact(shop_records)
        .await
        .unwrap();

    shop
}

#[staging_test]
async fn should_materialize_product_in_dynamodb_when_put_new_item() {
    let stack = get_cfn_output();
    let shop = prepare_test_shop().await;
    let mut put_product_data: PutProductData = Faker.fake();
    put_product_data
        .url
        .set_host(Some(shop.domains.into_iter().next().unwrap().as_str()))
        .unwrap();

    let url = format!("{}/api/v1/products", stack.api_gateway_endpoint_url);
    let response = reqwest::Client::new()
        .put(url)
        .json(&PutCollectionData {
            items: vec![put_product_data.clone()],
        })
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());

    let dynamodb_client = get_dynamodb_client().await;
    let repository =
        ProductDynamoDbRepositoryImpl::new(dynamodb_client, &stack.dynamodb_table_1_name);

    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let materialized = repository
            .get_product_record(&shop.shop_id, &put_product_data.shops_product_id)
            .await
            .unwrap();

        if let Some(materialized) = materialized {
            assert_eq!(shop.shop_id, materialized.shop_id);
            assert_eq!(
                put_product_data.shops_product_id,
                materialized.shops_product_id
            );
            assert_eq!(put_product_data.url, materialized.url);
            break;
        }

        if Instant::now() >= deadline {
            panic!(
                "Timeout: ProductRecord with shop_id '{}' and shops_product_id '{}' not found in DynamoDB after 60 seconds",
                shop.shop_id, put_product_data.shops_product_id
            );
        }

        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

#[staging_test]
async fn should_materialize_product_in_dynamodb_for_update_product_command() {
    let stack = get_cfn_output();
    let dynamodb_client = get_dynamodb_client().await;
    let repository =
        ProductDynamoDbRepositoryImpl::new(dynamodb_client, &stack.dynamodb_table_1_name);
    let shop = prepare_test_shop().await;

    let mut materialized_old: ProductRecord = Faker.fake();
    materialized_old.pk = mk_pk(&shop.shop_id, &materialized_old.shops_product_id);
    materialized_old.shop_id = shop.shop_id;
    materialized_old
        .url
        .set_host(Some(shop.domains.into_iter().next().unwrap().as_str()))
        .unwrap();
    let insert_res = repository
        .put_product_records([materialized_old.clone()].into())
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap_or_default().is_empty());
    tokio::time::sleep(Duration::from_secs(3)).await;

    let new_state = match materialized_old.state {
        ProductStateRecord::Available => ProductStateData::Sold,
        _ => ProductStateData::Available,
    };
    let put_product_data = PutProductData {
        shops_product_id: materialized_old.shops_product_id,
        title: Faker.fake(),
        description: None,
        price: None,
        state: new_state,
        url: materialized_old.url,
        images: materialized_old
            .images
            .into_iter()
            .map(|image| image.url)
            .collect(),
    };

    let url = format!("{}/api/v1/products", stack.api_gateway_endpoint_url);
    let response = reqwest::Client::new()
        .put(url)
        .json(&PutCollectionData {
            items: vec![put_product_data.clone()],
        })
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());

    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let materialized = repository
            .get_product_record(&shop.shop_id, &put_product_data.shops_product_id)
            .await
            .unwrap();

        if let Some(materialized) = materialized
            && ProductState::from(new_state) == ProductState::from(materialized.state)
        {
            assert_eq!(shop.shop_id, materialized.shop_id);
            assert_eq!(
                put_product_data.shops_product_id,
                materialized.shops_product_id
            );
            assert_eq!(
                ProductState::from(new_state),
                ProductState::from(materialized.state)
            );
            break;
        }

        if Instant::now() >= deadline {
            panic!(
                "Timeout: ProductRecord with shop_id '{}' and shops_product_id '{}' \
                    has not been updated in DynamoDB or been updated with expected state after 60 seconds",
                shop.shop_id, put_product_data.shops_product_id
            );
        }

        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

#[staging_test]
async fn should_materialize_product_in_opensearch_for_create_product_command() {
    let stack = get_cfn_output();
    let mut put_product_data: PutProductData = Faker.fake();
    let shop = prepare_test_shop().await;

    put_product_data.title = LocalizedTextData {
        text: "Exactly the expected title".to_string(),
        language: common::language::data::LanguageData::En,
    };
    put_product_data
        .url
        .set_host(Some(shop.domains.into_iter().next().unwrap().as_str()))
        .unwrap();

    let url = format!("{}/api/v1/products", stack.api_gateway_endpoint_url);
    let response = reqwest::Client::new()
        .put(url)
        .json(&PutCollectionData {
            items: vec![put_product_data.clone()],
        })
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());

    let opensearch_client = get_opensearch_client().await;
    let repository = ProductOpenSearchRepositoryImpl::new(opensearch_client);
    refresh_index("products").await;

    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let materialized = repository
            .search_product_documents(
                &ProductSearch {
                    language: common::language::domain::Language::En,
                    currency: common::currency::domain::Currency::Eur,
                    product_query: "Exactly the expected title".try_into().unwrap(),
                    shop_name_query: None,
                    shop_type_query: Default::default(),
                    price_query: None,
                    state_query: Default::default(),
                    origin_year_query: None,
                    authenticity_query: Default::default(),
                    condition_query: Default::default(),
                    provenance_query: Default::default(),
                    restoration_query: Default::default(),
                    created_query: None,
                    updated_query: None,
                },
                &Sort {
                    sort: SortProductField::Score,
                    order: SortOrder::Desc,
                },
                &None,
            )
            .await
            .unwrap()
            .hits
            .hits
            .first()
            .cloned();

        if let Some(materialized) = materialized {
            assert_eq!(shop.shop_id, materialized.source.shop_id);
            assert_eq!(
                put_product_data.shops_product_id,
                materialized.source.shops_product_id
            );
            assert_eq!(put_product_data.url, materialized.source.url);
            break;
        }

        if Instant::now() >= deadline {
            panic!(
                "Timeout: ProductDocument with shop_id '{}' and shops_product_id '{}' not found in OpenSearch after 60 seconds",
                shop.shop_id, put_product_data.shops_product_id
            );
        }

        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

#[staging_test]
async fn should_materialize_product_in_opensearch_for_update_product_command() {
    let stack = get_cfn_output();
    let shop = prepare_test_shop().await;

    // we also need to ingest materialized into DynamoDB because product-write-lambda-update performs validity and existence checks in the primary data-store
    let dynamodb_client = get_dynamodb_client().await;
    let repository =
        ProductDynamoDbRepositoryImpl::new(dynamodb_client, &stack.dynamodb_table_1_name);
    let mut materialized_ddb_old: ProductRecord = Faker.fake();
    materialized_ddb_old.pk = mk_pk(&shop.shop_id, &materialized_ddb_old.shops_product_id);
    materialized_ddb_old.shop_id = shop.shop_id;
    materialized_ddb_old.title_en = Some("Exactly the expected title".to_string());
    materialized_ddb_old
        .url
        .set_host(Some(shop.domains.into_iter().next().unwrap().as_str()))
        .unwrap();
    let insert_res = repository
        .put_product_records([materialized_ddb_old.clone()].into())
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap_or_default().is_empty());

    let opensearch_client = get_opensearch_client().await;
    let repository = ProductOpenSearchRepositoryImpl::new(opensearch_client);
    let materialized_os_old: ProductDocument = materialized_ddb_old.clone().into();
    let insert_res = repository
        .create_product_documents(vec![materialized_os_old.clone()])
        .await
        .unwrap();
    assert!(!insert_res.errors);
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_secs(10)).await;

    let new_state = match materialized_ddb_old.state {
        ProductStateRecord::Available => ProductStateData::Sold,
        _ => ProductStateData::Available,
    };
    let put_product_data = PutProductData {
        shops_product_id: materialized_ddb_old.shops_product_id,
        title: Faker.fake(),
        description: None,
        price: None,
        state: new_state,
        url: materialized_ddb_old.url,
        images: materialized_ddb_old
            .images
            .into_iter()
            .map(|image| image.url)
            .collect(),
    };

    let url = format!("{}/api/v1/products", stack.api_gateway_endpoint_url);
    let response = reqwest::Client::new()
        .put(url)
        .json(&PutCollectionData {
            items: vec![put_product_data.clone()],
        })
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());

    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        refresh_index("products").await;
        let materialized = repository
            .search_product_documents(
                &ProductSearch {
                    language: common::language::domain::Language::En,
                    currency: common::currency::domain::Currency::Usd,
                    product_query: "Exactly the expected title".try_into().unwrap(),
                    shop_name_query: None,
                    shop_type_query: Default::default(),
                    price_query: None,
                    state_query: Default::default(),
                    origin_year_query: None,
                    authenticity_query: Default::default(),
                    condition_query: Default::default(),
                    provenance_query: Default::default(),
                    restoration_query: Default::default(),
                    created_query: None,
                    updated_query: None,
                },
                &Sort {
                    sort: SortProductField::Score,
                    order: SortOrder::Desc,
                },
                &None,
            )
            .await
            .unwrap()
            .hits
            .hits
            .first()
            .cloned();

        if let Some(materialized) = materialized
            && ProductState::from(new_state) == ProductState::from(materialized.source.state)
        {
            assert_eq!(shop.shop_id, materialized.source.shop_id);
            assert_eq!(
                put_product_data.shops_product_id,
                materialized.source.shops_product_id
            );
            assert_eq!(
                ProductState::from(new_state),
                ProductState::from(materialized.source.state)
            );
            break;
        }

        if Instant::now() >= deadline {
            panic!(
                "Timeout: ProductDocument with shop_id '{}' and shops_product_id '{}' \
                    has not been updated in OpenSearch or been updated with expected state after 60 seconds",
                shop.shop_id, put_product_data.shops_product_id
            );
        }

        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}
