use common::{pagination::cursor::api::TimeCursoredData, user_id::UserId};
use item_api_watchlist_get::{WatchlistItemDataView, handler};
use item_dynamodb::{
    item_record::ItemRecord,
    repository::{ItemDynamoDbRepository, ItemDynamoDbRepositoryImpl},
};
use item_service::get_service::GetItemServiceImpl;
use item_watchlist::{
    record::{WatchlistItemRecord, mk_lsi1_sk, mk_pk, mk_sk},
    repository::{WatchlistItemDynamoDbRepository, WatchlistItemDynamoDbRepositoryImpl},
    service::ItemWatchListServiceImpl,
};
use lambda_runtime::LambdaEvent;
use test_api::*;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

#[localstack_test(services = [DynamoDB()])]
async fn should_200_when_sort_created_asc() {
    let client = get_dynamodb_client().await;
    let item_repository = ItemDynamoDbRepositoryImpl::new(client, "table_1");
    let watchlist_repository = WatchlistItemDynamoDbRepositoryImpl::new(client, "table_1");
    let get_item_service = GetItemServiceImpl::new(&item_repository);
    let service =
        ItemWatchListServiceImpl::new(&watchlist_repository, &item_repository, &get_item_service);

    let item_records = fake::vec![ItemRecord; 23];
    let put_res = item_repository
        .put_item_records(item_records.clone().try_into().unwrap())
        .await
        .unwrap();
    assert!(put_res.unprocessed_items.unwrap_or_default().is_empty());

    let user_id = UserId::new();
    for item_record in item_records.clone() {
        let created = OffsetDateTime::now_utc();
        let watchlist_record = WatchlistItemRecord {
            pk: mk_pk(&user_id),
            sk: mk_sk(&item_record.shop_id, &item_record.shops_item_id),
            lsi1_sk: mk_lsi1_sk(&created).unwrap(),
            user_id,
            item_id: item_record.item_id,
            shop_id: item_record.shop_id,
            shops_item_id: item_record.shops_item_id,
            notifications: false,
            created,
            updated: created,
        };
        watchlist_repository
            .put_watchlist_record(watchlist_record)
            .await
            .unwrap();
    }

    let expected = item_records
        .into_iter()
        .take(10)
        .map(|record| record.item_id)
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

    let actual: TimeCursoredData<WatchlistItemDataView> =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert_eq!(10, actual.size);
    assert_eq!(10, actual.items.len());
    assert_eq!(
        expected,
        actual
            .items
            .into_iter()
            .map(|item| item.item.item_id)
            .collect::<Vec<_>>()
    );
}

#[localstack_test(services = [DynamoDB()])]
async fn should_200_when_sort_created_asc_search_after() {
    let client = get_dynamodb_client().await;
    let item_repository = ItemDynamoDbRepositoryImpl::new(client, "table_1");
    let watchlist_repository = WatchlistItemDynamoDbRepositoryImpl::new(client, "table_1");
    let get_item_service = GetItemServiceImpl::new(&item_repository);
    let service =
        ItemWatchListServiceImpl::new(&watchlist_repository, &item_repository, &get_item_service);

    let item_records = fake::vec![ItemRecord; 23];
    let put_res = item_repository
        .put_item_records(item_records.clone().try_into().unwrap())
        .await
        .unwrap();
    assert!(put_res.unprocessed_items.unwrap_or_default().is_empty());

    let user_id = UserId::new();
    let mut from = None;
    let mut expected_next_after = None;
    for (i, item_record) in item_records.iter().cloned().enumerate() {
        let created = OffsetDateTime::now_utc();
        if i == 7 {
            from = Some(created);
        }
        if i == 19 {
            expected_next_after = Some(created);
        }
        let watchlist_record = WatchlistItemRecord {
            pk: mk_pk(&user_id),
            sk: mk_sk(&item_record.shop_id, &item_record.shops_item_id),
            lsi1_sk: mk_lsi1_sk(&created).unwrap(),
            user_id,
            item_id: item_record.item_id,
            shop_id: item_record.shop_id,
            shops_item_id: item_record.shops_item_id,
            notifications: false,
            created,
            updated: created,
        };
        watchlist_repository
            .put_watchlist_record(watchlist_record)
            .await
            .unwrap();
    }

    let expected = item_records
        .iter()
        .skip(8)
        .take(12)
        .map(|record| record.item_id)
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

    let actual: TimeCursoredData<WatchlistItemDataView> =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert_eq!(12, actual.size);
    assert_eq!(12, actual.items.len());
    assert_eq!(
        expected,
        actual
            .items
            .into_iter()
            .map(|item| item.item.item_id)
            .collect::<Vec<_>>()
    );
    assert_eq!(expected_next_after.unwrap(), actual.search_after.unwrap());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_200_when_sort_created_desc() {
    let client = get_dynamodb_client().await;
    let item_repository = ItemDynamoDbRepositoryImpl::new(client, "table_1");
    let watchlist_repository = WatchlistItemDynamoDbRepositoryImpl::new(client, "table_1");
    let get_item_service = GetItemServiceImpl::new(&item_repository);
    let service =
        ItemWatchListServiceImpl::new(&watchlist_repository, &item_repository, &get_item_service);

    let item_records = fake::vec![ItemRecord; 23];
    let put_res = item_repository
        .put_item_records(item_records.clone().try_into().unwrap())
        .await
        .unwrap();
    assert!(put_res.unprocessed_items.unwrap_or_default().is_empty());

    let user_id = UserId::new();
    for item_record in item_records.clone() {
        let created = OffsetDateTime::now_utc();
        let watchlist_record = WatchlistItemRecord {
            pk: mk_pk(&user_id),
            sk: mk_sk(&item_record.shop_id, &item_record.shops_item_id),
            lsi1_sk: mk_lsi1_sk(&created).unwrap(),
            user_id,
            item_id: item_record.item_id,
            shop_id: item_record.shop_id,
            shops_item_id: item_record.shops_item_id,
            notifications: false,
            created,
            updated: created,
        };
        watchlist_repository
            .put_watchlist_record(watchlist_record)
            .await
            .unwrap();
    }

    let expected = item_records
        .into_iter()
        .skip(16)
        .rev()
        .map(|record| record.item_id)
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

    let actual: TimeCursoredData<WatchlistItemDataView> =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert_eq!(7, actual.size);
    assert_eq!(7, actual.items.len());
    assert_eq!(
        expected,
        actual
            .items
            .into_iter()
            .map(|item| item.item.item_id)
            .collect::<Vec<_>>()
    );
}

#[localstack_test(services = [DynamoDB()])]
async fn should_200_when_sort_created_desc_search_after() {
    let client = get_dynamodb_client().await;
    let item_repository = ItemDynamoDbRepositoryImpl::new(client, "table_1");
    let watchlist_repository = WatchlistItemDynamoDbRepositoryImpl::new(client, "table_1");
    let get_item_service = GetItemServiceImpl::new(&item_repository);
    let service =
        ItemWatchListServiceImpl::new(&watchlist_repository, &item_repository, &get_item_service);

    let item_records = fake::vec![ItemRecord; 23];
    let put_res = item_repository
        .put_item_records(item_records.clone().try_into().unwrap())
        .await
        .unwrap();
    assert!(put_res.unprocessed_items.unwrap_or_default().is_empty());

    let user_id = UserId::new();
    let mut from = None;
    let mut expected_next_after = None;
    for (i, item_record) in item_records.iter().cloned().enumerate() {
        let created = OffsetDateTime::now_utc();
        if i == 7 {
            from = Some(created);
        }
        if i == 0 {
            expected_next_after = Some(created);
        }
        let watchlist_record = WatchlistItemRecord {
            pk: mk_pk(&user_id),
            sk: mk_sk(&item_record.shop_id, &item_record.shops_item_id),
            lsi1_sk: mk_lsi1_sk(&created).unwrap(),
            user_id,
            item_id: item_record.item_id,
            shop_id: item_record.shop_id,
            shops_item_id: item_record.shops_item_id,
            notifications: false,
            created,
            updated: created,
        };
        watchlist_repository
            .put_watchlist_record(watchlist_record)
            .await
            .unwrap();
    }

    let expected = item_records
        .iter()
        .take(7)
        .rev()
        .map(|record| record.item_id)
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

    let actual: TimeCursoredData<WatchlistItemDataView> =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert_eq!(7, actual.size);
    assert_eq!(7, actual.items.len());
    assert_eq!(
        expected,
        actual
            .items
            .into_iter()
            .map(|item| item.item.item_id)
            .collect::<Vec<_>>()
    );
    assert_eq!(expected_next_after.unwrap(), actual.search_after.unwrap());
}
