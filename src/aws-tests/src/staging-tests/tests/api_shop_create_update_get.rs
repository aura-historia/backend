use aws_tests_common::get_cfn_output;
use common::shop_id::ShopId;
use shop::data::{
    get_shop_data::GetShopData, patch_shop_data::PatchShopData, post_shop_data::PostShopData,
};
use staging_tests::staging_test;
use std::time::Duration;
use url::Url;

#[staging_test]
async fn should_create_update_get_shop() {
    let post_shop_data = PostShopData {
        name: "Woobl woop".into(),
        urls: [Url::parse("https://hans-shopping-nig.com").unwrap()].into(),
        image: None,
    };
    let post_url = format!("{}/api/v1/shops", get_cfn_output().api_gateway_endpoint_url);
    let response = reqwest::Client::new()
        .post(post_url)
        .json(&post_shop_data)
        .send()
        .await
        .unwrap();
    assert_eq!(201, response.status());
    let created = response.json::<GetShopData>().await.unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;

    let patch_shop_data = PatchShopData {
        name: Some("hans goes shopping nig".into()),
        urls: None,
        image: Some(Url::parse("https://hans-shopping-nig.co.uk").unwrap()),
    };
    let post_url = format!(
        "{}/api/v1/shops/{}",
        get_cfn_output().api_gateway_endpoint_url,
        created.shop_id
    );
    let response = reqwest::Client::new()
        .patch(post_url)
        .json(&patch_shop_data)
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());
    let updated = response.json::<GetShopData>().await.unwrap();
    assert_eq!(patch_shop_data.name.unwrap(), updated.name);
    assert_eq!(post_shop_data.urls, updated.urls);
    assert_eq!(
        patch_shop_data.image.unwrap(),
        updated.image.clone().unwrap()
    );
    tokio::time::sleep(Duration::from_secs(1)).await;

    let get_url = format!(
        "{}/api/v1/shops/{}",
        get_cfn_output().api_gateway_endpoint_url,
        created.shop_id
    );
    let response = reqwest::Client::new().get(get_url).send().await.unwrap();
    assert_eq!(200, response.status());
    let gotten = response.json::<GetShopData>().await.unwrap();
    assert_eq!(updated, gotten);
}

#[staging_test]
async fn should_respond_404_when_shop_does_not_exist() {
    let response = reqwest::get(format!(
        "{}/api/v1/shops/{}",
        get_cfn_output().api_gateway_endpoint_url,
        ShopId::new()
    ))
    .await
    .unwrap();
    assert_eq!(404, response.status());

    let body = response.json::<serde_json::Value>().await.unwrap();
    assert_eq!(404, body["status"]);
    assert_eq!("SHOP_NOT_FOUND", body["error"]);
}
