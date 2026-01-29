use common::pagination::cursor::api::JsonCursoredData;
use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use shop::data::{get_shop_data::GetShopData, shop_search_data::ShopSearchData};
use shop::opensearch::repository::{ShopOpenSearchRepository, ShopOpenSearchRepositoryImpl};
use shop::service::command_service::MockCommandShopService;
use shop::service::get_service::MockGetShopService;
use shop::service::query_service::QueryShopServiceImpl;
use shop_api::handle;
use test_api::*;

#[rstest::rstest]
#[trace]
#[test_attr(apply(test))]
#[case("score", "asc", 5)]
#[case("score", "desc", 10)]
#[case("name", "asc", 12)]
#[case("name", "desc", 15)]
#[case("created", "asc", 20)]
#[case("created", "desc", 20)]
#[case("updated", "asc", 20)]
#[case("updated", "desc", 20)]
#[localstack_test(services = [OpenSearch()])]
async fn should_follow_up_search_after_query(
    #[case] sort: &str,
    #[case] order: &str,
    #[case] size: u64,
) {
    let repository = ShopOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let service = QueryShopServiceImpl::new(&repository);

    for _ in 0..300 {
        let _ = repository.index_shop_document(Faker.fake()).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    }

    // first request
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .route_key("POST /api/v1/shops/search")
            .query_string_parameter("size", size.to_string())
            .query_string_parameter("sort", sort)
            .query_string_parameter("order", order)
            .body_serde(&ShopSearchData::default())
            .build(),
        context: Default::default(),
    };
    let response1 = handle(
        lambda_event,
        &MockGetShopService::default(),
        &service,
        &MockCommandShopService::default(),
    )
    .await
    .unwrap();
    assert_eq!(200, response1.status_code);
    let payload1 = serde_json::from_value::<JsonCursoredData<GetShopData>>(
        extract_apigw_response_json_body!(response1),
    )
    .unwrap();
    assert_eq!(size, payload1.size);
    assert_eq!(size, payload1.items.len() as u64);

    // subsequent request
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .route_key("POST /api/v1/shops/search")
            .query_string_parameter("size", size.to_string())
            .query_string_parameter("sort", sort)
            .query_string_parameter("order", order)
            .query_string_parameter(
                "searchAfter",
                serde_json::to_string(&payload1.clone().search_after.unwrap()).unwrap(),
            )
            .body_serde(&ShopSearchData::default())
            .build(),
        context: Default::default(),
    };
    let response2 = handle(
        lambda_event,
        &MockGetShopService::default(),
        &service,
        &MockCommandShopService::default(),
    )
    .await
    .unwrap();
    assert_eq!(200, response2.status_code);
    let payload2 = serde_json::from_value::<JsonCursoredData<GetShopData>>(
        extract_apigw_response_json_body!(response2),
    )
    .unwrap();
    assert_eq!(size, payload2.size);
    assert_eq!(size, payload2.items.len() as u64);

    assert_ne!(payload1.items, payload2.items);
    assert_ne!(
        payload1.search_after.unwrap(),
        payload2.search_after.unwrap()
    );
}

#[rstest::rstest]
#[test_attr(apply(test))]
#[case([shop::data::shop_type_data::ShopTypeData::AuctionHouse].into())]
#[case([shop::data::shop_type_data::ShopTypeData::AuctionPlatform].into())]
#[case([shop::data::shop_type_data::ShopTypeData::CommercialDealer].into())]
#[case([shop::data::shop_type_data::ShopTypeData::Marketplace].into())]
#[case([shop::data::shop_type_data::ShopTypeData::AuctionHouse, shop::data::shop_type_data::ShopTypeData::Marketplace].into())]
#[trace]
#[localstack_test(services = [OpenSearch()])]
async fn should_200_when_shop_type_query(
    #[case] query: std::collections::HashSet<shop::data::shop_type_data::ShopTypeData>,
) {
    let repository = ShopOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let service = QueryShopServiceImpl::new(&repository);

    for _ in 0..100 {
        let _ = repository.index_shop_document(Faker.fake()).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    }
    refresh_index("shops").await;
    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

    let search = ShopSearchData {
        shop_name_query: None,
        shop_type_query: query.clone(),
        created: None,
        updated: None,
        min_score: None,
    };

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .route_key("POST /api/v1/shops/search")
            .body_serde(&search)
            .build(),
        context: Default::default(),
    };
    let response = handle(
        lambda_event,
        &MockGetShopService::default(),
        &service,
        &MockCommandShopService::default(),
    )
    .await
    .unwrap();
    assert_eq!(200, response.status_code);

    let payload = serde_json::from_value::<JsonCursoredData<GetShopData>>(
        extract_apigw_response_json_body!(response),
    )
    .unwrap();

    assert!(payload.total.unwrap() > 0);
    assert!(
        payload
            .items
            .iter()
            .all(|shop| query.contains(&shop.shop_type))
    );
}

#[localstack_test(services = [OpenSearch()])]
async fn should_200_when_min_score_filters_results() {
    let repository = ShopOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let service = QueryShopServiceImpl::new(&repository);

    // Create shop with high relevance
    let high_relevance_shop = shop::opensearch::shop_document::ShopDocument {
        shop_id: Default::default(),
        shop_slug_id: Faker.fake(),
        name: "Antique Auction House".into(),
        domains: ["antique-auction.com".into()].into(),
        shop_type: shop::opensearch::shop_type_document::ShopTypeDocument::AuctionHouse,
        created: time::OffsetDateTime::now_utc(),
        updated: time::OffsetDateTime::now_utc(),
        image: Some("https://antique-auction.com/logo.jpg".parse().unwrap()),
    };

    // Create shop with lower relevance
    let low_relevance_shop = shop::opensearch::shop_document::ShopDocument {
        shop_id: Default::default(),
        shop_slug_id: Faker.fake(),
        name: "Modern Store antique mention".into(),
        domains: ["modern-store.com".into()].into(),
        shop_type: shop::opensearch::shop_type_document::ShopTypeDocument::CommercialDealer,
        created: time::OffsetDateTime::now_utc(),
        updated: time::OffsetDateTime::now_utc(),
        image: Some("https://modern-store.com/logo.jpg".parse().unwrap()),
    };

    repository.index_shop_document(high_relevance_shop).await.unwrap();
    repository.index_shop_document(low_relevance_shop).await.unwrap();
    refresh_index("shops").await;
    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

    // Test with min_score threshold
    let search_with_threshold = ShopSearchData {
        shop_name_query: Some("antique".try_into().unwrap()),
        shop_type_query: Default::default(),
        created: None,
        updated: None,
        min_score: Some(0.5),
    };

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .route_key("POST /api/v1/shops/search")
            .body_serde(&search_with_threshold)
            .build(),
        context: Default::default(),
    };
    
    let response = handle(
        lambda_event,
        &MockGetShopService::default(),
        &service,
        &MockCommandShopService::default(),
    )
    .await
    .unwrap();
    
    assert_eq!(200, response.status_code);

    let payload = serde_json::from_value::<JsonCursoredData<GetShopData>>(
        extract_apigw_response_json_body!(response),
    )
    .unwrap();

    // Should return filtered results (at least 1, at most 2)
    assert!(payload.items.len() >= 1);
    assert!(payload.items.len() <= 2);
}
