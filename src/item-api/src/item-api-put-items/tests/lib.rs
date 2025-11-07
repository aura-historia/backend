use common::{api::collection::PutCollectionData, price::domain::FixedFxRate};
use fake::{Fake, Faker};
use item::data::put_data::PutItemData;
use item::dynamodb::repository::ItemDynamoDbRepositoryImpl;
use item::service::{
    enrichment_service::ItemCommandEnrichmentServiceImpl, upsert_service::UpsertItemsServiceImpl,
};
use item_api_put_items::{PutItemsResponse, handler};
use lambda_runtime::LambdaEvent;
use shop::core::shop::Shop;
use shop::dynamodb::{
    repository::{ShopDynamoDbRepository, ShopDynamoDbRepositoryImpl},
    shop_record::ShopRecord,
};
use test_api::*;

const INGEST_QUEUE: Sqs = Sqs {
    name: "ingest_queue",
};

#[localstack_test(services = [DynamoDB(), INGEST_QUEUE])]
async fn should_fail_items_with_unknown_url() {
    let dynamodb_client = get_dynamodb_client().await;
    let sqs_client = get_sqs_client().await;
    let shop_repository = ShopDynamoDbRepositoryImpl::new(dynamodb_client, "table_1");
    let item_repository = ItemDynamoDbRepositoryImpl::new(dynamodb_client, "table_1");
    let fx_rate = FixedFxRate();
    let queue_url = INGEST_QUEUE.queue_url();
    let enrichment_service = ItemCommandEnrichmentServiceImpl::new(&shop_repository, &fx_rate);
    let upsert_service =
        UpsertItemsServiceImpl::new(&item_repository, sqs_client, &queue_url, &fx_rate);

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::PUT)
            .body_serde(&PutCollectionData {
                items: fake::vec![PutItemData; 1870],
            })
            .build(),
        context: Default::default(),
    };
    let response = handler(lambda_event, &upsert_service, &enrichment_service)
        .await
        .unwrap();

    let actual_json = extract_apigw_response_json_body!(response);
    let actual = serde_json::from_value::<PutItemsResponse>(actual_json).unwrap();

    assert!(actual.unprocessed.is_empty());
    assert_eq!(0, actual.skipped);
    assert_eq!(1870, actual.failed.len());
}

#[localstack_test(services = [DynamoDB(), INGEST_QUEUE])]
async fn should_put_items_with_known_url() {
    let dynamodb_client = get_dynamodb_client().await;
    let sqs_client = get_sqs_client().await;
    let shop_repository = ShopDynamoDbRepositoryImpl::new(dynamodb_client, "table_1");
    let item_repository = ItemDynamoDbRepositoryImpl::new(dynamodb_client, "table_1");
    let fx_rate = FixedFxRate();
    let queue_url = INGEST_QUEUE.queue_url();
    let enrichment_service = ItemCommandEnrichmentServiceImpl::new(&shop_repository, &fx_rate);
    let upsert_service =
        UpsertItemsServiceImpl::new(&item_repository, sqs_client, &queue_url, &fx_rate);

    let shop = Faker.fake::<Shop>();
    let mut shop_records = ShopRecord::try_clone_from_shop_as_shop_url_records(&shop).unwrap();
    shop_records.push(ShopRecord::from_shop_as_shop_id_record(shop.clone()));
    let _ = shop_repository
        .put_shop_records_transact(shop_records)
        .await
        .unwrap();

    let mut items = fake::vec![PutItemData; 235];
    for item in &mut items {
        let shop_host = shop.urls[0].host_str();
        item.url.set_host(shop_host).unwrap();
    }
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::PUT)
            .body_serde(&PutCollectionData {
                items: items.clone(),
            })
            .build(),
        context: Default::default(),
    };
    let response = handler(lambda_event, &upsert_service, &enrichment_service)
        .await
        .unwrap();

    let actual_json = extract_apigw_response_json_body!(response);
    let actual = serde_json::from_value::<PutItemsResponse>(actual_json).unwrap();

    assert!(actual.unprocessed.is_empty());
    assert!(actual.failed.is_empty());
    assert_eq!(0, actual.skipped);

    let mut message_count = 0;
    loop {
        let resp = sqs_client
            .receive_message()
            .queue_url(queue_url.clone())
            .max_number_of_messages(10)
            .visibility_timeout(600)
            .send()
            .await
            .unwrap();

        let messages = resp.messages.unwrap_or_default();
        message_count += messages.len();
        if messages.is_empty() {
            break;
        }
    }
    assert_eq!(235, message_count);
}
