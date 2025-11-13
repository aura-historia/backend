use common::{pagination::cursor::api::TimeCursoredData, user_id::UserId};
use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use product::dynamodb::{
    product_record::ProductRecord,
    repository::{ProductDynamoDbRepository, ProductDynamoDbRepositoryImpl},
};
use product::service::get_service::GetProductServiceImpl;
use product::watchlist::{
    dynamodb::record::{WatchlistProductRecord, mk_gsi1_pk, mk_gsi1_sk, mk_lsi1_sk, mk_pk, mk_sk},
    dynamodb::repository::{
        WatchlistProductDynamoDbRepository, WatchlistProductDynamoDbRepositoryImpl,
    },
    service::product_watchlist_service::ProductWatchListServiceImpl,
};
use product_api_watchlist_get::{WatchlistProductDataView, handler};
use test_api::*;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use user::dynamodb::repository::UserDynamoDbRepositoryImpl;

#[localstack_test(services = [DynamoDB()])]
async fn should_200_when_sort_created_asc() {
    let client = get_dynamodb_client().await;
    let user_repository = UserDynamoDbRepositoryImpl::new(client, "table_1");
    let product_repository = ProductDynamoDbRepositoryImpl::new(client, "table_1");
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(client, "table_1");
    let get_product_service = GetProductServiceImpl::new(&product_repository);
    let service = ProductWatchListServiceImpl::new(
        &watchlist_repository,
        &user_repository,
        &product_repository,
        &get_product_service,
    );

    let product_records = fake::vec![ProductRecord; 23];
    let put_res = product_repository
        .put_product_records(product_records.clone().try_into().unwrap())
        .await
        .unwrap();
    assert!(put_res.unprocessed_items.unwrap_or_default().is_empty());

    let user_id = UserId::new();
    for product_record in product_records.clone() {
        let created = OffsetDateTime::now_utc();
        let watchlist_record = WatchlistProductRecord {
            pk: mk_pk(&user_id),
            sk: mk_sk(&product_record.shop_id, &product_record.shops_product_id),
            lsi1_sk: mk_lsi1_sk(&created).unwrap(),
            gsi1_pk: None,
            gsi1_sk: None,
            user_id,
            product_id: product_record.product_id,
            shop_id: product_record.shop_id,
            shops_product_id: product_record.shops_product_id,
            notifications: false,
            user_record: Faker.fake(),
            created,
            updated: created,
        };
        watchlist_repository
            .put_watchlist_record(watchlist_record)
            .await
            .unwrap();
    }

    let expected = product_records
        .into_iter()
        .take(10)
        .map(|record| record.product_id)
        .collect::<Vec<_>>();
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .jwt_claim("sub", user_id)
            .header("accept-language", "de")
            .query_string_parameter("currency", "EUR")
            .query_string_parameter("sort", "created")
            .query_string_parameter("order", "asc")
            .query_string_parameter("searchAfter", "2021-12-31T23:59:59Z")
            .query_string_parameter("size", "10")
            .build(),
        context: Default::default(),
    };

    let response = handler(lambda_event, &service).await.unwrap();
    assert_eq!(200, response.status_code);

    let actual: TimeCursoredData<WatchlistProductDataView> =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert_eq!(10, actual.size);
    assert_eq!(10, actual.items.len());
    assert_eq!(
        expected,
        actual
            .items
            .into_iter()
            .map(|item| item.product.product_id)
            .collect::<Vec<_>>()
    );
    assert_eq!(23, actual.total.unwrap());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_200_when_sort_created_asc_search_after() {
    let client = get_dynamodb_client().await;
    let user_repository = UserDynamoDbRepositoryImpl::new(client, "table_1");
    let product_repository = ProductDynamoDbRepositoryImpl::new(client, "table_1");
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(client, "table_1");
    let get_product_service = GetProductServiceImpl::new(&product_repository);
    let service = ProductWatchListServiceImpl::new(
        &watchlist_repository,
        &user_repository,
        &product_repository,
        &get_product_service,
    );

    let product_records = fake::vec![ProductRecord; 23];
    let put_res = product_repository
        .put_product_records(product_records.clone().try_into().unwrap())
        .await
        .unwrap();
    assert!(put_res.unprocessed_items.unwrap_or_default().is_empty());

    let user_id = UserId::new();
    let mut from = None;
    let mut expected_next_after = None;
    for (i, product_record) in product_records.iter().cloned().enumerate() {
        let created = OffsetDateTime::now_utc();
        if i == 7 {
            from = Some(created);
        }
        if i == 19 {
            expected_next_after = Some(created);
        }
        let watchlist_record = WatchlistProductRecord {
            pk: mk_pk(&user_id),
            sk: mk_sk(&product_record.shop_id, &product_record.shops_product_id),
            lsi1_sk: mk_lsi1_sk(&created).unwrap(),
            user_id,
            gsi1_pk: None,
            gsi1_sk: None,
            product_id: product_record.product_id,
            shop_id: product_record.shop_id,
            shops_product_id: product_record.shops_product_id,
            notifications: false,
            user_record: Faker.fake(),
            created,
            updated: created,
        };
        watchlist_repository
            .put_watchlist_record(watchlist_record)
            .await
            .unwrap();
    }

    let expected = product_records
        .iter()
        .skip(8)
        .take(12)
        .map(|record| record.product_id)
        .collect::<Vec<_>>();
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .jwt_claim("sub", user_id)
            .header("accept-language", "de")
            .query_string_parameter("currency", "EUR")
            .query_string_parameter("sort", "created")
            .query_string_parameter("order", "asc")
            .query_string_parameter("searchAfter", from.unwrap().format(&Rfc3339).unwrap())
            .query_string_parameter("size", "12")
            .build(),
        context: Default::default(),
    };

    let response = handler(lambda_event, &service).await.unwrap();
    assert_eq!(200, response.status_code);

    let actual: TimeCursoredData<WatchlistProductDataView> =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert_eq!(12, actual.size);
    assert_eq!(12, actual.items.len());
    assert_eq!(
        expected,
        actual
            .items
            .into_iter()
            .map(|item| item.product.product_id)
            .collect::<Vec<_>>()
    );
    assert_eq!(expected_next_after.unwrap(), actual.search_after.unwrap());
    assert_eq!(15, actual.total.unwrap());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_200_when_sort_created_desc() {
    let client = get_dynamodb_client().await;
    let user_repository = UserDynamoDbRepositoryImpl::new(client, "table_1");
    let product_repository = ProductDynamoDbRepositoryImpl::new(client, "table_1");
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(client, "table_1");
    let get_product_service = GetProductServiceImpl::new(&product_repository);
    let service = ProductWatchListServiceImpl::new(
        &watchlist_repository,
        &user_repository,
        &product_repository,
        &get_product_service,
    );

    let product_records = fake::vec![ProductRecord; 23];
    let put_res = product_repository
        .put_product_records(product_records.clone().try_into().unwrap())
        .await
        .unwrap();
    assert!(put_res.unprocessed_items.unwrap_or_default().is_empty());

    let user_id = UserId::new();
    for product_record in product_records.clone() {
        let created = OffsetDateTime::now_utc();
        let watchlist_record = WatchlistProductRecord {
            pk: mk_pk(&user_id),
            sk: mk_sk(&product_record.shop_id, &product_record.shops_product_id),
            lsi1_sk: mk_lsi1_sk(&created).unwrap(),
            user_id,
            gsi1_pk: None,
            gsi1_sk: None,
            product_id: product_record.product_id,
            shop_id: product_record.shop_id,
            shops_product_id: product_record.shops_product_id,
            notifications: false,
            user_record: Faker.fake(),
            created,
            updated: created,
        };
        watchlist_repository
            .put_watchlist_record(watchlist_record)
            .await
            .unwrap();
    }

    let expected = product_records
        .into_iter()
        .skip(16)
        .rev()
        .map(|record| record.product_id)
        .collect::<Vec<_>>();
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .jwt_claim("sub", user_id)
            .header("accept-language", "de")
            .query_string_parameter("currency", "EUR")
            .query_string_parameter("sort", "created")
            .query_string_parameter("order", "desc")
            .query_string_parameter("searchAfter", "2999-12-31T23:59:59Z")
            .query_string_parameter("size", "7")
            .build(),
        context: Default::default(),
    };

    let response = handler(lambda_event, &service).await.unwrap();
    assert_eq!(200, response.status_code);

    let actual: TimeCursoredData<WatchlistProductDataView> =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert_eq!(7, actual.size);
    assert_eq!(7, actual.items.len());
    assert_eq!(
        expected,
        actual
            .items
            .into_iter()
            .map(|item| item.product.product_id)
            .collect::<Vec<_>>()
    );
    assert_eq!(23, actual.total.unwrap());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_200_when_sort_created_desc_search_after() {
    let client = get_dynamodb_client().await;
    let user_repository = UserDynamoDbRepositoryImpl::new(client, "table_1");
    let product_repository = ProductDynamoDbRepositoryImpl::new(client, "table_1");
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(client, "table_1");
    let get_product_service = GetProductServiceImpl::new(&product_repository);
    let service = ProductWatchListServiceImpl::new(
        &watchlist_repository,
        &user_repository,
        &product_repository,
        &get_product_service,
    );

    let product_records = fake::vec![ProductRecord; 23];
    let put_res = product_repository
        .put_product_records(product_records.clone().try_into().unwrap())
        .await
        .unwrap();
    assert!(put_res.unprocessed_items.unwrap_or_default().is_empty());

    let user_id = UserId::new();
    let mut from = None;
    let mut expected_next_after = None;
    for (i, product_record) in product_records.iter().cloned().enumerate() {
        let created = OffsetDateTime::now_utc();
        if i == 7 {
            from = Some(created);
        }
        if i == 0 {
            expected_next_after = Some(created);
        }
        let watchlist_record = WatchlistProductRecord {
            pk: mk_pk(&user_id),
            sk: mk_sk(&product_record.shop_id, &product_record.shops_product_id),
            lsi1_sk: mk_lsi1_sk(&created).unwrap(),
            user_id,
            gsi1_pk: Some(mk_gsi1_pk(&product_record.product_id)),
            gsi1_sk: Some(mk_gsi1_sk(&user_id)),
            product_id: product_record.product_id,
            shop_id: product_record.shop_id,
            shops_product_id: product_record.shops_product_id,
            notifications: true,
            user_record: Faker.fake(),
            created,
            updated: created,
        };
        watchlist_repository
            .put_watchlist_record(watchlist_record)
            .await
            .unwrap();
    }

    let expected = product_records
        .iter()
        .take(7)
        .rev()
        .map(|record| record.product_id)
        .collect::<Vec<_>>();
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .jwt_claim("sub", user_id)
            .header("accept-language", "de")
            .query_string_parameter("currency", "EUR")
            .query_string_parameter("sort", "created")
            .query_string_parameter("order", "desc")
            .query_string_parameter("searchAfter", from.unwrap().format(&Rfc3339).unwrap())
            .query_string_parameter("size", "20")
            .build(),
        context: Default::default(),
    };

    let response = handler(lambda_event, &service).await.unwrap();
    assert_eq!(200, response.status_code);

    let actual: TimeCursoredData<WatchlistProductDataView> =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert_eq!(7, actual.size);
    assert_eq!(7, actual.items.len());
    assert_eq!(
        expected,
        actual
            .items
            .into_iter()
            .map(|item| item.product.product_id)
            .collect::<Vec<_>>()
    );
    assert_eq!(expected_next_after.unwrap(), actual.search_after.unwrap());
    assert_eq!(7, actual.total.unwrap());
}
