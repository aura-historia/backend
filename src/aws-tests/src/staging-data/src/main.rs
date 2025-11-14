use aws_tests_common::get_cfn_output;
use common::api::collection::PutCollectionData;
use fake::{
    Fake, Faker,
    rand::{self, seq::IndexedRandom},
};
use product::data::put_data::PutProductData;
use shop::core::shop::Shop;
use shop::dynamodb::{
    repository::{ShopDynamoDbRepository, ShopDynamoDbRepositoryImpl},
    shop_record::ShopRecord,
};
use shop::opensearch::repository::{ShopOpenSearchRepository, ShopOpenSearchRepositoryImpl};
use staging_tests::{get_dynamodb_client, get_opensearch_client};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    staging_tests::reset().await;
    let shops = populate_shops().await;
    populate_products(shops).await;

    Ok(())
}

async fn populate_products(shops: Vec<Shop>) {
    println!("Populating products...");
    let stack = get_cfn_output();
    let put_products_url = format!("{}/api/v1/products", stack.api_gateway_endpoint_url);

    let shop_urls = shops
        .into_iter()
        .flat_map(|shop| shop.urls)
        .collect::<Vec<_>>();

    // create products
    let mut products = fake::vec![PutProductData; 142];
    for product in &mut products {
        let host = shop_urls.choose(&mut fake::rand::rng()).unwrap().clone();
        product.url.set_host(host.host_str()).unwrap();
    }

    let mut payload = PutCollectionData { items: products };
    let response = reqwest::Client::new()
        .put(&put_products_url)
        .json(&payload)
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());
    tokio::time::sleep(Duration::from_secs(30)).await;

    // put updates
    for i in 0..10 {
        for product in &mut payload.items {
            if rand::random_range(0..3) < 1 {
                product.state = Faker.fake();
            }
            if rand::random_range(0..3) < 2 {
                product.price = Some(Faker.fake());
            }
        }
        let collection = PutCollectionData {
            items: payload.items.clone(),
        };
        let response = reqwest::Client::new()
            .put(&put_products_url)
            .json(&collection)
            .send()
            .await
            .unwrap();
        assert_eq!(200, response.status());
        tokio::time::sleep(Duration::from_secs(30)).await;
        println!("Finished products' update-iteration {i}.");
    }
    println!("Populated products.");
}

async fn populate_shops() -> Vec<Shop> {
    println!("Populating shops...");
    let stack = get_cfn_output();
    let shops = fake::vec![Shop; 42];

    let dynamodb_repository =
        ShopDynamoDbRepositoryImpl::new(get_dynamodb_client().await, &stack.dynamodb_table_1_name);
    for shop in shops.clone() {
        let mut shop_records = ShopRecord::try_clone_from_shop_as_shop_url_records(&shop).unwrap();
        shop_records.push(ShopRecord::from_shop_as_shop_id_record(shop));
        let _ = dynamodb_repository
            .put_shop_records_transact(shop_records)
            .await
            .unwrap();
    }

    let opensearch_repository = ShopOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    for shop in shops.clone() {
        let _ = opensearch_repository
            .create_shop_document(shop.into())
            .await
            .unwrap();
    }
    println!("Populated shops.");
    shops
}
