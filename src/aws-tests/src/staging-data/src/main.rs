use aws_tests_common::get_cfn_output;
use common::api::collection::PutCollectionData;
use fake::{Fake, Faker};
use item_data::put_data::PutItemData;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let stack = get_cfn_output();
    let put_items_url = format!("{}/api/v1/items", stack.api_gateway_endpoint_url);

    staging_tests::reset().await;

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

    let mut updates = put_item_commands
        .items
        .into_iter()
        .take(75)
        .collect::<Vec<_>>();
    for put_item in &mut updates {
        put_item.state = Faker.fake();
        put_item.price = Some(Faker.fake());
    }
    let put_update_commands = PutCollectionData { items: updates };
    let response = reqwest::Client::new()
        .put(&put_items_url)
        .json(&put_update_commands)
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());

    Ok(())
}
