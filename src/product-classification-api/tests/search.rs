use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use product_classification::category::category_search::CategorySearchData;
use product_classification::category::data::get_category_summary_data::GetCategorySummaryData;
use product_classification::category::opensearch_repository::{
    CategoryOpenSearchRepository, CategoryOpenSearchRepositoryImpl,
};
use product_classification::category::service::CategoryServiceImpl;
use product_classification_api::handle;
use test_api::*;

#[rstest::rstest]
#[trace]
#[test_attr(apply(test))]
#[case("score", "desc")]
#[case("name", "asc")]
#[case("name", "desc")]
#[case("created", "asc")]
#[case("created", "desc")]
#[case("updated", "asc")]
#[case("updated", "desc")]
#[localstack_test(services = [DynamoDB(), OpenSearch()])]
async fn should_search_categories_with_sort(#[case] sort: &str, #[case] order: &str) {
    let opensearch_repository =
        CategoryOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let dynamodb_repository =
        product_classification::category::dynamodb_repository::CategoryDynamoDbRepositoryImpl::new(
            get_dynamodb_client().await,
            "table_1",
        );
    let service = CategoryServiceImpl::new(&dynamodb_repository, &opensearch_repository);

    for _ in 0..10 {
        let _ = opensearch_repository
            .index_category_document(Faker.fake())
            .await
            .unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    }
    refresh_index("categories").await;
    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .route_key("POST /api/v1/categories/search")
            .query_string_parameter("sort", sort)
            .query_string_parameter("order", order)
            .body_serde(&CategorySearchData::default())
            .build(),
        context: Default::default(),
    };
    let response = handle(lambda_event, &service).await.unwrap();

    assert_eq!(200, response.status_code);
    let payload: Vec<GetCategorySummaryData> =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert_eq!(10, payload.len());
}
