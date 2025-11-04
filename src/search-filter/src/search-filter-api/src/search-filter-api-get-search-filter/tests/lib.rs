use common::user_id::UserId;
use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use search_filter_api_get_search_filter::handler;
use search_filter_core::search_filter::SearchFilter;
use search_filter_dynamodb::repository::SearchFilterDynamoDbRepositoryImpl;
use search_filter_service::service::{SearchFilterService, SearchFilterServiceImpl};
use test_api::*;

#[localstack_test(services = [DynamoDB()])]
async fn should_return_actual_search_filter() {
    let repository =
        SearchFilterDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let service = SearchFilterServiceImpl::new(&repository);

    let user_id = UserId::new();
    let expected = service
        .save_search_filter(&user_id, Faker.fake(), Faker.fake::<SearchFilter>())
        .await
        .unwrap();
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .jwt_claim("sub", user_id)
            .path_parameter("searchFilterId", expected.search_filter_id)
            .build(),
        context: Default::default(),
    };

    let response = handler(lambda_event, &service).await.unwrap();
    assert_eq!(200, response.status_code);
    let json = extract_apigw_response_json_body!(response);

    assert_eq!(user_id.to_string(), json["userId"]);
    assert_eq!(
        expected.search_filter_id.to_string(),
        json["searchFilterId"]
    );
    assert_eq!(
        expected.search_filter.item_query.to_string(),
        json["search"]["itemQuery"]
    );
}
