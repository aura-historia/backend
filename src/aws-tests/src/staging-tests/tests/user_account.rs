use aws_tests_common::get_cfn_output;
use common::{
    currency::{data::CurrencyData, record::CurrencyRecord},
    language::{data::LanguageData, record::LanguageRecord},
    product_id::api::ProductKeyData,
    user_id::UserId,
};
use fake::{Fake, Faker};
use product::dynamodb::{
    product_record::ProductRecord,
    repository::{ProductDynamoDbRepository, ProductDynamoDbRepositoryImpl},
};
use product_watchlist::dynamodb::repository::{
    WatchlistProductDynamoDbRepository, WatchlistProductDynamoDbRepositoryImpl,
};
use staging_tests::{
    create_random_test_user, get_cognito_client, get_dynamodb_client, staging_test,
};
use std::time::Duration;
use user::{
    data::{get_user_data::GetUserAccountData, patch_user_data::PatchUserAccountData},
    dynamodb::repository::{UserDynamoDbRepository, UserDynamoDbRepositoryImpl},
};

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

#[staging_test]
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

#[staging_test]
async fn should_200_update_all_denormalized_watchlist_entries_for_patch_user() {
    let user = create_random_test_user().await;

    // persist materialized item-record we want to watch
    let product_repository = ProductDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &get_cfn_output().dynamodb_table_1_name,
    );
    let materialized = Faker.fake::<ProductRecord>();
    let put_res = product_repository
        .put_product_records([materialized.clone()].into())
        .await
        .unwrap();
    assert!(put_res.unprocessed_items.unwrap_or_default().is_empty());

    // Add product to watchlist
    let post_watchlist_url = format!(
        "{}/api/v1/me/watchlist",
        get_cfn_output().api_gateway_endpoint_url,
    );
    let post_response = reqwest::Client::new()
        .post(post_watchlist_url)
        .json(&ProductKeyData {
            shop_id: materialized.shop_id,
            shops_product_id: materialized.shops_product_id.clone(),
        })
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(201, post_response.status());

    // Patch user
    let account_url = format!(
        "{}/api/v1/me/account",
        get_cfn_output().api_gateway_endpoint_url,
    );
    let patch_user_account_data = PatchUserAccountData {
        first_name: Some("Hansi".into()),
        last_name: Some("Hans".into()),
        language: Some(LanguageData::Fr),
        currency: Some(CurrencyData::Nzd),
    };
    let patch_response = reqwest::Client::new()
        .patch(account_url.clone())
        .bearer_auth(user.access_token.clone())
        .json(&patch_user_account_data)
        .send()
        .await
        .unwrap();
    assert_eq!(200, patch_response.status());

    // Wait for asynchronous processing of denormalized user-information to finish
    tokio::time::sleep(Duration::from_secs(30)).await;

    // Get updated watchlist-record
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &get_cfn_output().dynamodb_table_1_name,
    );
    let actual_watchlist_record = watchlist_repository
        .get_watchlist_record(
            &user.sub.into(),
            &materialized.shop_id,
            &materialized.shops_product_id,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        "Hansi",
        actual_watchlist_record
            .user_record
            .first_name
            .as_ref()
            .unwrap()
            .as_ref()
    );
    assert_eq!(
        "Hans",
        actual_watchlist_record
            .user_record
            .last_name
            .as_ref()
            .unwrap()
            .as_ref()
    );
    assert_eq!(
        &LanguageRecord::Fr,
        actual_watchlist_record
            .user_record
            .language
            .as_ref()
            .unwrap()
    );
    assert_eq!(
        &CurrencyRecord::Nzd,
        actual_watchlist_record
            .user_record
            .currency
            .as_ref()
            .unwrap()
    );
}
