use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use product_classification::category::category_search::CategorySearchData;
use product_classification::category::data::get_category_summary_data::GetCategorySummaryData;
use product_classification::category::document::CategoryDocument;
use product_classification::category::dynamodb_repository::CategoryDynamoDbRepositoryImpl;
use product_classification::category::opensearch_repository::{
    CategoryOpenSearchRepository, CategoryOpenSearchRepositoryImpl,
};
use product_classification::category::service::CategoryServiceImpl;
use product_classification::period::service::MockPeriodService;
use product_classification_api::handle;
use test_api::*;

#[localstack_test(services = [DynamoDB(), OpenSearch()])]
async fn should_sort_by_name_ascending_when_name_asc_for_search_api() {
    let opensearch_repository =
        CategoryOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let dynamodb_repository =
        CategoryDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let category_service = CategoryServiceImpl::new(&dynamodb_repository, &opensearch_repository);
    let period_service = MockPeriodService::default();

    let names = ["Charlie", "Alpha", "Bravo"];
    for name in &names {
        let mut doc = Faker.fake::<CategoryDocument>();
        doc.display_name_en = name.to_string();
        let _ = opensearch_repository
            .index_category_document(doc)
            .await
            .unwrap();
    }
    refresh_index("categories").await;
    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .route_key("POST /api/v1/categories/search")
            .query_string_parameter("sort", "name")
            .query_string_parameter("order", "asc")
            .body_serde(&CategorySearchData::default())
            .build(),
        context: Default::default(),
    };
    let response = handle(lambda_event, &category_service, &period_service)
        .await
        .unwrap();

    assert_eq!(200, response.status_code);
    let payload: Vec<GetCategorySummaryData> =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert_eq!(3, payload.len());

    let names: Vec<_> = payload.iter().map(|c| c.name.text.as_str()).collect();
    assert_eq!(vec!["Alpha", "Bravo", "Charlie"], names);
}

#[localstack_test(services = [DynamoDB(), OpenSearch()])]
async fn should_sort_by_created_descending_when_created_desc_for_search_api() {
    let opensearch_repository =
        CategoryOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let dynamodb_repository =
        CategoryDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let category_service = CategoryServiceImpl::new(&dynamodb_repository, &opensearch_repository);
    let period_service = MockPeriodService::default();

    let timestamps = [
        time::macros::datetime!(2024-12-01 0:00 UTC),
        time::macros::datetime!(2020-01-01 0:00 UTC),
        time::macros::datetime!(2022-06-15 0:00 UTC),
    ];
    for ts in &timestamps {
        let mut doc = Faker.fake::<CategoryDocument>();
        doc.created = *ts;
        let _ = opensearch_repository
            .index_category_document(doc)
            .await
            .unwrap();
    }
    refresh_index("categories").await;
    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .route_key("POST /api/v1/categories/search")
            .query_string_parameter("sort", "created")
            .query_string_parameter("order", "desc")
            .body_serde(&CategorySearchData::default())
            .build(),
        context: Default::default(),
    };
    let response = handle(lambda_event, &category_service, &period_service)
        .await
        .unwrap();

    assert_eq!(200, response.status_code);
    let payload: Vec<GetCategorySummaryData> =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert_eq!(3, payload.len());

    let created: Vec<_> = payload.iter().map(|c| c.created).collect();
    assert_eq!(
        vec![
            time::macros::datetime!(2024-12-01 0:00 UTC),
            time::macros::datetime!(2022-06-15 0:00 UTC),
            time::macros::datetime!(2020-01-01 0:00 UTC),
        ],
        created
    );
}
