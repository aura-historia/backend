use aws_tests_common::get_cfn_output;
use staging_tests::create_random_test_user;
use staging_tests_macros::staging_test;

#[staging_test]
async fn should_return_actual_search_filters() {
    let user = create_random_test_user().await;
    let url = format!(
        "{}/api/v1/search-filters",
        get_cfn_output().api_gateway_endpoint_url,
    );
    let response = reqwest::Client::new()
        .get(url)
        .bearer_auth(user.access_token)
        .send()
        .await
        .unwrap();

    assert_eq!(200, response.status());
    assert!(
        response.json::<serde_json::Value>().await.unwrap()["items"]
            .as_array()
            .unwrap()
            .is_empty()
    )
}
