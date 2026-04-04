use common::product_id::api::ProductKeyData;
use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use notification::dynamodb::repository::NotificationDynamoDbRepositoryImpl;
use notification::service::noop_adapters::{NoopS3Adapter, NoopSesAdapter};
use notification::service::notification_service::NotificationServiceImpl;
use product::dynamodb::{
    product_record::ProductRecord,
    repository::{ProductDynamoDbRepository, ProductDynamoDbRepositoryImpl},
};
use product::service::get_service::GetProductServiceImpl;
use product_personalization::service::ProductPersonalizationServiceImpl;
use product_watchlist::dynamodb::record::{mk_gsi1_pk, mk_gsi1_sk};
use product_watchlist::{
    dynamodb::record::{WatchlistProductRecord, mk_lsi1_sk, mk_pk, mk_sk},
    dynamodb::repository::{
        WatchlistProductDynamoDbRepository, WatchlistProductDynamoDbRepositoryImpl,
    },
    service::product_watchlist_service::ProductWatchListServiceImpl,
};
use product_watchlist_api::watchlist_post::handle;
use test_api::*;
use time::OffsetDateTime;
use user::dynamodb::repository::{UserDynamoDbRepository, UserDynamoDbRepositoryImpl};
use user::dynamodb::user_record::UserRecord;
use user::service::user_service::UserServiceImpl;

#[localstack_test(services = [DynamoDB()])]
async fn should_201_when_new_watchlist_entry_would_not_exceed_quota() {
    let client = get_dynamodb_client().await;
    let user_record = Faker.fake::<UserRecord>();

    let product_repository = ProductDynamoDbRepositoryImpl::new(client, "table_1");
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(client, "table_1");
    let notification_repository = NotificationDynamoDbRepositoryImpl::new(client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(client, "table_1");
    user_repository
        .put_user_record(user_record.clone())
        .await
        .unwrap();
    let get_product_service = GetProductServiceImpl::new(&product_repository);
    let noop_ses = NoopSesAdapter;
    let noop_s3 = NoopS3Adapter;
    let user_service = UserServiceImpl::new(&user_repository);
    let notification_service = NotificationServiceImpl::new(
        &notification_repository,
        &user_service,
        &noop_ses,
        &noop_s3,
        "",
        "",
        "",
    );
    let personalization_service = ProductPersonalizationServiceImpl::new(
        &watchlist_repository,
        &notification_service,
        &user_service,
    );
    let service =
        ProductWatchListServiceImpl::new(&watchlist_repository, &product_repository, &user_service);

    let product_records = fake::vec![ProductRecord; (user::core::tier::UserTier::Free.watchlist_limit() - 1) as usize];
    let put_res = product_repository
        .put_product_records(product_records.clone().try_into().unwrap())
        .await
        .unwrap();
    assert!(put_res.unprocessed_items.unwrap_or_default().is_empty());

    let new_product_record = Faker.fake::<ProductRecord>();
    let put_overflowing_res = product_repository
        .put_product_records(vec![new_product_record.clone()].try_into().unwrap())
        .await
        .unwrap();
    assert!(
        put_overflowing_res
            .unprocessed_items
            .unwrap_or_default()
            .is_empty()
    );

    let user_id = user_record.user_id;
    for product_record in product_records {
        let created = OffsetDateTime::now_utc();
        let watchlist_record = WatchlistProductRecord {
            pk: mk_pk(&user_id),
            sk: mk_sk(&product_record.shop_id, &product_record.shops_product_id),
            lsi1_sk: mk_lsi1_sk(&created),
            user_id,
            gsi1_pk: mk_gsi1_pk(&product_record.product_id),
            gsi1_sk: mk_gsi1_sk(&user_id),
            product_id: product_record.product_id,
            shop_id: product_record.shop_id,
            shops_product_id: product_record.shops_product_id,
            notifications: false,
            created,
            updated: created,
        };
        watchlist_repository
            .put_watchlist_record(watchlist_record)
            .await
            .unwrap();
    }

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .body_serde(&ProductKeyData {
                shop_id: new_product_record.shop_id,
                shops_product_id: new_product_record.shops_product_id.clone(),
            })
            .jwt_claim("sub", user_id)
            .query_string_parameter("language", "de")
            .query_string_parameter("currency", "EUR")
            .build(),
        context: Default::default(),
    };

    let response = handle(
        lambda_event,
        &service,
        &get_product_service,
        &personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(201, response.status_code);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_422_when_new_watchlist_entry_would_exceed_quota() {
    let client = get_dynamodb_client().await;
    let user_record = Faker.fake::<UserRecord>();

    let product_repository = ProductDynamoDbRepositoryImpl::new(client, "table_1");
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(client, "table_1");
    let notification_repository = NotificationDynamoDbRepositoryImpl::new(client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(client, "table_1");
    user_repository
        .put_user_record(user_record.clone())
        .await
        .unwrap();
    let get_product_service = GetProductServiceImpl::new(&product_repository);
    let noop_ses = NoopSesAdapter;
    let noop_s3 = NoopS3Adapter;
    let user_service = UserServiceImpl::new(&user_repository);
    let notification_service = NotificationServiceImpl::new(
        &notification_repository,
        &user_service,
        &noop_ses,
        &noop_s3,
        "",
        "",
        "",
    );
    let personalization_service = ProductPersonalizationServiceImpl::new(
        &watchlist_repository,
        &notification_service,
        &user_service,
    );
    let service =
        ProductWatchListServiceImpl::new(&watchlist_repository, &product_repository, &user_service);

    let product_records =
        fake::vec![ProductRecord; user::core::tier::UserTier::Free.watchlist_limit() as usize];
    let put_res = product_repository
        .put_product_records(product_records.clone().try_into().unwrap())
        .await
        .unwrap();
    assert!(put_res.unprocessed_items.unwrap_or_default().is_empty());

    let overflowing_product_record = Faker.fake::<ProductRecord>();
    let put_overflowing_res = product_repository
        .put_product_records(vec![overflowing_product_record.clone()].try_into().unwrap())
        .await
        .unwrap();
    assert!(
        put_overflowing_res
            .unprocessed_items
            .unwrap_or_default()
            .is_empty()
    );

    let user_id = user_record.user_id;
    for product_record in product_records {
        let created = OffsetDateTime::now_utc();
        let watchlist_record = WatchlistProductRecord {
            pk: mk_pk(&user_id),
            sk: mk_sk(&product_record.shop_id, &product_record.shops_product_id),
            lsi1_sk: mk_lsi1_sk(&created),
            user_id,
            gsi1_pk: mk_gsi1_pk(&product_record.product_id),
            gsi1_sk: mk_gsi1_sk(&user_id),
            product_id: product_record.product_id,
            shop_id: product_record.shop_id,
            shops_product_id: product_record.shops_product_id,
            notifications: false,
            created,
            updated: created,
        };
        watchlist_repository
            .put_watchlist_record(watchlist_record)
            .await
            .unwrap();
    }

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .body_serde(&ProductKeyData {
                shop_id: overflowing_product_record.shop_id,
                shops_product_id: overflowing_product_record.shops_product_id.clone(),
            })
            .jwt_claim("sub", user_id)
            .query_string_parameter("language", "de")
            .query_string_parameter("currency", "EUR")
            .build(),
        context: Default::default(),
    };

    let actual = handle(
        lambda_event,
        &service,
        &get_product_service,
        &personalization_service,
    )
    .await
    .unwrap_err();
    assert_eq!(422, actual.status);
}
