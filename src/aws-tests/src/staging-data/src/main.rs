use std::time::Duration;

use aws_tests_common::get_cfn_output;
use common::api::collection::PutCollectionData;
use fake::{Fake, Faker};
use item_data::put_data::PutItemData;
use shop_core::shop::Shop;
use shop_dynamodb::repository::{ShopDynamoDbRepository, ShopDynamoDbRepositoryImpl};
use shop_opensearch::repository::{ShopOpenSearchRepository, ShopOpenSearchRepositoryImpl};
use staging_tests::{get_dynamodb_client, get_opensearch_client};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    staging_tests::reset().await;
    populate_items().await;
    populate_shops().await;

    Ok(())
}

async fn populate_items() {
    let stack = get_cfn_output();
    let put_items_url = format!("{}/api/v1/items", stack.api_gateway_endpoint_url);

    // create items
    let put_item_commands = PutCollectionData {
        items: fake::vec![PutItemData; 142],
    };
    let response = reqwest::Client::new()
        .put(&put_items_url)
        .json(&put_item_commands)
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());
    tokio::time::sleep(Duration::from_secs(30)).await;

    // put updates
    for _ in 0..7 {
        let mut updates = put_item_commands.items.to_vec();
        for put_item in &mut updates {
            put_item.state = Faker.fake();
            put_item.price = Faker.fake();
        }
        let put_commands = PutCollectionData { items: updates };
        let response = reqwest::Client::new()
            .put(&put_items_url)
            .json(&put_commands)
            .send()
            .await
            .unwrap();
        assert_eq!(200, response.status());
        tokio::time::sleep(Duration::from_secs(30)).await;
    }
}

async fn populate_shops() {
    let stack = get_cfn_output();
    let shops = fake::vec![Shop; 42];

    let dynamodb_repository =
        ShopDynamoDbRepositoryImpl::new(get_dynamodb_client().await, &stack.dynamodb_table_1_name);
    for shop in shops.clone() {
        let _ = dynamodb_repository
            .put_shop_record(shop.into())
            .await
            .unwrap();
    }

    let opensearch_repository = ShopOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    for shop in shops {
        let _ = opensearch_repository
            .create_shop_document(shop.into())
            .await
            .unwrap();
    }
}
