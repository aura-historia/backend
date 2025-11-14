use common::user_id::UserId;
use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use product::core::product_search::ProductSearch;
use search_filter::dynamodb::repository::{
    UserSearchFilterDynamoDbRepository, UserSearchFilterDynamoDbRepositoryImpl,
};
use search_filter::service::user_search_filter_service::{
    UserSearchFilterService, UserSearchFilterServiceImpl,
};
use search_filter_api_delete_search_filter::handler;
use test_api::*;

#[localstack_test(services = [DynamoDB()])]
async fn should_delete_search_filter() {
    let repository =
        UserSearchFilterDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let service = UserSearchFilterServiceImpl::new(&repository);

    let user_id = UserId::new();
    let expected = service
        .save_user_search_filter(&user_id, Faker.fake(), Faker.fake::<ProductSearch>())
        .await
        .unwrap();
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::DELETE)
            .jwt_claim("sub", user_id)
            .path_parameter("userSearchFilterId", expected.user_search_filter_id)
            .build(),
        context: Default::default(),
    };

    let response = handler(lambda_event, &service).await.unwrap();
    assert_eq!(204, response.status_code);

    let actual = repository
        .get_user_search_filter_record(&user_id, &expected.user_search_filter_id)
        .await
        .unwrap();
    assert!(actual.is_none());
}
