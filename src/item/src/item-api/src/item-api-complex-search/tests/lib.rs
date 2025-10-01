use common::pagination::page::api::PaginatedData;
use fake::{Fake, Faker};
use item_api_complex_search::handler;
use item_data::get_data::GetItemData;
use item_opensearch::repository::ItemOpenSearchRepositoryImpl;
use item_service::query_service::QueryItemServiceImpl;
use lambda_runtime::LambdaEvent;
use search_filter_data::search_filter_data::SearchFilterData;
use test_api::*;

#[localstack_test(services = [OpenSearch()])]
async fn should_200_when_no_hits() {
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .body_serde(&Faker.fake::<SearchFilterData>())
            .build(),
        context: Default::default(),
    };

    let repository = ItemOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let service = QueryItemServiceImpl::new(&repository);

    let response = handler(lambda_event, &service).await.unwrap();
    assert_eq!(200, response.status_code);

    let json = extract_apigw_response_json_body!(response);
    let collection_data: PaginatedData<GetItemData> = serde_json::from_value(json).unwrap();
    assert!(collection_data.items.is_empty());
    assert_eq!(0, collection_data.total.unwrap());
}
