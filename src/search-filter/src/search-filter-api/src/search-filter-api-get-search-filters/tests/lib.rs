use common::user_id::UserId;
use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use search_filter_api_get_search_filters::handler;
use search_filter_core::{search_filter::SearchFilter, search_filter_id::SearchFilterId};
use search_filter_dynamodb::repository::SearchFilterDynamoDbRepositoryImpl;
use search_filter_service::service::{SearchFilterService, SearchFilterServiceImpl};
use test_api::*;

#[localstack_test(services = [DynamoDB()])]
async fn should_return_actual_search_filters_sortet_oldest_for_order_asc() {
    let repository =
        SearchFilterDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let service = SearchFilterServiceImpl::new(&repository);

    let user_id = UserId::new();
    let mut expected = vec![];
    for search_filter in fake::vec![SearchFilter; 81] {
        let saved = service
            .save_search_filter(&user_id, Faker.fake(), search_filter)
            .await
            .unwrap();
        expected.push(saved);
    }
    expected.sort_by(|l, r| l.created.cmp(&r.created));
    let expected: Vec<SearchFilterId> = expected
        .into_iter()
        .map(|filter| filter.search_filter_id)
        .collect();
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .query_string_parameter("sort", "created")
            .query_string_parameter("order", "asc")
            .jwt_claim("sub", user_id)
            .build(),
        context: Default::default(),
    };

    let response = handler(lambda_event, &service).await.unwrap();
    assert_eq!(200, response.status_code);
    let json = extract_apigw_response_json_body!(response);

    assert_eq!(
        expected,
        json["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|filter| filter["searchFilterId"].as_str().unwrap())
            .map(SearchFilterId::try_from)
            .map(Result::unwrap)
            .collect::<Vec<SearchFilterId>>()
    );
    assert_eq!(0, json["from"]);
    assert_eq!(81, json["size"]);
    assert_eq!(81, json["total"]);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_actual_search_filters_sortet_latest_for_order_desc() {
    let repository =
        SearchFilterDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let service = SearchFilterServiceImpl::new(&repository);

    let user_id = UserId::new();
    let mut expected = vec![];
    for search_filter in fake::vec![SearchFilter; 81] {
        let saved = service
            .save_search_filter(&user_id, Faker.fake(), search_filter)
            .await
            .unwrap();
        expected.push(saved);
    }
    expected.sort_by(|l, r| l.created.cmp(&r.created).reverse());
    let expected: Vec<SearchFilterId> = expected
        .into_iter()
        .map(|filter| filter.search_filter_id)
        .collect();
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .query_string_parameter("sort", "created")
            .query_string_parameter("order", "desc")
            .jwt_claim("sub", user_id)
            .build(),
        context: Default::default(),
    };

    let response = handler(lambda_event, &service).await.unwrap();
    assert_eq!(200, response.status_code);
    let json = extract_apigw_response_json_body!(response);

    assert_eq!(
        expected,
        json["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|filter| filter["searchFilterId"].as_str().unwrap())
            .map(SearchFilterId::try_from)
            .map(Result::unwrap)
            .collect::<Vec<SearchFilterId>>()
    );
    assert_eq!(0, json["from"]);
    assert_eq!(81, json["size"]);
    assert_eq!(81, json["total"]);
}
