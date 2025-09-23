use common::{
    currency::domain::Currency,
    event::Event,
    event_id::EventId,
    item_state::domain::ItemState,
    price::domain::{FixedFxRate, FxRate, Price},
};
use fake::{Fake, Faker};
use item_api_get_item::handler;
use item_core::{
    hash::ItemHash,
    item_event::{ItemEventPayload, ItemPriceChangeEventPayload, ItemStateChangeEventPayload},
};
use item_dynamodb::{
    item_record::ItemRecord,
    repository::{ItemDynamoDbRepository, ItemDynamoDbRepositoryImpl},
};
use item_service::get_service::GetItemServiceImpl;
use lambda_runtime::LambdaEvent;
use std::time::{Duration, SystemTime};
use test_api::*;

#[localstack_test(services = [DynamoDB()])]
async fn should_respond_200_with_history_when_history_flag_true() {
    let ddb_client = get_dynamodb_client().await;
    let repository = ItemDynamoDbRepositoryImpl::new(ddb_client, "table_1");
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
            native_price: event_1_price,
            other_price: FixedFxRate()
                .exchange_all(event_1_price.currency, event_1_price.monetary_amount)
                .unwrap(),
            hash: ItemHash::new(&Some(event_1_price), &record.state.into()),
        }),
    };
    tokio::time::sleep(Duration::from_secs(1)).await;
    let event_2_id = EventId::new();
    let event_2 = Event {
        aggregate_id: record.item_id,
        event_id: event_2_id,
        timestamp: SystemTime::now().into(),
        payload: ItemEventPayload::StateRemoved(ItemStateChangeEventPayload {
            shop_id: record.shop_id,
            shops_item_id: record.shops_item_id.clone(),
            hash: ItemHash::new(&Some(event_1_price), &ItemState::Removed),
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

    let service = GetItemServiceImpl::new(&repository);
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .path_parameter("shopId", record.shop_id)
            .path_parameter("shopsItemId", record.shops_item_id)
            .query_string_parameter("history", "true")
            .query_string_parameter("currency", "USD")
            .build(),
        context: Default::default(),
    };
    let response = handler(lambda_event, &service).await.unwrap();

    assert_eq!(200, response.status_code);

    let body = extract_apigw_response_json_body!(response);

    let history = body["history"].as_array().unwrap();
    assert_eq!(2, history.len());
    assert_eq!(event_1_id.to_string(), history[0]["eventId"]);
    assert_eq!("PRICE_DROPPED", history[0]["eventType"]);
    assert_eq!("USD", history[0]["payload"]["currency"]);
    assert_eq!(
        u64::from(
            event_1_price
                .into_exchanged(&FixedFxRate(), Currency::Usd)
                .unwrap()
                .monetary_amount
        ),
        history[0]["payload"]["amount"]
    );
    assert_eq!(event_2_id.to_string(), history[1]["eventId"]);
    assert_eq!("STATE_REMOVED", history[1]["eventType"]);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_respond_200_with_history_when_history_flag_false() {
    let ddb_client = get_dynamodb_client().await;
    let repository = ItemDynamoDbRepositoryImpl::new(ddb_client, "table_1");
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
            native_price: event_1_price,
            other_price: FixedFxRate()
                .exchange_all(event_1_price.currency, event_1_price.monetary_amount)
                .unwrap(),
            hash: ItemHash::new(&Some(event_1_price), &record.state.into()),
        }),
    };
    tokio::time::sleep(Duration::from_secs(1)).await;
    let event_2_id = EventId::new();
    let event_2 = Event {
        aggregate_id: record.item_id,
        event_id: event_2_id,
        timestamp: SystemTime::now().into(),
        payload: ItemEventPayload::StateRemoved(ItemStateChangeEventPayload {
            shop_id: record.shop_id,
            shops_item_id: record.shops_item_id.clone(),
            hash: ItemHash::new(&Some(event_1_price), &ItemState::Removed),
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

    let service = GetItemServiceImpl::new(&repository);
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .path_parameter("shopId", record.shop_id)
            .path_parameter("shopsItemId", record.shops_item_id)
            .query_string_parameter("history", "false")
            .build(),
        context: Default::default(),
    };
    let response = handler(lambda_event, &service).await.unwrap();

    assert_eq!(200, response.status_code);

    let body = extract_apigw_response_json_body!(response);
    assert!(body["history"].is_null())
}
