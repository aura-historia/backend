use aws_tests_common::get_cfn_output;
use common::{
    pagination::cursor::api::TimeCursoredData, product_id::api::ProductKeyData, shop_id::ShopId,
    shops_product_id::ShopsProductId,
};
use fake::{Fake, Faker};
use product::dynamodb::{
    product_record::ProductRecord,
    repository::{ProductDynamoDbRepository, ProductDynamoDbRepositoryImpl},
};
use product::watchlist::data::watchlist_product_data::WatchlistProductData;
use product_api::watchlist_get::WatchlistProductDataView;
use product_api::watchlist_patch::WatchlistProductPatch;
use staging_tests::{create_random_test_user, get_dynamodb_client, staging_test};

#[staging_test]
async fn should_401_when_unauthorized_for_post() {
    let url = format!(
        "{}/api/v1/me/watchlist",
        get_cfn_output().api_gateway_endpoint_url,
    );
    let response = reqwest::Client::new().post(url).send().await.unwrap();
    assert_eq!(401, response.status());
}

#[staging_test]
async fn should_401_when_unauthorized_for_delete() {
    let url = format!(
        "{}/api/v1/me/watchlist/{}/{}",
        get_cfn_output().api_gateway_endpoint_url,
        ShopId::new(),
        ShopsProductId::new()
    );
    let response = reqwest::Client::new().delete(url).send().await.unwrap();
    assert_eq!(401, response.status());
}

#[staging_test]
async fn should_401_when_unauthorized_for_get() {
    let url = format!(
        "{}/api/v1/me/watchlist",
        get_cfn_output().api_gateway_endpoint_url,
    );
    let response = reqwest::Client::new().get(url).send().await.unwrap();
    assert_eq!(401, response.status());
}

#[staging_test]
async fn should_put_and_get_and_patch_and_delete_watchlist_product_and_verify_not_exists() {
    let user = create_random_test_user().await;

    // persist materialized item-record we want to watch
    let repository = ProductDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &get_cfn_output().dynamodb_table_1_name,
    );
    let materialized = Faker.fake::<ProductRecord>();
    let put_res = repository
        .put_product_records([materialized.clone()].into())
        .await
        .unwrap();
    assert!(put_res.unprocessed_items.unwrap_or_default().is_empty());

    // Add product to watchlist
    let post_url = format!(
        "{}/api/v1/me/watchlist",
        get_cfn_output().api_gateway_endpoint_url,
    );
    let post_response = reqwest::Client::new()
        .post(post_url)
        .json(&ProductKeyData {
            shop_id: materialized.shop_id,
            shops_product_id: materialized.shops_product_id.clone(),
        })
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(201, post_response.status());

    // Get posted
    let get_url = format!(
        "{}/api/v1/me/watchlist?currency=EUR&sort=created&order=desc",
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
        .json::<TimeCursoredData<WatchlistProductDataView>>()
        .await
        .unwrap();

    assert_eq!(1, gotten.items.len());
    assert_eq!(
        &materialized.product_id,
        &gotten.items[0].product.product_id
    );
    assert_eq!(&materialized.shop_id, &gotten.items[0].product.shop_id);
    assert_eq!(
        &materialized.shops_product_id,
        &gotten.items[0].product.shops_product_id
    );
    assert_eq!(1, gotten.total.unwrap());

    // Patch gotten
    let patch_url = format!(
        "{}/api/v1/me/watchlist/{}/{}",
        get_cfn_output().api_gateway_endpoint_url,
        materialized.shop_id,
        materialized.shops_product_id,
    );
    let patch_response = reqwest::Client::new()
        .patch(patch_url)
        .bearer_auth(&user.access_token)
        .json(&WatchlistProductPatch {
            notifications: Some(!gotten.items[0].notifications),
        })
        .send()
        .await
        .unwrap();
    assert_eq!(200, patch_response.status());
    let patch_res_payload = patch_response.json::<WatchlistProductData>().await.unwrap();
    assert_eq!(gotten.items[0].created, patch_res_payload.created);
    assert_eq!(materialized.shop_id, patch_res_payload.shop_id);
    assert_eq!(
        materialized.shops_product_id,
        patch_res_payload.shops_product_id
    );
    assert_eq!(materialized.product_id, patch_res_payload.product_id);

    // Delete gotten
    let delete_url = format!(
        "{}/api/v1/me/watchlist/{}/{}",
        get_cfn_output().api_gateway_endpoint_url,
        materialized.shop_id,
        materialized.shops_product_id,
    );
    let delete_response = reqwest::Client::new()
        .delete(delete_url)
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(204, delete_response.status());

    // Get all (none)
    let get_url = format!(
        "{}/api/v1/me/watchlist?currency=EUR&sort=created&order=desc",
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
        .json::<TimeCursoredData<WatchlistProductDataView>>()
        .await
        .unwrap();

    assert!(gotten.items.is_empty());
    assert_eq!(0, gotten.total.unwrap_or(0));
}
