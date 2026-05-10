use common::pagination::cursor::api::TimeCursoredData;
use common::personalized::api::PersonalizedData;
use common::user_id::UserId;
use common::user_search_filter_id::UserSearchFilterId;
use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use notification::dynamodb::repository::NotificationDynamoDbRepositoryImpl;
use notification::service::noop_adapters::{NoopS3Adapter, NoopSesAdapter};
use notification::service::notification_service::NotificationServiceImpl;
use product::data::get_data::GetProductData;
use product::data::user_state_data::ProductUserStateData;
use product::dynamodb::{
    product_record::ProductRecord,
    repository::{ProductDynamoDbRepository, ProductDynamoDbRepositoryImpl},
};
use product::service::get_service::GetProductServiceImpl;
use product::service::query_service::MockQueryProductService;
use product_personalization::service::ProductPersonalizationServiceImpl;
use product_watchlist::dynamodb::repository::WatchlistProductDynamoDbRepositoryImpl;
use search_filter::dynamodb::repository::{
    UserSearchFilterDynamoDbRepository, UserSearchFilterDynamoDbRepositoryImpl,
};
use search_filter::dynamodb::user_search_filter_match_record::{
    UserSearchFilterMatchRecord, mk_lsi1_sk, mk_lsi2_sk, mk_pk, mk_sk,
};
use search_filter::service::user_search_filter_service::{
    UserSearchFilterService, UserSearchFilterServiceImpl,
};
use search_filter_api::handle;
use test_api::*;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use user::core::tier::UserTier;
use user::dynamodb::repository::UserDynamoDbRepositoryImpl;
use user::service::command::UpdateUserCommand;
use user::service::user_service::{UserService, UserServiceImpl};

fn setup_services(
    client: &'static aws_sdk_dynamodb::Client,
) -> (
    UserSearchFilterServiceImpl<'static>,
    GetProductServiceImpl<'static>,
    ProductPersonalizationServiceImpl<'static>,
) {
    let product_repository = Box::leak(Box::new(ProductDynamoDbRepositoryImpl::new(
        client, "table_1",
    )));
    let watchlist_repository = Box::leak(Box::new(WatchlistProductDynamoDbRepositoryImpl::new(
        client, "table_1",
    )));
    let notification_repository = Box::leak(Box::new(NotificationDynamoDbRepositoryImpl::new(
        client, "table_1",
    )));
    let user_repository = Box::leak(Box::new(UserDynamoDbRepositoryImpl::new(client, "table_1")));
    let search_filter_repository = Box::leak(Box::new(
        UserSearchFilterDynamoDbRepositoryImpl::new(client, "table_1"),
    ));
    let get_product_service = GetProductServiceImpl::new(product_repository);
    let noop_ses: &'static NoopSesAdapter = Box::leak(Box::new(NoopSesAdapter));
    let noop_s3: &'static NoopS3Adapter = Box::leak(Box::new(NoopS3Adapter));
    let user_service: &'static UserServiceImpl<'static> =
        Box::leak(Box::new(UserServiceImpl::new(user_repository)));
    let notification_service: &'static NotificationServiceImpl<'static> =
        Box::leak(Box::new(NotificationServiceImpl::new(
            notification_repository,
            user_service,
            noop_ses,
            noop_s3,
            "",
            "",
            "",
        )));
    let personalization_service = ProductPersonalizationServiceImpl::new(
        watchlist_repository,
        notification_service,
        user_service,
        search_filter_repository,
    );
    let service = UserSearchFilterServiceImpl::new(search_filter_repository, user_service);
    (service, get_product_service, personalization_service)
}

async fn create_user(client: &'static aws_sdk_dynamodb::Client) -> UserId {
    let user_repository = UserDynamoDbRepositoryImpl::new(client, "table_1");
    let user_service = user::service::user_service::UserServiceImpl::new(&user_repository);
    let user = user_service.create_user(Faker.fake()).await.unwrap();
    let update_cmd = UpdateUserCommand {
        tier: Some(UserTier::Ultimate),
        ..Default::default()
    };
    user_service
        .update_user(&user.user_id, update_cmd)
        .await
        .unwrap();
    user.user_id
}

async fn seed_match_records(
    client: &'static aws_sdk_dynamodb::Client,
    user_id: &UserId,
    search_filter_id: &UserSearchFilterId,
    product_records: &[ProductRecord],
) -> Vec<OffsetDateTime> {
    let repository = UserSearchFilterDynamoDbRepositoryImpl::new(client, "table_1");
    let mut timestamps = Vec::with_capacity(product_records.len());
    for product_record in product_records {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let created = OffsetDateTime::now_utc();
        timestamps.push(created);
        let record = UserSearchFilterMatchRecord {
            pk: mk_pk(user_id),
            sk: mk_sk(
                search_filter_id,
                &product_record.shop_id,
                &product_record.shops_product_id,
            ),
            lsi1_sk: mk_lsi1_sk(&created),
            lsi2_sk: Some(mk_lsi2_sk(
                &product_record.shop_id,
                &product_record.shops_product_id,
                &created,
            )),
            user_id: *user_id,
            user_search_filter_id: *search_filter_id,
            user_search_filter_name: None,
            shop_id: product_record.shop_id,
            shops_product_id: product_record.shops_product_id.clone(),
            product_id: product_record.product_id,
            origin_event_id: Faker.fake(),
            enhanced_match_reason: None,
            feedback: None,
            created,
            updated: created,
        };
        repository
            .put_user_search_filter_match_record(record)
            .await
            .unwrap();
    }
    timestamps
}

#[localstack_test(services = [DynamoDB()])]
async fn should_200_when_sort_created_asc() {
    let client = get_dynamodb_client().await;
    let (service, get_product_service, personalization_service) = setup_services(client);
    let product_repository = ProductDynamoDbRepositoryImpl::new(client, "table_1");

    let product_records = fake::vec![ProductRecord; 23];
    let put_res = product_repository
        .put_product_records(product_records.clone().try_into().unwrap())
        .await
        .unwrap();
    assert!(put_res.unprocessed_items.unwrap_or_default().is_empty());

    let user_id = create_user(client).await;
    let search_filter = service
        .create_user_search_filter(&user_id, Faker.fake(), Faker.fake(), None)
        .await
        .unwrap();

    let _timestamps = seed_match_records(
        client,
        &user_id,
        &search_filter.user_search_filter_id,
        &product_records,
    )
    .await;

    let expected = product_records
        .iter()
        .take(10)
        .map(|record| record.product_id)
        .collect::<Vec<_>>();
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .route_key("GET /api/v1/me/search-filters/{userSearchFilterId}/matches")
            .jwt_claim("sub", user_id)
            .path_parameter("userSearchFilterId", search_filter.user_search_filter_id)
            .query_string_parameter("language", "de")
            .query_string_parameter("currency", "EUR")
            .query_string_parameter("sort", "created")
            .query_string_parameter("order", "asc")
            .query_string_parameter("searchAfter", "2021-12-31T23:59:59Z")
            .query_string_parameter("size", "10")
            .build(),
        context: Default::default(),
    };

    let query_product_service = MockQueryProductService::default();
    let response = handle(
        lambda_event,
        &service,
        &get_product_service,
        &query_product_service,
        None,
        None,
        &personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response.status_code);

    let actual: TimeCursoredData<PersonalizedData<GetProductData, ProductUserStateData>> =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert_eq!(10, actual.size);
    assert_eq!(10, actual.items.len());
    assert_eq!(
        expected,
        actual
            .items
            .into_iter()
            .map(|item| item.item.product_id)
            .collect::<Vec<_>>()
    );
    assert_eq!(23, actual.total.unwrap());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_200_when_sort_created_asc_search_after() {
    let client = get_dynamodb_client().await;
    let (service, get_product_service, personalization_service) = setup_services(client);
    let product_repository = ProductDynamoDbRepositoryImpl::new(client, "table_1");

    let product_records = fake::vec![ProductRecord; 23];
    let put_res = product_repository
        .put_product_records(product_records.clone().try_into().unwrap())
        .await
        .unwrap();
    assert!(put_res.unprocessed_items.unwrap_or_default().is_empty());

    let user_id = create_user(client).await;
    let search_filter = service
        .create_user_search_filter(&user_id, Faker.fake(), Faker.fake(), None)
        .await
        .unwrap();

    let timestamps = seed_match_records(
        client,
        &user_id,
        &search_filter.user_search_filter_id,
        &product_records,
    )
    .await;
    let from = timestamps[7];
    let expected_next_after = timestamps[19];

    let expected = product_records
        .iter()
        .skip(8)
        .take(12)
        .map(|record| record.product_id)
        .collect::<Vec<_>>();
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .route_key("GET /api/v1/me/search-filters/{userSearchFilterId}/matches")
            .jwt_claim("sub", user_id)
            .path_parameter("userSearchFilterId", search_filter.user_search_filter_id)
            .query_string_parameter("language", "de")
            .query_string_parameter("currency", "EUR")
            .query_string_parameter("sort", "created")
            .query_string_parameter("order", "asc")
            .query_string_parameter("searchAfter", from.format(&Rfc3339).unwrap())
            .query_string_parameter("size", "12")
            .build(),
        context: Default::default(),
    };

    let query_product_service = MockQueryProductService::default();
    let response = handle(
        lambda_event,
        &service,
        &get_product_service,
        &query_product_service,
        None,
        None,
        &personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response.status_code);

    let actual: TimeCursoredData<PersonalizedData<GetProductData, ProductUserStateData>> =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert_eq!(12, actual.size);
    assert_eq!(12, actual.items.len());
    assert_eq!(
        expected,
        actual
            .items
            .into_iter()
            .map(|item| item.item.product_id)
            .collect::<Vec<_>>()
    );
    assert_eq!(expected_next_after, actual.search_after.unwrap());
    assert_eq!(15, actual.total.unwrap());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_200_when_sort_created_desc() {
    let client = get_dynamodb_client().await;
    let (service, get_product_service, personalization_service) = setup_services(client);
    let product_repository = ProductDynamoDbRepositoryImpl::new(client, "table_1");

    let product_records = fake::vec![ProductRecord; 23];
    let put_res = product_repository
        .put_product_records(product_records.clone().try_into().unwrap())
        .await
        .unwrap();
    assert!(put_res.unprocessed_items.unwrap_or_default().is_empty());

    let user_id = create_user(client).await;
    let search_filter = service
        .create_user_search_filter(&user_id, Faker.fake(), Faker.fake(), None)
        .await
        .unwrap();

    let _timestamps = seed_match_records(
        client,
        &user_id,
        &search_filter.user_search_filter_id,
        &product_records,
    )
    .await;

    let expected = product_records
        .iter()
        .skip(16)
        .rev()
        .map(|record| record.product_id)
        .collect::<Vec<_>>();
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .route_key("GET /api/v1/me/search-filters/{userSearchFilterId}/matches")
            .jwt_claim("sub", user_id)
            .path_parameter("userSearchFilterId", search_filter.user_search_filter_id)
            .query_string_parameter("language", "de")
            .query_string_parameter("currency", "EUR")
            .query_string_parameter("sort", "created")
            .query_string_parameter("order", "desc")
            .query_string_parameter("searchAfter", "2999-12-31T23:59:59Z")
            .query_string_parameter("size", "7")
            .build(),
        context: Default::default(),
    };

    let query_product_service = MockQueryProductService::default();
    let response = handle(
        lambda_event,
        &service,
        &get_product_service,
        &query_product_service,
        None,
        None,
        &personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response.status_code);

    let actual: TimeCursoredData<PersonalizedData<GetProductData, ProductUserStateData>> =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert_eq!(7, actual.size);
    assert_eq!(7, actual.items.len());
    assert_eq!(
        expected,
        actual
            .items
            .into_iter()
            .map(|item| item.item.product_id)
            .collect::<Vec<_>>()
    );
    assert_eq!(23, actual.total.unwrap());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_200_when_sort_created_desc_search_after() {
    let client = get_dynamodb_client().await;
    let (service, get_product_service, personalization_service) = setup_services(client);
    let product_repository = ProductDynamoDbRepositoryImpl::new(client, "table_1");

    let product_records = fake::vec![ProductRecord; 23];
    let put_res = product_repository
        .put_product_records(product_records.clone().try_into().unwrap())
        .await
        .unwrap();
    assert!(put_res.unprocessed_items.unwrap_or_default().is_empty());

    let user_id = create_user(client).await;
    let search_filter = service
        .create_user_search_filter(&user_id, Faker.fake(), Faker.fake(), None)
        .await
        .unwrap();

    let timestamps = seed_match_records(
        client,
        &user_id,
        &search_filter.user_search_filter_id,
        &product_records,
    )
    .await;
    let from = timestamps[7];
    let expected_next_after = timestamps[0];

    let expected = product_records
        .iter()
        .take(7)
        .rev()
        .map(|record| record.product_id)
        .collect::<Vec<_>>();
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .route_key("GET /api/v1/me/search-filters/{userSearchFilterId}/matches")
            .jwt_claim("sub", user_id)
            .path_parameter("userSearchFilterId", search_filter.user_search_filter_id)
            .query_string_parameter("language", "de")
            .query_string_parameter("currency", "EUR")
            .query_string_parameter("sort", "created")
            .query_string_parameter("order", "desc")
            .query_string_parameter("searchAfter", from.format(&Rfc3339).unwrap())
            .query_string_parameter("size", "20")
            .build(),
        context: Default::default(),
    };

    let query_product_service = MockQueryProductService::default();
    let response = handle(
        lambda_event,
        &service,
        &get_product_service,
        &query_product_service,
        None,
        None,
        &personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response.status_code);

    let actual: TimeCursoredData<PersonalizedData<GetProductData, ProductUserStateData>> =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert_eq!(7, actual.size);
    assert_eq!(7, actual.items.len());
    assert_eq!(
        expected,
        actual
            .items
            .into_iter()
            .map(|item| item.item.product_id)
            .collect::<Vec<_>>()
    );
    assert_eq!(expected_next_after, actual.search_after.unwrap());
    assert_eq!(7, actual.total.unwrap());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_200_empty_when_no_matches() {
    let client = get_dynamodb_client().await;
    let (service, get_product_service, personalization_service) = setup_services(client);

    let user_id = create_user(client).await;
    let search_filter = service
        .create_user_search_filter(&user_id, Faker.fake(), Faker.fake(), None)
        .await
        .unwrap();

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .route_key("GET /api/v1/me/search-filters/{userSearchFilterId}/matches")
            .jwt_claim("sub", user_id)
            .path_parameter("userSearchFilterId", search_filter.user_search_filter_id)
            .query_string_parameter("language", "de")
            .query_string_parameter("currency", "EUR")
            .query_string_parameter("sort", "created")
            .query_string_parameter("order", "asc")
            .query_string_parameter("searchAfter", "2021-12-31T23:59:59Z")
            .query_string_parameter("size", "10")
            .build(),
        context: Default::default(),
    };

    let query_product_service = MockQueryProductService::default();
    let response = handle(
        lambda_event,
        &service,
        &get_product_service,
        &query_product_service,
        None,
        None,
        &personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response.status_code);

    let actual: TimeCursoredData<PersonalizedData<GetProductData, ProductUserStateData>> =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert_eq!(0, actual.size);
    assert!(actual.items.is_empty());
    assert!(actual.search_after.is_none());
    assert_eq!(0, actual.total.unwrap());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_only_return_matches_for_specific_filter() {
    let client = get_dynamodb_client().await;
    let (service, get_product_service, personalization_service) = setup_services(client);
    let product_repository = ProductDynamoDbRepositoryImpl::new(client, "table_1");

    let product_records = fake::vec![ProductRecord; 10];
    let put_res = product_repository
        .put_product_records(product_records.clone().try_into().unwrap())
        .await
        .unwrap();
    assert!(put_res.unprocessed_items.unwrap_or_default().is_empty());

    let user_id = create_user(client).await;
    let filter_a = service
        .create_user_search_filter(&user_id, Faker.fake(), Faker.fake(), None)
        .await
        .unwrap();
    let filter_b = service
        .create_user_search_filter(&user_id, Faker.fake(), Faker.fake(), None)
        .await
        .unwrap();

    // Seed 5 matches for filter_a
    seed_match_records(
        client,
        &user_id,
        &filter_a.user_search_filter_id,
        &product_records[..5],
    )
    .await;
    // Seed 5 matches for filter_b
    seed_match_records(
        client,
        &user_id,
        &filter_b.user_search_filter_id,
        &product_records[5..],
    )
    .await;

    let expected = product_records[..5]
        .iter()
        .map(|record| record.product_id)
        .collect::<Vec<_>>();
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .route_key("GET /api/v1/me/search-filters/{userSearchFilterId}/matches")
            .jwt_claim("sub", user_id)
            .path_parameter("userSearchFilterId", filter_a.user_search_filter_id)
            .query_string_parameter("language", "de")
            .query_string_parameter("currency", "EUR")
            .query_string_parameter("sort", "created")
            .query_string_parameter("order", "asc")
            .query_string_parameter("searchAfter", "2021-12-31T23:59:59Z")
            .query_string_parameter("size", "20")
            .build(),
        context: Default::default(),
    };

    let query_product_service = MockQueryProductService::default();
    let response = handle(
        lambda_event,
        &service,
        &get_product_service,
        &query_product_service,
        None,
        None,
        &personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response.status_code);

    let actual: TimeCursoredData<PersonalizedData<GetProductData, ProductUserStateData>> =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert_eq!(5, actual.size);
    assert_eq!(5, actual.items.len());
    assert_eq!(
        expected,
        actual
            .items
            .into_iter()
            .map(|item| item.item.product_id)
            .collect::<Vec<_>>()
    );
    assert_eq!(5, actual.total.unwrap());
}

async fn create_free_user(client: &'static aws_sdk_dynamodb::Client) -> UserId {
    let user_repository = UserDynamoDbRepositoryImpl::new(client, "table_1");
    let user_service = user::service::user_service::UserServiceImpl::new(&user_repository);
    let user = user_service.create_user(Faker.fake()).await.unwrap();
    user.user_id
}

#[localstack_test(services = [DynamoDB()])]
async fn should_hide_products_when_search_filter_match_quota_exceeded() {
    let client = get_dynamodb_client().await;
    let (service, get_product_service, personalization_service) = setup_services(client);
    let product_repository = ProductDynamoDbRepositoryImpl::new(client, "table_1");

    // Create a Free-tier user (quota of 10 matches per month)
    let user_id = create_free_user(client).await;
    let free_search = product::core::product_search::ProductSearch {
        product_query: Faker.fake(),
        ..Default::default()
    };
    let search_filter = service
        .create_user_search_filter(&user_id, Faker.fake(), free_search, None)
        .await
        .unwrap();

    // Seed 10 match records that fill the quota — these should remain visible
    let within_quota_records = fake::vec![ProductRecord; 10];
    let put_res = product_repository
        .put_product_records(within_quota_records.clone().try_into().unwrap())
        .await
        .unwrap();
    assert!(put_res.unprocessed_items.unwrap_or_default().is_empty());

    seed_match_records(
        client,
        &user_id,
        &search_filter.user_search_filter_id,
        &within_quota_records,
    )
    .await;

    // Seed 3 more match records that exceed the quota — these should be hidden
    let beyond_quota_records = fake::vec![ProductRecord; 3];
    let put_res = product_repository
        .put_product_records(beyond_quota_records.clone().try_into().unwrap())
        .await
        .unwrap();
    assert!(put_res.unprocessed_items.unwrap_or_default().is_empty());

    seed_match_records(
        client,
        &user_id,
        &search_filter.user_search_filter_id,
        &beyond_quota_records,
    )
    .await;

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .route_key("GET /api/v1/me/search-filters/{userSearchFilterId}/matches")
            .jwt_claim("sub", user_id)
            .path_parameter("userSearchFilterId", search_filter.user_search_filter_id)
            .query_string_parameter("language", "en")
            .query_string_parameter("currency", "EUR")
            .query_string_parameter("sort", "created")
            .query_string_parameter("order", "asc")
            .query_string_parameter("searchAfter", "2021-12-31T23:59:59Z")
            .query_string_parameter("size", "20")
            .build(),
        context: Default::default(),
    };

    let query_product_service = MockQueryProductService::default();
    let response = handle(
        lambda_event,
        &service,
        &get_product_service,
        &query_product_service,
        None,
        None,
        &personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response.status_code);

    let actual: TimeCursoredData<PersonalizedData<GetProductData, ProductUserStateData>> =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();

    assert_eq!(13, actual.total.unwrap());

    let within_quota_product_ids: std::collections::HashSet<_> = within_quota_records
        .iter()
        .map(|r| r.product_id.to_string())
        .collect();

    let mut visible_count = 0;
    let mut hidden_count = 0;

    for item in &actual.items {
        let user_state = item.user_state.as_ref().unwrap();
        assert!(user_state.search_filter.matched);

        if within_quota_product_ids.contains(&item.item.product_id.to_string()) {
            // Within-quota items should be visible
            assert!(!user_state.search_filter.hidden);
            visible_count += 1;
        } else {
            // Beyond-quota items should be hidden and anonymized
            assert!(user_state.search_filter.hidden);
            assert_eq!(
                item.item.product_id.to_string(),
                "00000000-0000-0000-0000-000000000000"
            );
            assert_eq!(item.item.title.text, "Hidden Product Title");
            hidden_count += 1;
        }
    }

    assert_eq!(visible_count, 10);
    assert_eq!(hidden_count, 3);
}
