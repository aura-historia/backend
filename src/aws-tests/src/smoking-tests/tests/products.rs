mod http;

use aws_tests_common::get_cfn_output;
use http::http_client;

use smoking_tests::smoking_test;
use uuid::Uuid;

#[smoking_test]
async fn should_respond_404_for_get_product_when_not_exists() {
    let response = http_client()
        .get(format!(
            "{}/api/v1/shops/{}/products/{}",
            get_cfn_output().api_gateway_endpoint_url,
            Uuid::new_v4(),
            Uuid::new_v4()
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(404, response.status());

    let body = response.json::<serde_json::Value>().await.unwrap();
    assert_eq!(404, body["status"]);
    assert_eq!("PRODUCT_NOT_FOUND", body["error"]);
}

#[smoking_test]
async fn should_respond_404_for_get_product_by_slug_when_not_exists() {
    let response = http_client()
        .get(format!(
            "{}/api/v1/by-slug/shops/non-existent-shop/products/non-existent-product-aabbcc",
            get_cfn_output().api_gateway_endpoint_url,
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(404, response.status());

    let body = response.json::<serde_json::Value>().await.unwrap();
    assert_eq!(404, body["status"]);
    assert_eq!("PRODUCT_NOT_FOUND", body["error"]);
}

#[smoking_test]
async fn should_respond_404_for_get_product_history_when_not_exists() {
    let response = http_client()
        .get(format!(
            "{}/api/v1/shops/{}/products/{}/history",
            get_cfn_output().api_gateway_endpoint_url,
            Uuid::new_v4(),
            Uuid::new_v4()
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(404, response.status());

    let body = response.json::<serde_json::Value>().await.unwrap();
    assert_eq!(404, body["status"]);
    assert_eq!("PRODUCT_NOT_FOUND", body["error"]);
}

#[smoking_test]
async fn should_respond_404_for_get_similar_products_when_not_exists() {
    let response = http_client()
        .get(format!(
            "{}/api/v1/shops/{}/products/{}/similar",
            get_cfn_output().api_gateway_endpoint_url,
            Uuid::new_v4(),
            Uuid::new_v4()
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(404, response.status());

    let body = response.json::<serde_json::Value>().await.unwrap();
    assert_eq!(404, body["status"]);
    assert_eq!("PRODUCT_NOT_FOUND", body["error"]);
}

#[smoking_test]
async fn should_respond_200_for_complex_search_products() {
    let response = http_client()
        .post(format!(
            "{}/api/v1/products/search",
            get_cfn_output().api_gateway_endpoint_url
        ))
        .query(&[("sort", "price"), ("order", "asc"), ("size", "5")])
        .json(&serde_json::json!({}))
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
async fn should_respond_200_for_simple_search_products() {
    let response = http_client()
        .get(format!(
            "{}/api/v1/products",
            get_cfn_output().api_gateway_endpoint_url
        ))
        .query(&[("language", "en"), ("currency", "EUR"), ("size", "5")])
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());

    let body = response.json::<serde_json::Value>().await.unwrap();
    assert!(body["items"].is_array());
    assert!(body["size"].is_u64());
    assert!(body["total"].is_u64());
}
