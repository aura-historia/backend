mod http;

use aws_tests_common::get_cfn_output;
use http::http_client;
use smoking_tests::smoking_test;
use uuid::Uuid;

#[smoking_test]
async fn should_respond_404_for_get_shop_when_not_exists() {
    let response = http_client()
        .get(format!(
            "{}/api/v1/shops/{}",
            get_cfn_output().api_gateway_endpoint_url,
            Uuid::new_v4(),
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(404, response.status());

    let body = response.json::<serde_json::Value>().await.unwrap();
    assert_eq!(404, body["status"]);
    assert_eq!("SHOP_NOT_FOUND", body["error"]);
}

#[smoking_test]
async fn should_respond_404_for_get_shop_by_domain_when_not_exists() {
    let response = http_client()
        .get(format!(
            "{}/api/v1/by-domain/shops/non-existent-shop.com",
            get_cfn_output().api_gateway_endpoint_url,
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(404, response.status());

    let body = response.json::<serde_json::Value>().await.unwrap();
    assert_eq!(404, body["status"]);
    assert_eq!("SHOP_NOT_FOUND", body["error"]);
}

#[smoking_test]
async fn should_respond_404_for_get_shop_by_slug_when_not_exists() {
    let response = http_client()
        .get(format!(
            "{}/api/v1/by-slug/shops/non-existent-shop",
            get_cfn_output().api_gateway_endpoint_url,
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(404, response.status());

    let body = response.json::<serde_json::Value>().await.unwrap();
    assert_eq!(404, body["status"]);
    assert_eq!("SHOP_NOT_FOUND", body["error"]);
}

#[smoking_test]
async fn should_respond_200_for_simple_search_shops() {
    let response = http_client()
        .get(format!(
            "{}/api/v1/shops",
            get_cfn_output().api_gateway_endpoint_url
        ))
        .query(&[("shopNameQuery", "test"), ("size", "5")])
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());
}

#[smoking_test]
async fn should_respond_200_for_complex_search_shops() {
    let response = http_client()
        .post(format!(
            "{}/api/v1/shops/search",
            get_cfn_output().api_gateway_endpoint_url
        ))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());
}
