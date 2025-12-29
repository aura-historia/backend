use common::domain::Domain;
use common::{api::collection::PutCollectionData, price::domain::FixedFxRate};
use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use product::data::put_data::PutProductData;
use product::dynamodb::repository::ProductDynamoDbRepositoryImpl;
use product::service::{
    enrichment_service::ProductCommandEnrichmentServiceImpl,
    upsert_service::UpsertProductsServiceImpl,
};
use product_api_put_products::{PutProductsResponse, handler};
use shop::core::shop::Shop;
use shop::dynamodb::{
    repository::{ShopDynamoDbRepository, ShopDynamoDbRepositoryImpl},
    shop_record::ShopRecord,
};
use test_api::*;
use time::OffsetDateTime;

const INGEST_QUEUE: Sqs = Sqs {
    name: "ingest_queue",
};

#[localstack_test(services = [DynamoDB(), INGEST_QUEUE])]
async fn should_fail_products_with_unknown_domain() {
    let dynamodb_client = get_dynamodb_client().await;
    let sqs_client = get_sqs_client().await;
    let shop_repository = ShopDynamoDbRepositoryImpl::new(dynamodb_client, "table_1");
    let product_repository = ProductDynamoDbRepositoryImpl::new(dynamodb_client, "table_1");
    let fx_rate = FixedFxRate();
    let queue_url = INGEST_QUEUE.queue_url();
    let enrichment_service = ProductCommandEnrichmentServiceImpl::new(&shop_repository, &fx_rate);
    let upsert_service =
        UpsertProductsServiceImpl::new(&product_repository, sqs_client, &queue_url, &fx_rate);

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::PUT)
            .body_serde(&PutCollectionData {
                items: fake::vec![PutProductData; 1870],
            })
            .build(),
        context: Default::default(),
    };
    let response = handler(lambda_event, &upsert_service, &enrichment_service)
        .await
        .unwrap();

    let actual_json = extract_apigw_response_json_body!(response);
    let actual = serde_json::from_value::<PutProductsResponse>(actual_json).unwrap();

    assert!(actual.unprocessed.is_empty());
    assert_eq!(0, actual.skipped);
    assert_eq!(1870, actual.failed.len());
}

#[localstack_test(services = [DynamoDB(), INGEST_QUEUE])]
async fn should_put_products_with_known_domain() {
    let dynamodb_client = get_dynamodb_client().await;
    let sqs_client = get_sqs_client().await;
    let shop_repository = ShopDynamoDbRepositoryImpl::new(dynamodb_client, "table_1");
    let product_repository = ProductDynamoDbRepositoryImpl::new(dynamodb_client, "table_1");
    let fx_rate = FixedFxRate();
    let queue_url = INGEST_QUEUE.queue_url();
    let enrichment_service = ProductCommandEnrichmentServiceImpl::new(&shop_repository, &fx_rate);
    let upsert_service =
        UpsertProductsServiceImpl::new(&product_repository, sqs_client, &queue_url, &fx_rate);

    let shop = Faker.fake::<Shop>();
    let mut shop_records = ShopRecord::clone_from_shop_as_shop_domain_records(&shop);
    shop_records.push(ShopRecord::from_shop_as_shop_id_record(shop.clone()));
    let _ = shop_repository
        .put_shop_records_transact(shop_records)
        .await
        .unwrap();

    let mut products = fake::vec![PutProductData; 235];
    for product in &mut products {
        let shop_host = shop.domains.iter().next().unwrap().as_str();
        product.url.set_host(Some(shop_host)).unwrap();
    }
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::PUT)
            .body_serde(&PutCollectionData {
                items: products.clone(),
            })
            .build(),
        context: Default::default(),
    };
    let response = handler(lambda_event, &upsert_service, &enrichment_service)
        .await
        .unwrap();

    let actual_json = extract_apigw_response_json_body!(response);
    let actual = serde_json::from_value::<PutProductsResponse>(actual_json).unwrap();

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

#[localstack_test(services = [DynamoDB(), INGEST_QUEUE])]
async fn should_put_products_with_known_domain_when_domain_contains_subdomain_www() {
    let dynamodb_client = get_dynamodb_client().await;
    let sqs_client = get_sqs_client().await;
    let shop_repository = ShopDynamoDbRepositoryImpl::new(dynamodb_client, "table_1");
    let product_repository = ProductDynamoDbRepositoryImpl::new(dynamodb_client, "table_1");
    let fx_rate = FixedFxRate();
    let queue_url = INGEST_QUEUE.queue_url();
    let enrichment_service = ProductCommandEnrichmentServiceImpl::new(&shop_repository, &fx_rate);
    let upsert_service =
        UpsertProductsServiceImpl::new(&product_repository, sqs_client, &queue_url, &fx_rate);

    let shop = Shop {
        shop_id: Faker.fake(),
        name: Faker.fake(),
        domains: HashSet::from_iter([
            Domain::try_from("https://www.antiquitaeten-tuebingen.de").unwrap()
        ]),
        image: Faker.fake(),
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
    };
    let mut shop_records = ShopRecord::clone_from_shop_as_shop_domain_records(&shop);
    shop_records.push(ShopRecord::from_shop_as_shop_id_record(shop.clone()));

    let _ = shop_repository
        .put_shop_records_transact(shop_records)
        .await
        .unwrap();

    let mut products = fake::vec![PutProductData; 235];
    for product in &mut products {
        let shop_host = shop.domains.iter().next().unwrap().as_str();
        product
            .url
            .set_host(Some(&format!("www.{shop_host}")))
            .unwrap();
    }
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::PUT)
            .body_serde(&PutCollectionData {
                items: products.clone(),
            })
            .build(),
        context: Default::default(),
    };
    let response = handler(lambda_event, &upsert_service, &enrichment_service)
        .await
        .unwrap();

    let actual_json = extract_apigw_response_json_body!(response);
    let actual = serde_json::from_value::<PutProductsResponse>(actual_json).unwrap();

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
