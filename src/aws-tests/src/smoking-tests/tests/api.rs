use aws_tests_common::get_cfn_output;
use fake::{Fake, Faker};
use item::data::item_search_data::ItemSearchData;
use smoking_tests::smoking_test;
use uuid::Uuid;

#[smoking_test]
async fn should_respond_404_for_get_item_when_not_exists() {
    let response = reqwest::get(format!(
        "{}/api/v1/items/{}/{}",
        get_cfn_output().api_gateway_endpoint_url,
        Uuid::new_v4(),
        Uuid::new_v4()
    ))
    .await
    .unwrap();
    assert_eq!(404, response.status());

    let body = response.json::<serde_json::Value>().await.unwrap();
    assert_eq!(404, body["status"]);
    assert_eq!("ITEM_NOT_FOUND", body["error"]);
}

#[smoking_test]
async fn should_respond_200_for_search_items() {
    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/items?sort=price&order=asc&from=0&size=5",
            get_cfn_output().api_gateway_endpoint_url
        ))
        .json(&Faker.fake::<ItemSearchData>())
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());

    let body = response.json::<serde_json::Value>().await.unwrap();
    assert!(body["items"].is_array());
    assert!(body["size"].is_u64());
    assert!(body["total"].is_u64());
}

#[smoking_test]
async fn should_respond_404_for_get_shop_when_not_exists() {
    let response = reqwest::get(format!(
        "{}/api/v1/shops/{}",
        get_cfn_output().api_gateway_endpoint_url,
        Uuid::new_v4(),
    ))
    .await
    .unwrap();
    assert_eq!(404, response.status());

    let body = response.json::<serde_json::Value>().await.unwrap();
    assert_eq!(404, body["status"]);
    assert_eq!("SHOP_NOT_FOUND", body["error"]);
}
