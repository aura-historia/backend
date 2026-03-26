mod common;

use aws_tests_common::get_cfn_output;
use common::http_client;
use smoking_tests::smoking_test;

#[smoking_test]
async fn should_respond_404_for_get_period_when_not_exists() {
    let response = http_client()
        .get(format!(
            "{}/api/v1/periods/non-existent-period-id",
            get_cfn_output().api_gateway_endpoint_url,
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(404, response.status());

    let body = response.json::<serde_json::Value>().await.unwrap();
    assert_eq!(404, body["status"]);
    assert_eq!("NOT_FOUND", body["error"]);
}

#[smoking_test]
async fn should_respond_200_for_simple_search_periods() {
    let response = http_client()
        .get(format!(
            "{}/api/v1/periods",
            get_cfn_output().api_gateway_endpoint_url
        ))
        .query(&[("language", "de"), ("nameQuery", "baroque")])
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());
}

#[smoking_test]
async fn should_respond_200_for_complex_search_periods() {
    let response = http_client()
        .post(format!(
            "{}/api/v1/periods/search",
            get_cfn_output().api_gateway_endpoint_url
        ))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());
}
