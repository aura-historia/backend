use common::user_id::UserId;
use fake::{Fake, Faker};
use product::core::item_search::ItemSearch;
use lambda_runtime::LambdaEvent;
use search_filter::dynamodb::repository::UserSearchFilterDynamoDbRepositoryImpl;
use search_filter::service::user_search_filter_service::{
    UserSearchFilterService, UserSearchFilterServiceImpl,
};
use search_filter_api_get_search_filter::handler;
use test_api::*;

#[localstack_test(services = [DynamoDB()])]
async fn should_return_actual_search_filter() {
    let repository =
        UserSearchFilterDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let service = UserSearchFilterServiceImpl::new(&repository);

    let user_id = UserId::new();
    let expected = service
        .save_user_search_filter(&user_id, Faker.fake(), Faker.fake::<ItemSearch>())
        .await
        .unwrap();
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .jwt_claim("sub", user_id)
            .path_parameter("userSearchFilterId", expected.user_search_filter_id)
            .build(),
        context: Default::default(),
    };

    let response = handler(lambda_event, &service).await.unwrap();
    assert_eq!(200, response.status_code);
    let json = extract_apigw_response_json_body!(response);

    assert_eq!(user_id.to_string(), json["userId"]);
    assert_eq!(
        expected.user_search_filter_id.to_string(),
        json["userSearchFilterId"]
    );
    assert_eq!(
        expected.search.item_query.to_string(),
        json["search"]["itemQuery"]
    );
}
