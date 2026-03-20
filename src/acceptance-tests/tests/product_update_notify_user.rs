use aws_tests_common::get_cfn_output;
use common::{
    api::collection::PutCollectionData, product_id::api::ProductKeyData, user_id::UserId,
};
use fake::{Fake, Faker};
use product::data::{product_state_data::ProductStateData, put_data::PutProductData};
use product::dynamodb::repository::{ProductDynamoDbRepository, ProductDynamoDbRepositoryImpl};
use product_watchlist::dynamodb::repository::WatchlistProductDynamoDbRepository;
use product_watchlist::{
    data::watchlist_product_data::WatchlistProductData,
    dynamodb::repository::WatchlistProductDynamoDbRepositoryImpl,
};
use product_watchlist_api::watchlist_patch::WatchlistProductPatch;
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
async fn should_send_email_to_user_when_watched_product_has_update() {
    let stack = get_cfn_output();
    let shop = prepare_test_shop().await;

    // create product
    let mut put_product_data: PutProductData = Faker.fake();
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
    tokio::time::sleep(Duration::from_secs(45)).await;
    let product_repository = ProductDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &stack.dynamodb_table_1_name,
    );
    assert!(
        product_repository
            .get_product_record(&shop.shop_id, &put_product_data.shops_product_id)
            .await
            .unwrap()
            .is_some()
    );

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

    // add product to watchlist
    let post_url = format!(
        "{}/api/v1/me/watchlist",
        get_cfn_output().api_gateway_endpoint_url,
    );
    let post_response = reqwest::Client::new()
        .post(post_url)
        .json(&ProductKeyData {
            shop_id: shop.shop_id,
            shops_product_id: put_product_data.shops_product_id.clone(),
        })
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(201, post_response.status());
    tokio::time::sleep(Duration::from_secs(10)).await;

    // enable notifications
    let patch_url = format!(
        "{}/api/v1/me/watchlist/{}/{}",
        get_cfn_output().api_gateway_endpoint_url,
        shop.shop_id,
        put_product_data.shops_product_id,
    );
    let patch_response = reqwest::Client::new()
        .patch(patch_url)
        .bearer_auth(&user.access_token)
        .json(&WatchlistProductPatch {
            notifications: Some(true),
        })
        .send()
        .await
        .unwrap();
    assert_eq!(200, patch_response.status());
    let patched = patch_response.json::<WatchlistProductData>().await.unwrap();
    tokio::time::sleep(Duration::from_secs(10)).await;
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &stack.dynamodb_table_1_name,
    );
    let eligible = watchlist_repository
        .query_user_ids_watching_product(&patched.product_id)
        .await
        .unwrap();
    let eligible_user_ids: Vec<UserId> = eligible.into_iter().map(|(user_id, _)| user_id).collect();
    assert_eq!(vec![UserId::from(user.sub)], eligible_user_ids);
    tokio::time::sleep(Duration::from_secs(10)).await;

    // update product
    put_product_data.state = if matches!(put_product_data.state, ProductStateData::Available) {
        ProductStateData::Sold
    } else {
        ProductStateData::Available
    };
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

    // verify email with update-notification arrived
    assert!(wait_for_email("Antiquitäten-Update").await)
}
