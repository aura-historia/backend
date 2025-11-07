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
use item::{
    core::item_event::{
        ItemEventPayload, ItemPriceChangeEventPayload, ItemStateChangeEventPayload,
    },
    service::get_service::GetItemServiceImpl,
    watchlist::{
        dynamodb::repository::WatchlistItemDynamoDbRepositoryImpl,
        service::item_watchlist_service::ItemWatchListServiceImpl,
    },
};
use item::{
    dynamodb::{
        item_record::ItemRecord,
        repository::{ItemDynamoDbRepository, ItemDynamoDbRepositoryImpl},
    },
    watchlist::service::item_watchlist_service::ItemWatchListService,
};
use staging_tests::{create_random_test_user, get_dynamodb_client, staging_test};
use std::time::{Duration, SystemTime};
use user::dynamodb::repository::UserDynamoDbRepositoryImpl;

#[staging_test]
async fn should_respond_200_when_anon_and_item_does_exist() {
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
    assert_eq!(record.shop_id.to_string(), body["item"]["shopId"]);
    assert_eq!(
        record.shops_item_id.to_string(),
        body["item"]["shopsItemId"]
    );
    assert_eq!(record.item_id.to_string(), body["item"]["itemId"]);
    assert_eq!(record.event_id.to_string(), body["item"]["eventId"]);
    assert_eq!(record.url.to_string(), body["item"]["url"]);
    assert_eq!(record.price_gbp.unwrap(), body["item"]["price"]["amount"]);
    assert_eq!("GBP", body["item"]["price"]["currency"]);
}

#[staging_test]
async fn should_respond_200_personalized_when_authenticated_and_item_does_exist_and_watched() {
    let user = create_random_test_user().await;
    let ddb_client = get_dynamodb_client().await;
    let item_repository =
        ItemDynamoDbRepositoryImpl::new(ddb_client, &get_cfn_output().dynamodb_table_1_name);
    let watchlist_repository = WatchlistItemDynamoDbRepositoryImpl::new(
        ddb_client,
        &get_cfn_output().dynamodb_table_1_name,
    );
    let user_repository =
        UserDynamoDbRepositoryImpl::new(ddb_client, &get_cfn_output().dynamodb_table_1_name);
    let get_item_service = GetItemServiceImpl::new(&item_repository);
    let watchlist_service = ItemWatchListServiceImpl::new(
        &watchlist_repository,
        &user_repository,
        &item_repository,
        &get_item_service,
    );
    let record = Faker.fake::<ItemRecord>();
    let insert_res = item_repository
        .put_item_records([record.clone()].into())
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap().is_empty());
    tokio::time::sleep(Duration::from_secs(1)).await;

    watchlist_service
        .create_watchlist_item(&user.sub.into(), &record.shop_id, &record.shops_item_id)
        .await
        .unwrap();

    let url = format!(
        "{}/api/v1/items/{}/{}?currency=GBP",
        get_cfn_output().api_gateway_endpoint_url,
        record.shop_id,
        record.shops_item_id
    );
    let response = reqwest::Client::new()
        .get(url)
        .bearer_auth(user.access_token)
        .send()
        .await
        .unwrap();

    assert_eq!(200, response.status());

    let body = response.json::<serde_json::Value>().await.unwrap();
    assert_eq!(record.shop_id.to_string(), body["item"]["shopId"]);
    assert_eq!(
        record.shops_item_id.to_string(),
        body["item"]["shopsItemId"]
    );
    assert_eq!(record.item_id.to_string(), body["item"]["itemId"]);
    assert_eq!(record.event_id.to_string(), body["item"]["eventId"]);
    assert_eq!(record.url.to_string(), body["item"]["url"]);
    assert_eq!(record.price_gbp.unwrap(), body["item"]["price"]["amount"]);
    assert_eq!("GBP", body["item"]["price"]["currency"]);
    assert!(
        body["userState"]["watchlist"]["watching"]
            .as_bool()
            .unwrap()
    );
    assert!(
        !body["userState"]["watchlist"]["notifications"]
            .as_bool()
            .unwrap()
    );
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
