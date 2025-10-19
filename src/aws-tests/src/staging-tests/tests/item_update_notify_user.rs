use std::time::Duration;

use aws_tests_common::get_cfn_output;
use common::{api::collection::PutCollectionData, item_id::api::ItemKeyData, user_id::UserId};
use fake::{Fake, Faker};
use item_api_watchlist_patch::WatchlistItemPatch;
use item_data::{item_state_data::ItemStateData, put_data::PutItemData};
use item_dynamodb::repository::{ItemDynamoDbRepository, ItemDynamoDbRepositoryImpl};
use item_watchlist::{
    data::WatchlistItemData,
    repository::{WatchlistItemDynamoDbRepository, WatchlistItemDynamoDbRepositoryImpl},
};
use shop_core::shop::Shop;
use shop_dynamodb::{
    repository::{ShopDynamoDbRepository, ShopDynamoDbRepositoryImpl},
    shop_record::ShopRecord,
};
use staging_tests::{create_test_user, get_dynamodb_client, staging_test};
use time::macros::date;
use user_dynamodb::repository::{UserDynamoDbRepository, UserDynamoDbRepositoryImpl};

async fn prepare_test_shop() -> Shop {
    let stack = get_cfn_output();
    let shop = Faker.fake::<Shop>();

    let dynamodb_repository =
        ShopDynamoDbRepositoryImpl::new(get_dynamodb_client().await, &stack.dynamodb_table_1_name);
    let mut shop_records = ShopRecord::try_clone_from_shop_as_shop_url_records(&shop).unwrap();
    shop_records.push(ShopRecord::from_shop_as_shop_id_record(shop.clone()));
    let _ = dynamodb_repository
        .put_shop_records_transact(shop_records)
        .await
        .unwrap();

    shop
}

#[staging_test]
async fn should_send_email_to_user_when_watched_item_has_update() {
    let stack = get_cfn_output();
    let shop = prepare_test_shop().await;

    // create item
    let mut put_item_data: PutItemData = Faker.fake();
    put_item_data
        .url
        .set_host(shop.urls.first().unwrap().host_str())
        .unwrap();
    let url = format!("{}/api/v1/items", stack.api_gateway_endpoint_url);
    let response = reqwest::Client::new()
        .put(url)
        .json(&PutCollectionData {
            items: vec![put_item_data.clone()],
        })
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());
    tokio::time::sleep(Duration::from_secs(30)).await;
    let item_repository =
        ItemDynamoDbRepositoryImpl::new(get_dynamodb_client().await, &stack.dynamodb_table_1_name);
    assert!(
        item_repository
            .get_item_record(&shop.shop_id, &put_item_data.shops_item_id)
            .await
            .unwrap()
            .is_some()
    );

    // create user
    let user = create_test_user(
        "no-reply@aura-historia.com",
        "No",
        "Reply",
        &date!(2002 - 04 - 05),
        "male",
        &None,
        &None,
        &None,
    )
    .await;
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

    // add item to watchlist
    let post_url = format!(
        "{}/api/v1/watchlist",
        get_cfn_output().api_gateway_endpoint_url,
    );
    let post_response = reqwest::Client::new()
        .post(post_url)
        .json(&ItemKeyData {
            shop_id: shop.shop_id,
            shops_item_id: put_item_data.shops_item_id.clone(),
        })
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(201, post_response.status());
    tokio::time::sleep(Duration::from_secs(3)).await;
    let watchlist_repository = WatchlistItemDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &stack.dynamodb_table_1_name,
    );

    // enable notifications
    let patch_url = format!(
        "{}/api/v1/watchlist/{}/{}",
        get_cfn_output().api_gateway_endpoint_url,
        shop.shop_id,
        put_item_data.shops_item_id,
    );
    let patch_response = reqwest::Client::new()
        .patch(patch_url)
        .bearer_auth(&user.access_token)
        .json(&WatchlistItemPatch {
            notifications: Some(true),
        })
        .send()
        .await
        .unwrap();
    assert_eq!(200, patch_response.status());
    let patched = patch_response.json::<WatchlistItemData>().await.unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;
    let eligible = watchlist_repository
        .query_user_records_with_notifications(&patched.item_id)
        .await
        .unwrap()
        .into_iter()
        .map(|user| user.id)
        .collect::<Vec<_>>();
    assert_eq!(vec![UserId::from(user.sub)], eligible);

    // update item
    put_item_data.state = if matches!(put_item_data.state, ItemStateData::Available) {
        ItemStateData::Sold
    } else {
        ItemStateData::Available
    };
    let url = format!("{}/api/v1/items", stack.api_gateway_endpoint_url);
    let response = reqwest::Client::new()
        .put(url)
        .json(&PutCollectionData {
            items: vec![put_item_data.clone()],
        })
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());
}
