use common::user_id::UserId;
use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use search_filter_api_post_search_filter::{handler, post::PostUserSearchFilterData};
use search_filter_data::user_search_filter_data::UserSearchFilterData;
use search_filter_dynamodb::repository::{
    SearchFilterDynamoDbRepository, SearchFilterDynamoDbRepositoryImpl,
};
use search_filter_service::service::SearchFilterServiceImpl;
use test_api::*;

#[localstack_test(services = [DynamoDB()])]
async fn should_save_search_filter() {
    let user_id = UserId::new();
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .body_serde(&Faker.fake::<PostUserSearchFilterData>())
            .jwt_claim("sub", user_id)
            .build(),
        context: Default::default(),
    };

    let repository =
        SearchFilterDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let service = SearchFilterServiceImpl::new(&repository);
    let response = handler(lambda_event, &service).await.unwrap();
    assert_eq!(201, response.status_code);

    let json = extract_apigw_response_json_body!(response);
    let actual: UserSearchFilterData = serde_json::from_value(json).unwrap();
    assert_eq!(user_id, actual.user_id);

    let record = repository
        .get_search_filter_record(&user_id, &actual.search_filter_id)
        .await
        .unwrap();
    assert!(record.is_some());
}
