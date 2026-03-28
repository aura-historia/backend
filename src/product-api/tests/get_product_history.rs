use common::{
    currency::domain::Currency,
    event::Event,
    event_id::EventId,
    price::domain::{FixedFxRate, FxRate, Price},
    product_state::domain::ProductState,
};
use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use product::core::product_event::{
    ProductEventPayload,
    domain::{
        ProductDomainEventPayload, ProductPriceChangeDomainEventPayload,
        ProductStateChangeDomainEventPayload,
    },
};
use product::dynamodb::{
    product_record::ProductRecord,
    repository::{ProductDynamoDbRepository, ProductDynamoDbRepositoryImpl},
};
use product::service::get_service::GetProductServiceImpl;
use product_api::get_product_history::handle;
use std::time::{Duration, SystemTime};
use test_api::*;

#[localstack_test(services = [DynamoDB()])]
async fn should_respond_200() {
    let ddb_client = get_dynamodb_client().await;
    let product_repository = ProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let get_product_service = GetProductServiceImpl::new(&product_repository);

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
        payload: ProductEventPayload::ProductDomainEvent(ProductDomainEventPayload::PriceChanged(
            ProductPriceChangeDomainEventPayload {
                shop_id: record.shop_id,
                shops_product_id: record.shops_product_id.clone(),
                new_native_price: Some(event_1_price),
                new_other_price: FixedFxRate()
                    .exchange_all(event_1_price.currency, event_1_price.monetary_amount)
                    .unwrap(),
                old_native_price: Some(Price {
                    monetary_amount: 100000u64.into(),
                    currency: Currency::Eur,
                }),
                old_other_price: FixedFxRate()
                    .exchange_all(Currency::Eur, 100000u64.into())
                    .unwrap(),
            },
        )),
    };
    tokio::time::sleep(Duration::from_secs(1)).await;
    let event_2_id = EventId::new();
    let event_2 = Event {
        aggregate_id: record.product_id,
        event_id: event_2_id,
        timestamp: SystemTime::now().into(),
        payload: ProductEventPayload::ProductDomainEvent(ProductDomainEventPayload::StateChanged(
            ProductStateChangeDomainEventPayload {
                shop_id: record.shop_id,
                shops_product_id: record.shops_product_id.clone(),
                old_state: ProductState::Sold,
                new_state: ProductState::Removed,
            },
        )),
    };
    let insert_res = product_repository
        .put_product_event_records([event_1.clone().into(), event_2.clone().into()].into())
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap().is_empty());
    tokio::time::sleep(Duration::from_secs(1)).await;

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .route_key("GET /api/v1/shops/{shopId}/products/{shopsProductId}/history".to_owned())
            .path_parameter("shopId", record.shop_id)
            .path_parameter("shopsProductId", record.shops_product_id)
            .query_string_parameter("currency", "USD")
            .build(),
        context: Default::default(),
    };
    let response = handle(lambda_event, &get_product_service).await.unwrap();

    assert_eq!(200, response.status_code);

    let body = extract_apigw_response_json_body!(response);
    let history = body.as_array().cloned().unwrap();

    assert_eq!(2, history.len());
    assert_eq!(event_1_id.to_string(), history[0]["eventId"]);
    assert_eq!("PRICE_CHANGED", history[0]["eventType"]);
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
    assert_eq!("STATE_CHANGED", history[1]["eventType"]);
}
