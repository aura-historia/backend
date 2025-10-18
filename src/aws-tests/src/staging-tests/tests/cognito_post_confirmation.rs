use std::time::Duration;

use aws_tests_common::get_cfn_output;
use common::user_id::UserId;
use fake::Fake;
use staging_tests::{get_cognito_client, get_dynamodb_client, staging_test};
use user_dynamodb::repository::{UserDynamoDbRepository, UserDynamoDbRepositoryImpl};

#[staging_test]
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
