use common::personalized::api::PersonalizedData;
use fake::Fake;
use fake::Faker;
use lambda_runtime::LambdaEvent;
use product::data::get_data::GetProductData;
use product::data::user_state_data::ProductUserStateData;
use product::dynamodb::product_record::ProductRecord;
use product::dynamodb::repository::ProductDynamoDbRepository;
use product::dynamodb::repository::ProductDynamoDbRepositoryImpl;
use product::service::get_service::GetProductServiceImpl;
use product_watchlist::dynamodb::record::WatchlistProductRecord;
use product_watchlist::dynamodb::repository::WatchlistProductDynamoDbRepository;
use product_watchlist::dynamodb::repository::WatchlistProductDynamoDbRepositoryImpl;
use product_watchlist::service::product_watchlist_service::ProductWatchListServiceImpl;
use product_watchlist_api::watchlist_patch::WatchlistProductPatch;
use product_watchlist_api::watchlist_patch::handle;
use test_api::*;
use time::OffsetDateTime;
use user::dynamodb::repository::UserDynamoDbRepository;
use user::dynamodb::repository::UserDynamoDbRepositoryImpl;
use user::dynamodb::user_record::UserRecord;

#[rstest::rstest]
#[test_attr(apply(test))]
#[case(false, false)]
#[case(false, true)]
#[case(true, false)]
#[case(true, true)]
#[trace]
#[localstack_test(services = [DynamoDB()])]
async fn should_respond_with_patched_notifications(
    #[case] old_notifications: bool,
    #[case] new_notifications: bool,
) {
    use product_watchlist::dynamodb::record::{mk_gsi1_pk, mk_gsi1_sk};

    let user_repository = UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let watchlist_repository =
        WatchlistProductDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let product_repository =
        ProductDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let get_product_service = GetProductServiceImpl::new(&product_repository);
    let service = ProductWatchListServiceImpl::new(&watchlist_repository, &get_product_service);

    let user_record = Faker.fake::<UserRecord>();
    let _ = user_repository
        .put_user_record(user_record.clone())
        .await
        .unwrap();

    let product_record = Faker.fake::<ProductRecord>();
    let put_product_res = product_repository
        .put_product_records([product_record.clone()].into())
        .await
        .unwrap();
    assert!(
        put_product_res
            .unprocessed_items
            .unwrap_or_default()
            .is_empty()
    );

    let created = OffsetDateTime::now_utc();
    let watchlist_record = WatchlistProductRecord {
        pk: product_watchlist::dynamodb::record::mk_pk(&user_record.user_id),
        sk: product_watchlist::dynamodb::record::mk_sk(
            &product_record.shop_id,
            &product_record.shops_product_id,
        ),
        lsi1_sk: product_watchlist::dynamodb::record::mk_lsi1_sk(&created).unwrap(),
        gsi1_pk: mk_gsi1_pk(&product_record.product_id),
        gsi1_sk: mk_gsi1_sk(&user_record.user_id),
        user_id: user_record.user_id,
        product_id: product_record.product_id,
        shop_id: product_record.shop_id,
        shops_product_id: product_record.shops_product_id.clone(),
        notifications: old_notifications,
        created,
        updated: created,
    };
    let _ = watchlist_repository
        .put_watchlist_record(watchlist_record)
        .await
        .unwrap();

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::PATCH)
            .path_parameter("shopId", product_record.shop_id)
            .path_parameter("shopsProductId", product_record.shops_product_id.clone())
            .query_string_parameter("language", "de")
            .query_string_parameter("currency", "EUR")
            .body_serde(&WatchlistProductPatch {
                notifications: Some(new_notifications),
            })
            .jwt_claim("sub", user_record.user_id)
            .build(),
        context: Default::default(),
    };

    let response = handle(lambda_event, &service).await.unwrap();
    assert_eq!(200, response.status_code);

    let patched: PersonalizedData<GetProductData, ProductUserStateData> =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert_eq!(
        new_notifications,
        patched.user_state.unwrap().watchlist.notifications
    );

    let updated_watchlist_product_record = watchlist_repository
        .get_watchlist_record(
            &user_record.user_id,
            &product_record.shop_id,
            &product_record.shops_product_id,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        new_notifications,
        updated_watchlist_product_record.notifications
    );
}
