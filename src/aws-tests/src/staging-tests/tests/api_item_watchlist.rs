use aws_tests_common::get_cfn_output;
use common::{
    item_id::api::ItemKeyData, pagination::cursor::api::TimeCursoredData, shop_id::ShopId,
    shops_item_id::ShopsItemId,
};
use fake::{Fake, Faker};
use item_api_watchlist_get::WatchlistItemDataView;
use item_dynamodb::{
    item_record::ItemRecord,
    repository::{ItemDynamoDbRepository, ItemDynamoDbRepositoryImpl},
};
use staging_tests::{create_random_test_user, get_dynamodb_client, staging_test};
use time::format_description::well_known::Rfc3339;

#[staging_test]
async fn should_401_when_unauthorized_for_post() {
    let url = format!(
        "{}/api/v1/watchlist",
        get_cfn_output().api_gateway_endpoint_url,
    );
    let response = reqwest::Client::new().post(url).send().await.unwrap();
    assert_eq!(401, response.status());
}

#[staging_test]
async fn should_401_when_unauthorized_for_delete() {
    let url = format!(
        "{}/api/v1/watchlist/{}/{}",
        get_cfn_output().api_gateway_endpoint_url,
        ShopId::new(),
        ShopsItemId::new()
    );
    let response = reqwest::Client::new().delete(url).send().await.unwrap();
    assert_eq!(401, response.status());
}

#[staging_test]
async fn should_401_when_unauthorized_for_get() {
    let url = format!(
        "{}/api/v1/watchlist",
        get_cfn_output().api_gateway_endpoint_url,
    );
    let response = reqwest::Client::new().get(url).send().await.unwrap();
    assert_eq!(401, response.status());
}

#[staging_test]
async fn should_put_and_get_and_delete_watchlist_item_and_verify_not_exists() {
    let user = create_random_test_user().await;

    // persist materialized item-record we want to watch
    let repository = ItemDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &get_cfn_output().dynamodb_table_1_name,
    );
    let materialized = Faker.fake::<ItemRecord>();
    let put_res = repository
        .put_item_records([materialized.clone()].into())
        .await
        .unwrap();
    assert!(put_res.unprocessed_items.unwrap_or_default().is_empty());

    // Add item to watchlist
    let post_url = format!(
        "{}/api/v1/watchlist",
        get_cfn_output().api_gateway_endpoint_url,
    );
    let post_response = reqwest::Client::new()
        .post(post_url)
        .json(&ItemKeyData {
            shop_id: materialized.shop_id,
            shops_item_id: materialized.shops_item_id.clone(),
        })
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(201, post_response.status());

    // Get posted
    let get_url = format!(
        "{}/api/v1/watchlist?currency=EUR&sort=created&order=desc",
        get_cfn_output().api_gateway_endpoint_url,
    );
    let get_response = reqwest::Client::new()
        .get(get_url)
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(200, get_response.status());
    let gotten = get_response
        .json::<TimeCursoredData<WatchlistItemDataView>>()
        .await
        .unwrap();

    assert_eq!(1, gotten.items.len());
    assert_eq!(&materialized.item_id, &gotten.items[0].item.item_id);
    assert_eq!(&materialized.shop_id, &gotten.items[0].item.shop_id);
    assert_eq!(
        &materialized.shops_item_id,
        &gotten.items[0].item.shops_item_id
    );

    // Delete gotten
    let delete_url = format!(
        "{}/api/v1/watchlist/{}/{}?created={}",
        get_cfn_output().api_gateway_endpoint_url,
        materialized.shop_id,
        materialized.shops_item_id,
        gotten.items[0].created.format(&Rfc3339).unwrap()
    );
    let get_response = reqwest::Client::new()
        .delete(delete_url)
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(204, get_response.status());

    // Get all (none)
    let get_url = format!(
        "{}/api/v1/watchlist?currency=EUR&sort=created&order=desc",
        get_cfn_output().api_gateway_endpoint_url,
    );
    let get_response = reqwest::Client::new()
        .get(get_url)
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(200, get_response.status());
    let gotten = get_response
        .json::<TimeCursoredData<WatchlistItemDataView>>()
        .await
        .unwrap();

    assert!(gotten.items.is_empty());
}
