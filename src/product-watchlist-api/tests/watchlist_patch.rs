use common::personalized::api::PersonalizedData;
use fake::Fake;
use fake::Faker;
use lambda_runtime::LambdaEvent;
use notification::dynamodb::repository::NotificationDynamoDbRepositoryImpl;
use notification::service::noop_adapters::{NoopS3Adapter, NoopSesAdapter};
use notification::service::notification_service::NotificationServiceImpl;
use product::data::get_data::GetProductData;
use product::data::user_state_data::ProductUserStateData;
use product::dynamodb::product_record::ProductRecord;
use product::dynamodb::repository::ProductDynamoDbRepository;
use product::dynamodb::repository::ProductDynamoDbRepositoryImpl;
use product::service::get_service::GetProductServiceImpl;
use product_personalization::service::ProductPersonalizationServiceImpl;
use product_watchlist::dynamodb::record::WatchlistProductRecord;
use product_watchlist::dynamodb::repository::WatchlistProductDynamoDbRepository;
use product_watchlist::dynamodb::repository::WatchlistProductDynamoDbRepositoryImpl;
use product_watchlist::service::product_watchlist_service::ProductWatchListServiceImpl;
use product_watchlist_api::watchlist_patch::WatchlistProductPatch;
use product_watchlist_api::watchlist_patch::handle;
use search_filter::dynamodb::repository::MockUserSearchFilterDynamoDbRepository;
use test_api::*;
use time::OffsetDateTime;
use user::dynamodb::repository::UserDynamoDbRepository;
use user::dynamodb::repository::UserDynamoDbRepositoryImpl;
use user::dynamodb::user_record::UserRecord;
use user::service::user_service::UserServiceImpl;

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
    let notification_repository =
        NotificationDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
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
    let mut search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
    search_filter_repository
        .expect_query_user_search_filter_match_records_for_product()
        .returning(|_, _, _| Box::pin(async { Ok(vec![]) }));
    let personalization_service = ProductPersonalizationServiceImpl::new(
        &watchlist_repository,
        &notification_service,
        &user_service,
        &search_filter_repository,
    );
    let service =
        ProductWatchListServiceImpl::new(&watchlist_repository, &product_repository, &user_service);

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
        lsi1_sk: product_watchlist::dynamodb::record::mk_lsi1_sk(&created),
        gsi1_pk: mk_gsi1_pk(&product_record.product_id),
        gsi1_sk: mk_gsi1_sk(&user_record.user_id),
        user_id: user_record.user_id,
        product_id: product_record.product_id,
        shop_id: product_record.shop_id,
        shops_product_id: product_record.shops_product_id.clone(),
        notifications: old_notifications,
        state: common::resource_state::record::ResourceStateRecord::Active,
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
                state: None,
            })
            .jwt_claim("sub", user_record.user_id)
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
    assert_eq!(200, response.status_code);

    let patched: PersonalizedData<GetProductData, ProductUserStateData> =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert_eq!(
        new_notifications,
        patched.user_state.clone().unwrap().watchlist.notifications
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
