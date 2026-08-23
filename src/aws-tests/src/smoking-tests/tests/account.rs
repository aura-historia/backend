mod http;

use aws_tests_common::get_cfn_output;
use http::http_client;
use smoking_tests::smoking_test;

#[smoking_test]
async fn should_respond_401_for_get_account_when_unauthorized() {
    let response = http_client()
        .get(format!(
            "{}/api/v1/me/account",
            get_cfn_output().api_gateway_endpoint_url,
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(401, response.status());

    let body = response.json::<serde_json::Value>().await.unwrap();
    assert_eq!(401, body["status"]);
    assert_eq!("UNAUTHORIZED", body["error"]);
}

#[smoking_test]
async fn should_respond_401_for_patch_account_when_unauthorized() {
    let response = http_client()
        .patch(format!(
            "{}/api/v1/me/account",
            get_cfn_output().api_gateway_endpoint_url,
        ))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(401, response.status());

    let body = response.json::<serde_json::Value>().await.unwrap();
    assert_eq!(401, body["status"]);
    assert_eq!("UNAUTHORIZED", body["error"]);
}
