use std::time::Duration;

use aws_tests_common::get_cfn_output;
use common::api::collection::PutCollectionData;
use fake::{Fake, Faker, rand};
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
    println!("Populating items...");
    let stack = get_cfn_output();
    let put_items_url = format!("{}/api/v1/items", stack.api_gateway_endpoint_url);

    // create items
    let mut items = PutCollectionData {
        items: fake::vec![PutItemData; 142],
    };
    let response = reqwest::Client::new()
        .put(&put_items_url)
        .json(&items)
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());
    tokio::time::sleep(Duration::from_secs(30)).await;

    // put updates
    for i in 0..10 {
        for item in &mut items.items {
            if rand::random_range(0..3) < 1 {
                item.state = Faker.fake();
            }
            if rand::random_range(0..3) < 2 {
                item.price = Some(Faker.fake());
            }
        }
        let collection = PutCollectionData {
            items: items.items.clone(),
        };
        let response = reqwest::Client::new()
            .put(&put_items_url)
            .json(&collection)
            .send()
            .await
            .unwrap();
        assert_eq!(200, response.status());
        tokio::time::sleep(Duration::from_secs(30)).await;
        println!("Finished items' update-iteration {i}.");
    }
    println!("Populated items.");
}

async fn populate_shops() {
    println!("Populating shops...");
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
    println!("Populated shops.");
}
