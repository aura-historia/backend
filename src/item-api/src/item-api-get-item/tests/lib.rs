use common::{
    cognito::MockCognitoService,
    currency::domain::Currency,
    event::Event,
    event_id::EventId,
    item_state::domain::ItemState,
    price::domain::{FixedFxRate, FxRate, Price},
};
use fake::{Fake, Faker};
use item::service::get_service::GetItemServiceImpl;
use item::{
    core::item_event::{
        ItemEventPayload, ItemPriceChangeEventPayload, ItemStateChangeEventPayload,
    },
    service::personalization_service::ItemPersonalizationServiceImpl,
};
use item::{
    dynamodb::{
        item_record::ItemRecord,
        repository::{ItemDynamoDbRepository, ItemDynamoDbRepositoryImpl},
    },
    watchlist::dynamodb::repository::WatchlistItemDynamoDbRepositoryImpl,
};
use item_api_get_item::handler;
use lambda_runtime::LambdaEvent;
use std::time::{Duration, SystemTime};
use test_api::*;

#[localstack_test(services = [DynamoDB()])]
async fn should_respond_200_with_history_when_anon_and_history_flag_true() {
    let ddb_client = get_dynamodb_client().await;
    let item_repository = ItemDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let watchlist_repository = WatchlistItemDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let get_item_service = GetItemServiceImpl::new(&item_repository);
    let item_personalization_service = ItemPersonalizationServiceImpl::new(&watchlist_repository);
    let mut cognito_service = MockCognitoService::default();
    cognito_service
        .expect_verify_extract_user_id()
        .return_once(|_| Box::pin(async { Ok(None) }));

    let record = Faker.fake::<ItemRecord>();
    let insert_res = item_repository
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
    tokio::time::sleep(Duration::from_secs(1)).await;
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
    let insert_res = item_repository
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

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .path_parameter("shopId", record.shop_id)
            .path_parameter("shopsItemId", record.shops_item_id)
            .query_string_parameter("history", "true")
            .query_string_parameter("currency", "USD")
            .build(),
        context: Default::default(),
    };
    let response = handler(
        lambda_event,
        &get_item_service,
        &cognito_service,
        &item_personalization_service,
    )
    .await
    .unwrap();

    assert_eq!(200, response.status_code);

    let body = extract_apigw_response_json_body!(response);

    assert!(body.get("userState").is_none());
    let history = body["item"]["history"].as_array().unwrap();
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

#[localstack_test(services = [DynamoDB()])]
async fn should_respond_200_with_history_when_anon_and_history_flag_false() {
    let ddb_client = get_dynamodb_client().await;
    let item_repository = ItemDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let watchlist_repository = WatchlistItemDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let get_item_service = GetItemServiceImpl::new(&item_repository);
    let item_personalization_service = ItemPersonalizationServiceImpl::new(&watchlist_repository);
    let mut cognito_service = MockCognitoService::default();
    cognito_service
        .expect_verify_extract_user_id()
        .return_once(|_| Box::pin(async { Ok(None) }));

    let record = Faker.fake::<ItemRecord>();
    let insert_res = item_repository
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
    tokio::time::sleep(Duration::from_secs(1)).await;
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
    let insert_res = item_repository
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

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .path_parameter("shopId", record.shop_id)
            .path_parameter("shopsItemId", record.shops_item_id)
            .query_string_parameter("history", "false")
            .build(),
        context: Default::default(),
    };
    let response = handler(
        lambda_event,
        &get_item_service,
        &cognito_service,
        &item_personalization_service,
    )
    .await
    .unwrap();

    assert_eq!(200, response.status_code);

    let body = extract_apigw_response_json_body!(response);
    assert!(body.get("userState").is_none());
    assert!(body["item"]["history"].is_null())
}
