use aws_tests_common::get_cfn_output;
use common::api::collection::PutCollectionData;
use fake::{Fake, Faker};
use product::data::{product_state_data::ProductStateData, put_data::PutProductData};
use search_filter::data::user_search_filter_data::UserSearchFilterData;
use shop::core::shop::Shop;
use shop::dynamodb::{
    repository::{ShopDynamoDbRepository, ShopDynamoDbRepositoryImpl},
    shop_record::ShopRecord,
};
use std::time::Duration;
use test_api::*;
use time::OffsetDateTime;
use user::dynamodb::repository::{UserDynamoDbRepository, UserDynamoDbRepositoryImpl};
use user::dynamodb::user_record_update::UserRecordUpdate;

async fn prepare_test_shop() -> Shop {
    let stack = get_cfn_output();
    let shop = Faker.fake::<Shop>();

    let dynamodb_repository =
        ShopDynamoDbRepositoryImpl::new(get_dynamodb_client().await, &stack.dynamodb_table_1_name);
    let mut shop_records = ShopRecord::clone_from_shop_as_shop_domain_records(&shop);
    shop_records.push(ShopRecord::from_shop_as_shop_id_record(shop.clone()));
    let _ = dynamodb_repository
        .put_shop_records_transact(shop_records)
        .await
        .unwrap();

    shop
}

#[localstack_test(services = [Cloudformation()])]
async fn should_send_email_to_user_when_product_matches_search_filter() {
    let stack = get_cfn_output();
    let shop = prepare_test_shop().await;

    // create user
    let user = create_test_user(&get_test_mail()).await;
    tokio::time::sleep(Duration::from_secs(10)).await;
    let user_repository =
        UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, &stack.dynamodb_table_1_name);
    assert!(
        user_repository
            .get_user_record(&user.sub.into())
            .await
            .unwrap()
            .is_some()
    );
    user_repository
        .update_user_record(
            &user.sub.into(),
            UserRecordUpdate {
                first_name: Some("Thomas".into()),
                last_name: Some("Testperson".into()),
                language: Some(common::language::record::LanguageRecord::De),
                currency: Some(common::currency::record::CurrencyRecord::Eur),
                prohibited_content_consent: None,
                updated: OffsetDateTime::now_utc(),
            },
        )
        .await
        .unwrap();

    // create search filter that matches products with state=AVAILABLE
    let post_url = format!(
        "{}/api/v1/me/search-filters",
        stack.api_gateway_endpoint_url,
    );
    let post_response = reqwest::Client::new()
        .post(post_url)
        .bearer_auth(&user.access_token)
        .json(&serde_json::json!({
            "name": "My Available Products",
            "search": {
                "language": "de",
                "currency": "EUR",
                "state": ["AVAILABLE"]
            }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(201, post_response.status());
    let filter: UserSearchFilterData = post_response.json().await.unwrap();
    assert_eq!(filter.name.to_string(), "My Available Products");
    tokio::time::sleep(Duration::from_secs(30)).await;

    // create a product with state=AVAILABLE (should match the search filter)
    let mut put_product_data: PutProductData = Faker.fake();
    put_product_data.state = ProductStateData::Available;
    put_product_data
        .url
        .set_host(Some(shop.domains.into_iter().next().unwrap().as_str()))
        .unwrap();
    let url = format!("{}/api/v1/products", stack.api_gateway_endpoint_url);
    let response = reqwest::Client::new()
        .put(url)
        .json(&PutCollectionData {
            items: vec![put_product_data.clone()],
        })
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());

    // verify email notification arrived for the matched search filter
    assert!(wait_for_email("Neues Ergebnis für").await)
}
