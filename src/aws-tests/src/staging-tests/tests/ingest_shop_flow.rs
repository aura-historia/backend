use aws_tests_common::get_cfn_output;
use fake::{Fake, Faker};
use opensearch::{GetParts, IndexParts, params::Refresh};
use serde::de::DeserializeOwned;
use shop::data::patch_shop_data::PatchShopData;
use shop::data::post_shop_data::PostShopData;
use shop::opensearch::shop_document::ShopDocument;
use shop::{
    data::get_shop_data::GetShopData,
    dynamodb::repository::{ShopDynamoDbRepository, ShopDynamoDbRepositoryImpl},
};
use staging_tests::{get_dynamodb_client, get_opensearch_client, staging_test};
use std::time::Duration;

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

#[staging_test]
async fn should_create_shop_dynamodb_and_index_opensearch_when_post_shop_then_patch() {
    let stack = get_cfn_output();
    let dynamodb_client = get_dynamodb_client().await;
    let dynamodb_repository =
        ShopDynamoDbRepositoryImpl::new(dynamodb_client, &stack.dynamodb_table_1_name);

    // POST
    let post_url = format!("{}/api/v1/shops", stack.api_gateway_endpoint_url);
    let post_shop_data = Faker.fake::<PostShopData>();
    let post_response = reqwest::Client::new()
        .post(post_url)
        .json(&post_shop_data)
        .send()
        .await
        .unwrap();
    assert_eq!(201, post_response.status());
    let post_res = post_response.json::<GetShopData>().await.unwrap();
    assert!(
        dynamodb_repository
            .get_shop_record_by_id(&post_res.shop_id)
            .await
            .unwrap()
            .is_some()
    );
    tokio::time::sleep(Duration::from_secs(30)).await;
    let shop_document = read_by_id::<ShopDocument>("shops", post_res.shop_id).await;
    assert_eq!(post_res.name, shop_document.name);

    // PATCH
    let patch_url = format!(
        "{}/api/v1/shops/{}",
        stack.api_gateway_endpoint_url, post_res.shop_id
    );
    let mut patch_shop_data = Faker.fake::<PatchShopData>();
    patch_shop_data.name = Some("Gretel und die 42 Elfen".into());
    let patch_response = reqwest::Client::new()
        .patch(patch_url)
        .json(&patch_shop_data)
        .send()
        .await
        .unwrap();
    assert_eq!(200, patch_response.status());
    let patch_res = patch_response.json::<GetShopData>().await.unwrap();
    let patched_shop_record = dynamodb_repository
        .get_shop_record_by_id(&patch_res.shop_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(patch_shop_data.name.unwrap(), patched_shop_record.name);
    tokio::time::sleep(Duration::from_secs(30)).await;
    let patched_shop_document = read_by_id::<ShopDocument>("shops", patch_res.shop_id).await;
    assert_eq!(patch_res.name, patched_shop_document.name);
}
