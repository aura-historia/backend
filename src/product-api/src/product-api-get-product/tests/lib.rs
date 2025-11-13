use cognito::access_token_verifier_service::MockAccessTokenVerifierService;
use common::{
    currency::domain::Currency,
    event::Event,
    event_id::EventId,
    price::domain::{FixedFxRate, FxRate, Price},
    product_state::domain::ProductState,
};
use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use product::{
    core::product_event::{
        ProductPriceChangeEventPayload, ProductStateChangeEventPayload, ProductEventPayload,
    },
    service::personalization_service::ProductPersonalizationServiceImpl,
    watchlist::service::{
        command::UpdateWatchlistProductCommand, product_watchlist_service::ProductWatchListService,
    },
};
use product::{
    dynamodb::{
        product_record::ProductRecord,
        repository::{ProductDynamoDbRepository, ProductDynamoDbRepositoryImpl},
    },
    watchlist::dynamodb::repository::WatchlistProductDynamoDbRepositoryImpl,
};
use product::{
    service::get_service::GetProductServiceImpl,
    watchlist::service::product_watchlist_service::ProductWatchListServiceImpl,
};
use product_api_get_product::handler;
use std::time::{Duration, SystemTime};
use test_api::*;
use user::dynamodb::{
    repository::{UserDynamoDbRepository, UserDynamoDbRepositoryImpl},
    user_record::UserRecord,
};

#[localstack_test(services = [DynamoDB()])]
async fn should_respond_200_with_history_when_anon_and_history_flag_true() {
    let ddb_client = get_dynamodb_client().await;
    let product_repository = ProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let get_product_service = GetProductServiceImpl::new(&product_repository);
    let product_personalization_service =
        ProductPersonalizationServiceImpl::new(&watchlist_repository);
    let mut access_token_verifier_service = MockAccessTokenVerifierService::default();
    access_token_verifier_service
        .expect_verify_extract_user_id()
        .return_once(|_| Box::pin(async { Ok(None) }));

    let record = Faker.fake::<ProductRecord>();
    let insert_res = product_repository
        .put_product_records([record.clone()].into())
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap().is_empty());
    tokio::time::sleep(Duration::from_secs(1)).await;

    let event_1_id = EventId::new();
    let event_1_price = Price::new(1000u64.into(), Currency::Eur);
    let event_1 = Event {
        aggregate_id: record.product_id,
        event_id: event_1_id,
        timestamp: SystemTime::now().into(),
        payload: ProductEventPayload::PriceDropped(ProductPriceChangeEventPayload {
            shop_id: record.shop_id,
            shops_product_id: record.shops_product_id.clone(),
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
        aggregate_id: record.product_id,
        event_id: event_2_id,
        timestamp: SystemTime::now().into(),
        payload: ProductEventPayload::StateRemoved(ProductStateChangeEventPayload {
            shop_id: record.shop_id,
            shops_product_id: record.shops_product_id.clone(),
            old_state: ProductState::Sold,
        }),
    };
    let insert_res = product_repository
        .put_product_event_records(
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
            .path_parameter("shopsProductId", record.shops_product_id)
            .query_string_parameter("history", "true")
            .query_string_parameter("currency", "USD")
            .build(),
        context: Default::default(),
    };
    let response = handler(
        lambda_event,
        &get_product_service,
        &access_token_verifier_service,
        &product_personalization_service,
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
async fn should_respond_200_without_history_when_anon_and_history_flag_false() {
    let ddb_client = get_dynamodb_client().await;
    let product_repository = ProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let get_product_service = GetProductServiceImpl::new(&product_repository);
    let product_personalization_service =
        ProductPersonalizationServiceImpl::new(&watchlist_repository);
    let mut access_token_verifier_service = MockAccessTokenVerifierService::default();
    access_token_verifier_service
        .expect_verify_extract_user_id()
        .return_once(|_| Box::pin(async { Ok(None) }));

    let record = Faker.fake::<ProductRecord>();
    let insert_res = product_repository
        .put_product_records([record.clone()].into())
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap().is_empty());
    tokio::time::sleep(Duration::from_secs(1)).await;

    let event_1_id = EventId::new();
    let event_1_price = Price::new(1000u64.into(), Currency::Eur);
    let event_1 = Event {
        aggregate_id: record.product_id,
        event_id: event_1_id,
        timestamp: SystemTime::now().into(),
        payload: ProductEventPayload::PriceDropped(ProductPriceChangeEventPayload {
            shop_id: record.shop_id,
            shops_product_id: record.shops_product_id.clone(),
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
        aggregate_id: record.product_id,
        event_id: event_2_id,
        timestamp: SystemTime::now().into(),
        payload: ProductEventPayload::StateRemoved(ProductStateChangeEventPayload {
            shop_id: record.shop_id,
            shops_product_id: record.shops_product_id.clone(),
            old_state: ProductState::Sold,
        }),
    };
    let insert_res = product_repository
        .put_product_event_records(
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
            .path_parameter("shopsProductId", record.shops_product_id)
            .query_string_parameter("history", "false")
            .build(),
        context: Default::default(),
    };
    let response = handler(
        lambda_event,
        &get_product_service,
        &access_token_verifier_service,
        &product_personalization_service,
    )
    .await
    .unwrap();

    assert_eq!(200, response.status_code);

    let body = extract_apigw_response_json_body!(response);
    assert!(body.get("userState").is_none());
    assert!(body["item"]["history"].is_null())
}

#[localstack_test(services = [DynamoDB()])]
async fn should_respond_200_personalized_when_authenticated_and_not_watched() {
    let ddb_client = get_dynamodb_client().await;
    let user_repository = UserDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let product_repository = ProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let get_product_service = GetProductServiceImpl::new(&product_repository);
    let product_personalization_service =
        ProductPersonalizationServiceImpl::new(&watchlist_repository);

    let user_record = Faker.fake::<UserRecord>();
    let _ = user_repository
        .put_user_record(user_record.clone())
        .await
        .unwrap();
    let mut access_token_verifier_service = MockAccessTokenVerifierService::default();
    access_token_verifier_service
        .expect_verify_extract_user_id()
        .return_once(move |_| Box::pin(async move { Ok(Some(user_record.id)) }));

    let record = Faker.fake::<ProductRecord>();
    let insert_res = product_repository
        .put_product_records([record.clone()].into())
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap().is_empty());
    tokio::time::sleep(Duration::from_secs(1)).await;

    let event_1_id = EventId::new();
    let event_1_price = Price::new(1000u64.into(), Currency::Eur);
    let event_1 = Event {
        aggregate_id: record.product_id,
        event_id: event_1_id,
        timestamp: SystemTime::now().into(),
        payload: ProductEventPayload::PriceDropped(ProductPriceChangeEventPayload {
            shop_id: record.shop_id,
            shops_product_id: record.shops_product_id.clone(),
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
        aggregate_id: record.product_id,
        event_id: event_2_id,
        timestamp: SystemTime::now().into(),
        payload: ProductEventPayload::StateRemoved(ProductStateChangeEventPayload {
            shop_id: record.shop_id,
            shops_product_id: record.shops_product_id.clone(),
            old_state: ProductState::Sold,
        }),
    };
    let insert_res = product_repository
        .put_product_event_records(
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
            .path_parameter("shopsProductId", record.shops_product_id)
            .query_string_parameter("history", "false")
            .build(),
        context: Default::default(),
    };
    let response = handler(
        lambda_event,
        &get_product_service,
        &access_token_verifier_service,
        &product_personalization_service,
    )
    .await
    .unwrap();

    assert_eq!(200, response.status_code);

    let body = extract_apigw_response_json_body!(response);
    assert!(body["item"]["history"].is_null());
    assert!(
        !body["userState"]["watchlist"]["watching"]
            .as_bool()
            .unwrap()
    );
    assert!(
        !body["userState"]["watchlist"]["notifications"]
            .as_bool()
            .unwrap()
    );
}

#[localstack_test(services = [DynamoDB()])]
async fn should_respond_200_personalized_when_authenticated_and_watched() {
    let ddb_client = get_dynamodb_client().await;
    let user_repository = UserDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let product_repository = ProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let get_product_service = GetProductServiceImpl::new(&product_repository);
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let watchlist_service = ProductWatchListServiceImpl::new(
        &watchlist_repository,
        &user_repository,
        &product_repository,
        &get_product_service,
    );
    let product_personalization_service =
        ProductPersonalizationServiceImpl::new(&watchlist_repository);

    let user_record = Faker.fake::<UserRecord>();
    let _ = user_repository
        .put_user_record(user_record.clone())
        .await
        .unwrap();
    let mut access_token_verifier_service = MockAccessTokenVerifierService::default();
    access_token_verifier_service
        .expect_verify_extract_user_id()
        .return_once(move |_| Box::pin(async move { Ok(Some(user_record.id)) }));

    let record = Faker.fake::<ProductRecord>();
    let insert_res = product_repository
        .put_product_records([record.clone()].into())
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap().is_empty());
    tokio::time::sleep(Duration::from_secs(1)).await;

    let _ = watchlist_service
        .create_watchlist_product(&user_record.id, &record.shop_id, &record.shops_product_id)
        .await
        .unwrap();
    let _ = watchlist_service
        .update_watchlist_product(
            &user_record.id,
            &record.shop_id,
            &record.shops_product_id,
            UpdateWatchlistProductCommand {
                notifications: Some(true),
            },
        )
        .await
        .unwrap();

    let event_1_id = EventId::new();
    let event_1_price = Price::new(1000u64.into(), Currency::Eur);
    let event_1 = Event {
        aggregate_id: record.product_id,
        event_id: event_1_id,
        timestamp: SystemTime::now().into(),
        payload: ProductEventPayload::PriceDropped(ProductPriceChangeEventPayload {
            shop_id: record.shop_id,
            shops_product_id: record.shops_product_id.clone(),
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
        aggregate_id: record.product_id,
        event_id: event_2_id,
        timestamp: SystemTime::now().into(),
        payload: ProductEventPayload::StateRemoved(ProductStateChangeEventPayload {
            shop_id: record.shop_id,
            shops_product_id: record.shops_product_id.clone(),
            old_state: ProductState::Sold,
        }),
    };
    let insert_res = product_repository
        .put_product_event_records(
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
            .path_parameter("shopsProductId", record.shops_product_id)
            .query_string_parameter("history", "false")
            .build(),
        context: Default::default(),
    };
    let response = handler(
        lambda_event,
        &get_product_service,
        &access_token_verifier_service,
        &product_personalization_service,
    )
    .await
    .unwrap();

    assert_eq!(200, response.status_code);

    let body = extract_apigw_response_json_body!(response);
    assert!(body["item"]["history"].is_null());
    assert!(
        body["userState"]["watchlist"]["watching"]
            .as_bool()
            .unwrap()
    );
    assert!(
        body["userState"]["watchlist"]["notifications"]
            .as_bool()
            .unwrap()
    );
}
