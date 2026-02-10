use aws_tests_common::get_cfn_output;
use common::api::collection::PutCollectionData;
use fake::{
    Fake, Faker,
    rand::{self, seq::IndexedRandom},
};
use opensearch::{IndexParts, params::Refresh};
use product::data::put_data::PutProductData;
use product_classification::category::{
    core::Category,
    dynamodb_repository::CategoryDynamoDbRepositoryImpl,
    opensearch_repository::CategoryOpenSearchRepositoryImpl,
    service::{CategoryService, CategoryServiceImpl},
};
use shop::data::{get_shop_data::GetShopData, post_shop_data::PostShopData};
use staging_tests::{get_dynamodb_client, get_opensearch_client};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    staging_tests::reset().await;
    let shops = populate_shops().await;
    populate_products(shops).await;
    let _ = populate_categories().await;

    Ok(())
}

async fn populate_products(shops: Vec<GetShopData>) {
    println!("Populating products...");
    let stack = get_cfn_output();
    let put_products_url = format!("{}/api/v1/products", stack.api_gateway_endpoint_url);

    let shop_domains = shops
        .into_iter()
        .flat_map(|shop| shop.domains)
        .collect::<Vec<_>>();

    // create products
    let mut products = fake::vec![PutProductData; 142];
    for product in &mut products {
        let host = shop_domains.choose(&mut fake::rand::rng()).unwrap().clone();
        product.url.set_host(Some(host.as_str())).unwrap();
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

async fn populate_shops() -> Vec<GetShopData> {
    println!("Populating shops...");
    let stack = get_cfn_output();

    let http = reqwest::Client::new();
    let post_shop_url = format!("{}/api/v1/shops", stack.api_gateway_endpoint_url);
    let mut shops = vec![];
    for _ in 0..42 {
        let mut post_shop_data = Faker.fake::<PostShopData>();
        post_shop_data.domains.insert(Faker.fake());
        let shop = http
            .post(&post_shop_url)
            .json(&post_shop_data)
            .send()
            .await
            .unwrap()
            .json::<GetShopData>()
            .await
            .unwrap();
        shops.push(shop);
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    tokio::time::sleep(Duration::from_secs(30)).await;
    println!("Populated shops.");
    shops
}

async fn populate_categories() -> Vec<Category> {
    let dynamodb_repository = CategoryDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &get_cfn_output().dynamodb_table_1_name,
    );
    let opensearch = get_opensearch_client().await;
    let opensearch_repository = CategoryOpenSearchRepositoryImpl::new(opensearch);
    let category_service = CategoryServiceImpl::new(&dynamodb_repository, &opensearch_repository);

    let categories = Category::load_categories();
    for category in &categories {
        let _ = category_service
            .upsert_category(category.clone())
            .await
            .unwrap();
    }
    refresh_index("categories").await;
    categories
}

pub async fn refresh_index(index: &str) {
    get_opensearch_client()
        .await
        .index(IndexParts::Index(index))
        .refresh(Refresh::True)
        .send()
        .await
        .unwrap();
}
