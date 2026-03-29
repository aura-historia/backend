use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use product::core::product_search::ProductSearch;
use product::service::get_service::MockGetProductService;
use product_personalization::service::MockProductPersonalizationService;
use search_filter::dynamodb::repository::{
    UserSearchFilterDynamoDbRepository, UserSearchFilterDynamoDbRepositoryImpl,
};
use search_filter::service::user_search_filter_service::{
    UserSearchFilterService, UserSearchFilterServiceImpl,
};
use search_filter_api::handle;
use test_api::*;
use user::service::user_service::UserService;

#[localstack_test(services = [DynamoDB()])]
async fn should_delete_search_filter() {
    let repository =
        UserSearchFilterDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let user_repository = user::dynamodb::repository::UserDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        "table_1",
    );
    let user_service = user::service::user_service::UserServiceImpl::new(&user_repository);
    let service = UserSearchFilterServiceImpl::new(&repository, &user_service);
    let get_product_service = MockGetProductService::default();
    let personalization_service = MockProductPersonalizationService::default();

    let user_id = user_service
        .create_user(Faker.fake())
        .await
        .unwrap()
        .user_id;
    let expected = service
        .create_user_search_filter(&user_id, Faker.fake(), Faker.fake::<ProductSearch>())
        .await
        .unwrap();
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::DELETE)
            .route_key("DELETE /api/v1/me/search-filters/{userSearchFilterId}")
            .jwt_claim("sub", user_id)
            .path_parameter("userSearchFilterId", expected.user_search_filter_id)
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
    assert_eq!(204, response.status_code);

    let actual = repository
        .get_user_search_filter_record(&user_id, &expected.user_search_filter_id)
        .await
        .unwrap();
    assert!(actual.is_none());
}
