use aws_sdk_cognitoidentityprovider::types::AuthFlowType;
use aws_sdk_cognitoidentityprovider::types::MessageActionType;
use aws_tests_common::get_cfn_output;
use fake::Fake;
use fake::Faker;
use staging_tests::get_cognito_client;
use staging_tests_macros::staging_test;

#[staging_test]
async fn should_authorize_user() {
    let cfn = get_cfn_output();
    let cognito = get_cognito_client().await;

    let username = "foo@bar.com";
    let password = format!("{}*aA1", Faker.fake::<String>());

    cognito
        .admin_create_user()
        .user_pool_id(&cfn.cognito_user_pool_id)
        .username(username)
        .message_action(MessageActionType::Suppress)
        .send()
        .await
        .unwrap();
    cognito
        .admin_set_user_password()
        .user_pool_id(&cfn.cognito_user_pool_id)
        .username(username)
        .password(&password)
        .permanent(true)
        .send()
        .await
        .unwrap();

    let resp = cognito
        .admin_initiate_auth()
        .user_pool_id(&cfn.cognito_user_pool_id)
        .client_id(&cfn.cognito_user_pool_client_admin_id)
        .auth_flow(AuthFlowType::AdminUserPasswordAuth)
        .auth_parameters("USERNAME", username)
        .auth_parameters("PASSWORD", &password)
        .send()
        .await
        .unwrap();

    let tokens = resp.authentication_result().unwrap();
    let access_token = tokens.access_token().unwrap();

    let url = format!(
        "{}/api/v1/search-filters",
        get_cfn_output().api_gateway_endpoint_url,
    );
    let response = reqwest::Client::new()
        .get(url)
        .bearer_auth(access_token)
        .send()
        .await
        .unwrap();
    tracing::info!(payload = ?response);
    assert_eq!(200, response.status());
    assert!(
        response.json::<serde_json::Value>().await.unwrap()["items"]
            .as_array()
            .unwrap()
            .is_empty()
    )
}
