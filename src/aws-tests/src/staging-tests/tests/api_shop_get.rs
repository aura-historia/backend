use aws_tests_common::get_cfn_output;
use common::shop_id::ShopId;
use fake::{Fake, Faker};
use shop_dynamodb::{
    repository::{ShopDynamoDbRepository, ShopDynamoDbRepositoryImpl},
    shop_record::ShopRecord,
};
use staging_tests::{get_dynamodb_client, staging_test};
use std::time::Duration;

#[staging_test]
async fn should_respond_200_when_shop_does_exist() {
    let ddb_client = get_dynamodb_client().await;
    let repository =
        ShopDynamoDbRepositoryImpl::new(ddb_client, &get_cfn_output().dynamodb_table_1_name);
    let record = Faker.fake::<ShopRecord>();
    let _ = repository.put_shop_record(record.clone()).await.unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;

    let url = format!(
        "{}/api/v1/shops/{}",
        get_cfn_output().api_gateway_endpoint_url,
        record.shop_id,
    );
    let response = reqwest::get(url).await.unwrap();

    assert_eq!(200, response.status());

    let body = response.json::<serde_json::Value>().await.unwrap();
    assert_eq!(record.shop_id.to_string(), body["shopId"]);
    assert_eq!(record.name.to_string(), body["name"]);
    assert_eq!(record.urls.to_string(), body["url"]);
}

#[staging_test]
async fn should_respond_404_when_shop_does_not_exist() {
    let response = reqwest::get(format!(
        "{}/api/v1/shops/{}",
        get_cfn_output().api_gateway_endpoint_url,
        ShopId::new()
    ))
    .await
    .unwrap();
    assert_eq!(404, response.status());

    let body = response.json::<serde_json::Value>().await.unwrap();
    assert_eq!(404, body["status"]);
    assert_eq!("SHOP_NOT_FOUND", body["error"]);
}
