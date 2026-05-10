use common::user_search_filter_id::UserSearchFilterId;
use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use product::service::get_service::MockGetProductService;
use product::service::query_service::MockQueryProductService;
use product_personalization::service::MockProductPersonalizationService;
use search_filter::core::quota::SearchFilterQuota;
use search_filter::data::user_search_filter_data::UserSearchFilterData;
use search_filter::dynamodb::repository::{
    UserSearchFilterDynamoDbRepository, UserSearchFilterDynamoDbRepositoryImpl,
};
use search_filter::dynamodb::user_search_filter_record::{UserSearchFilterRecord, mk_pk, mk_sk};
use search_filter::service::user_search_filter_service::UserSearchFilterServiceImpl;
use search_filter_api::handle;
use search_filter_api::post_types::PostUserSearchFilterData;
use test_api::*;
use user::core::tier::UserTier;

use user::service::command::UpdateUserCommand;
use user::service::user_service::UserService;

#[localstack_test(services = [DynamoDB()])]
async fn should_save_search_filter() {
    let repository =
        UserSearchFilterDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let user_repository = user::dynamodb::repository::UserDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        "table_1",
    );
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

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .route_key("POST /api/v1/me/search-filters")
            .body_serde(&Faker.fake::<PostUserSearchFilterData>())
            .jwt_claim("sub", user.user_id)
            .build(),
        context: Default::default(),
    };

    let service = UserSearchFilterServiceImpl::new(&repository, &user_service);
    let get_product_service = MockGetProductService::default();
    let query_product_service = MockQueryProductService::default();
    let personalization_service = MockProductPersonalizationService::default();
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
    assert_eq!(201, response.status_code);

    let json = extract_apigw_response_json_body!(response);
    let actual: UserSearchFilterData = serde_json::from_value(json).unwrap();
    assert_eq!(user.user_id, actual.user_id);

    let record = repository
        .get_user_search_filter_record(&user.user_id, &actual.user_search_filter_id)
        .await
        .unwrap();
    assert!(record.is_some());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_422_when_search_filter_quota_is_exceeded() {
    let repository =
        UserSearchFilterDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let user_repository = user::dynamodb::repository::UserDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        "table_1",
    );
    let user_service = user::service::user_service::UserServiceImpl::new(&user_repository);
    let created_user = user_service.create_user(Faker.fake()).await.unwrap();
    let user_id = created_user.user_id;

    // Fill the quota by inserting records directly via repository (bypassing the service limit)
    let limit = UserTier::Free.search_filter_quota();
    for _ in 0..limit {
        let filter_id = UserSearchFilterId::new();
        let record = UserSearchFilterRecord {
            pk: mk_pk(&user_id),
            sk: mk_sk(&filter_id),
            user_id,
            user_search_filter_id: filter_id,
            ..Faker.fake()
        };
        repository
            .put_user_search_filter_record(record)
            .await
            .unwrap();
    }

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .route_key("POST /api/v1/me/search-filters")
            .body_serde(&Faker.fake::<PostUserSearchFilterData>())
            .jwt_claim("sub", user_id)
            .build(),
        context: Default::default(),
    };

    let service = UserSearchFilterServiceImpl::new(&repository, &user_service);
    let get_product_service = MockGetProductService::default();
    let query_product_service = MockQueryProductService::default();
    let personalization_service = MockProductPersonalizationService::default();
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
    .unwrap_err();

    assert_eq!(422, response.status);
    assert_eq!("SEARCH_FILTER_QUOTA_EXCEEDED", response.error.to_string());
}
