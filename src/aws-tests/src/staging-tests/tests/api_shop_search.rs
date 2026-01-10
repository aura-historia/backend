use aws_tests_common::get_cfn_output;
use common::query::range_query::RangeQuery;
use fake::{Fake, Faker};
use opensearch::{IndexParts, params::Refresh};
use shop::data::shop_search_data::ShopSearchData;
use shop::opensearch::{
    repository::{ShopOpenSearchRepository, ShopOpenSearchRepositoryImpl},
    shop_document::ShopDocument,
};
use staging_tests::{get_opensearch_client, staging_test};
use std::{time::Duration, vec};
use time::macros::datetime;

#[staging_test]
async fn should_respond_200_when_hits() {
    let os_client = get_opensearch_client().await;
    let repository = ShopOpenSearchRepositoryImpl::new(os_client);
    let expected = Faker.fake::<ShopDocument>();
    let mut all = fake::vec![ShopDocument; 10];
    all.push(expected.clone());

    for shop in all {
        let _ = repository.index_shop_document(shop).await.unwrap();
    }
    os_client
        .index(IndexParts::Index("shops"))
        .refresh(Refresh::True)
        .send()
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_secs(3)).await;

    let search = ShopSearchData {
        shop_name_query: Some(expected.name.to_string().try_into().unwrap()),
        created: Some(RangeQuery {
            min: None,
            max: Some(datetime!(2999 - 01 - 02 0:00 UTC)),
        }),
        updated: None,
    };

    let url = format!(
        "{}/api/v1/shops/search?size=5",
        get_cfn_output().api_gateway_endpoint_url
    );
    let response = reqwest::Client::new()
        .post(url)
        .json(&search)
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());

    let body = response.json::<serde_json::Value>().await.unwrap();

    let item = body["items"].as_array().unwrap()[0].clone();
    assert_eq!(expected.shop_id.to_string(), item["shopId"]);
    assert_eq!(expected.name.to_string(), item["name"]);
}

#[staging_test]
async fn should_respond_200_when_no_hits() {
    let url = format!(
        "{}/api/v1/shops/search",
        get_cfn_output().api_gateway_endpoint_url
    );
    let response = reqwest::Client::new()
        .post(url)
        .json(&ShopSearchData {
            shop_name_query: Some(
                "woooah an incredible query no one can fulfil!"
                    .try_into()
                    .unwrap(),
            ),
            created: None,
            updated: None,
        })
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());
}
