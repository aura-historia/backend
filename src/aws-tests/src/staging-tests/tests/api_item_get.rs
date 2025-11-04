use aws_tests_common::get_cfn_output;
use common::{
    currency::domain::Currency,
    event::Event,
    event_id::EventId,
    item_state::domain::ItemState,
    price::domain::{FixedFxRate, FxRate, Price},
    shop_id::ShopId,
};
use fake::{Fake, Faker};
use item::core::item_event::{
    ItemEventPayload, ItemPriceChangeEventPayload, ItemStateChangeEventPayload,
};
use item::dynamodb::{
    item_record::ItemRecord,
    repository::{ItemDynamoDbRepository, ItemDynamoDbRepositoryImpl},
};
use staging_tests::{get_dynamodb_client, staging_test};
use std::time::{Duration, SystemTime};

#[staging_test]
async fn should_respond_200_when_item_does_exist() {
    let ddb_client = get_dynamodb_client().await;
    let repository =
        ItemDynamoDbRepositoryImpl::new(ddb_client, &get_cfn_output().dynamodb_table_1_name);
    let record = Faker.fake::<ItemRecord>();
    let insert_res = repository
        .put_item_records([record.clone()].into())
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap().is_empty());
    tokio::time::sleep(Duration::from_secs(1)).await;

    let url = format!(
        "{}/api/v1/items/{}/{}?currency=GBP",
        get_cfn_output().api_gateway_endpoint_url,
        record.shop_id,
        record.shops_item_id
    );
    let response = reqwest::get(url).await.unwrap();

    assert_eq!(200, response.status());

    let body = response.json::<serde_json::Value>().await.unwrap();
    assert_eq!(record.shop_id.to_string(), body["shopId"]);
    assert_eq!(record.shops_item_id.to_string(), body["shopsItemId"]);
    assert_eq!(record.item_id.to_string(), body["itemId"]);
    assert_eq!(record.event_id.to_string(), body["eventId"]);
    assert_eq!(record.url.to_string(), body["url"]);
    assert_eq!(record.price_gbp.unwrap(), body["price"]["amount"]);
    assert_eq!("GBP", body["price"]["currency"]);
}

#[staging_test]
async fn should_respond_200_with_history() {
    let ddb_client = get_dynamodb_client().await;
    let repository =
        ItemDynamoDbRepositoryImpl::new(ddb_client, &get_cfn_output().dynamodb_table_1_name);
    let record = Faker.fake::<ItemRecord>();
    let insert_res = repository
        .put_item_records([record.clone()].into())
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap().is_empty());
    tokio::time::sleep(Duration::from_secs(1)).await;

    let event_1_id = EventId::new();
    let event_1_price = Price::new(1000u64.into(), Currency::Eur);
    let event_1 = Event {
        aggregate_id: record.item_id,
        event_id: event_1_id,
        timestamp: SystemTime::now().into(),
        payload: ItemEventPayload::PriceDropped(ItemPriceChangeEventPayload {
            shop_id: record.shop_id,
            shops_item_id: record.shops_item_id.clone(),
            new_native_price: event_1_price,
            new_other_price: FixedFxRate()
                .exchange_all(event_1_price.currency, event_1_price.monetary_amount)
                .unwrap(),
            old_native_price: Price {
                monetary_amount: 100000u64.into(),
                currency: Currency::Eur,
            },
            old_other_price: FixedFxRate()
                .exchange_all(Currency::Eur, 100000u64.into())
                .unwrap(),
        }),
    };
    let event_2_id = EventId::new();
    let event_2 = Event {
        aggregate_id: record.item_id,
        event_id: event_2_id,
        timestamp: SystemTime::now().into(),
        payload: ItemEventPayload::StateRemoved(ItemStateChangeEventPayload {
            shop_id: record.shop_id,
            shops_item_id: record.shops_item_id.clone(),
            old_state: ItemState::Sold,
        }),
    };
    let insert_res = repository
        .put_item_event_records(
            [
                event_1.clone().try_into().unwrap(),
                event_2.clone().try_into().unwrap(),
            ]
            .into(),
        )
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap().is_empty());
    tokio::time::sleep(Duration::from_secs(1)).await;

    let url = format!(
        "{}/api/v1/items/{}/{}?currency=USD&history=true",
        get_cfn_output().api_gateway_endpoint_url,
        record.shop_id,
        record.shops_item_id
    );
    let response = reqwest::get(url).await.unwrap();

    assert_eq!(200, response.status());

    let body = response.json::<serde_json::Value>().await.unwrap();
    let history = body["history"].as_array().unwrap();
    assert_eq!(2, history.len());
    assert_eq!(event_1_id.to_string(), history[0]["eventId"]);
    assert_eq!("PRICE_DROPPED", history[0]["eventType"]);
    assert_eq!("USD", history[0]["payload"]["newPrice"]["currency"]);
    assert_eq!(
        u64::from(
            event_1_price
                .into_exchanged(&FixedFxRate(), Currency::Usd)
                .unwrap()
                .monetary_amount
        ),
        history[0]["payload"]["newPrice"]["amount"]
    );
    assert_eq!(event_2_id.to_string(), history[1]["eventId"]);
    assert_eq!("STATE_REMOVED", history[1]["eventType"]);
}

#[staging_test]
async fn should_respond_404_when_item_does_not_exist() {
    let response = reqwest::get(format!(
        "{}/api/v1/items/{}/bar",
        get_cfn_output().api_gateway_endpoint_url,
        ShopId::new()
    ))
    .await
    .unwrap();
    assert_eq!(404, response.status());

    let body = response.json::<serde_json::Value>().await.unwrap();
    assert_eq!(404, body["status"]);
    assert_eq!("ITEM_NOT_FOUND", body["error"]);
}
