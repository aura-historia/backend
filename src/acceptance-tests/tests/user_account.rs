use aws_tests_common::get_cfn_output;
use common::{currency::data::CurrencyData, language::data::LanguageData, user_id::UserId};
use fake::Fake;
use std::time::Duration;
use test_api::*;
use user::{
    data::{get_user_data::GetUserAccountData, patch_user_data::PatchUserAccountData},
    dynamodb::repository::{UserDynamoDbRepository, UserDynamoDbRepositoryImpl},
};

#[localstack_test(services = [Cloudformation()])]
async fn should_create_dynamodb_user_record_on_signup() {
    let cfn = get_cfn_output();
    let cognito = get_cognito_client().await;

    let email: String = fake::faker::internet::de_de::SafeEmail().fake();
    let password: String = format!(
        "{}*1bC",
        fake::faker::internet::de_de::Password(8..12).fake::<String>()
    );

    let user_id: UserId = cognito
        .sign_up()
        .client_id(&cfn.cognito_user_pool_client_public_id)
        .username(&email)
        .password(password)
        .user_attributes(
            aws_sdk_cognitoidentityprovider::types::AttributeType::builder()
                .name("email")
                .value(&email)
                .build()
                .unwrap(),
        )
        .send()
        .await
        .unwrap()
        .user_sub
        .try_into()
        .unwrap();
    let _ = cognito
        .admin_confirm_sign_up()
        .user_pool_id(&cfn.cognito_user_pool_id)
        .username(&email)
        .send()
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_secs(5)).await;

    let user_repository =
        UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, &cfn.dynamodb_table_1_name);
    let res = user_repository.get_user_record(&user_id).await.unwrap();

    assert!(res.is_some_and(|user_record| user_record.email == email));
}

#[localstack_test(services = [Cloudformation()])]
async fn should_200_for_get_patch_get() {
    let user = create_random_test_user().await;
    let url = format!(
        "{}/api/v1/me/account",
        get_cfn_output().api_gateway_endpoint_url,
    );

    let get_response1 = reqwest::Client::new()
        .get(url.clone())
        .bearer_auth(user.access_token.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(200, get_response1.status());
    let gotten1 = get_response1.json::<GetUserAccountData>().await.unwrap();
    assert_eq!(UserId::from(user.sub), gotten1.user_id);

    let patch_user_account_data = PatchUserAccountData {
        first_name: Some("Hansi".into()),
        last_name: Some("Hans".into()),
        language: Some(LanguageData::Fr),
        currency: Some(CurrencyData::Nzd),
        prohibited_content_consent: None,
    };
    let patch_response = reqwest::Client::new()
        .patch(url.clone())
        .bearer_auth(user.access_token.clone())
        .json(&patch_user_account_data)
        .send()
        .await
        .unwrap();
    assert_eq!(200, patch_response.status());
    let patched = patch_response.json::<GetUserAccountData>().await.unwrap();
    assert_eq!(UserId::from(user.sub), patched.user_id);
    assert_eq!(
        &patch_user_account_data.first_name.unwrap(),
        patched.first_name.as_ref().unwrap()
    );
    assert_eq!(
        &patch_user_account_data.last_name.unwrap(),
        patched.last_name.as_ref().unwrap()
    );
    assert_eq!(
        &patch_user_account_data.language.unwrap(),
        patched.language.as_ref().unwrap()
    );
    assert_eq!(
        &patch_user_account_data.currency.unwrap(),
        patched.currency.as_ref().unwrap()
    );

    let get_response2 = reqwest::Client::new()
        .get(url.clone())
        .bearer_auth(user.access_token.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(200, get_response2.status());
    let gotten2 = get_response2.json::<GetUserAccountData>().await.unwrap();
    assert_eq!(patched, gotten2);
}
