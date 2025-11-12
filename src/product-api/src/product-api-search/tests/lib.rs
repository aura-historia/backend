use cognito::access_token_verifier_service::MockAccessTokenVerifierService;
use common::personalized::api::PersonalizedData;
use common::{pagination::cursor::api::JsonCursoredData, query::range_query::RangeQuery};
use fake::{Fake, Faker, rand};
use item_api_search::handler;
use lambda_runtime::LambdaEvent;
use product::data::get_data::GetItemData;
use product::data::product_search_data::ProductSearchData;
use product::data::user_state_data::ItemUserStateData;
use product::opensearch::{
    item_document::ProductDocument,
    repository::{ProductOpenSearchRepository, ProductOpenSearchRepositoryImpl},
};
use product::service::personalization_service::ItemPersonalizationServiceImpl;
use product::service::query_service::QueryItemServiceImpl;
use product::watchlist::dynamodb::repository::WatchlistItemDynamoDbRepositoryImpl;
use test_api::*;
use time::OffsetDateTime;
use time::macros::datetime;

#[localstack_test(services = [OpenSearch(), DynamoDB()])]
async fn should_200_when_no_hits() {
    let ddb_client = get_dynamodb_client().await;
    let watchlist_repository = WatchlistItemDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let item_personalization_service = ItemPersonalizationServiceImpl::new(&watchlist_repository);
    let opensearch_repository = ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let query_service = QueryItemServiceImpl::new(&opensearch_repository);
    let mut access_token_verifier_service = MockAccessTokenVerifierService::default();
    access_token_verifier_service
        .expect_verify_extract_user_id()
        .returning(|_| Box::pin(async { Ok(None) }));

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .body_serde(&Faker.fake::<ProductSearchData>())
            .build(),
        context: Default::default(),
    };

    let response = handler(
        lambda_event,
        &query_service,
        &access_token_verifier_service,
        &item_personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response.status_code);

    let json = extract_apigw_response_json_body!(response);
    let response_data: JsonCursoredData<PersonalizedData<GetItemData, ItemUserStateData>> =
        serde_json::from_value(json).unwrap();
    assert!(response_data.items.is_empty());
    assert_eq!(0, response_data.total.unwrap());
}

#[localstack_test(services = [OpenSearch(), DynamoDB()])]
async fn should_200_when_following_search_after_from_previous_response_for_sort_price_asc() {
    let ddb_client = get_dynamodb_client().await;
    let watchlist_repository = WatchlistItemDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let item_personalization_service = ItemPersonalizationServiceImpl::new(&watchlist_repository);
    let opensearch_repository = ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let query_service = QueryItemServiceImpl::new(&opensearch_repository);
    let mut access_token_verifier_service = MockAccessTokenVerifierService::default();
    access_token_verifier_service
        .expect_verify_extract_user_id()
        .returning(|_| Box::pin(async { Ok(None) }));

    let search = ProductSearchData {
        language: common::language::data::LanguageData::De,
        currency: common::currency::data::CurrencyData::Eur,
        item_query: "Der erwartete Titel".try_into().unwrap(),
        shop_name_query: None,
        price_query: None,
        state_query: Default::default(),
        created_query: None,
        updated_query: None,
    };

    let mut items = fake::vec![ProductDocument; 1370];
    for item in &mut items {
        item.title_de = Some("Der erwartete Titel".to_string());
        item.price_eur = Some(rand::random_range(1..=10000000));
    }
    let create_res = opensearch_repository
        .create_item_documents(items.clone())
        .await
        .unwrap();
    assert!(!create_res.errors);
    refresh_index("items").await;
    tokio::time::sleep(std::time::Duration::from_secs(10)).await;

    let sorter = |l: &ProductDocument, r: &ProductDocument| match l
        .price_eur
        .unwrap()
        .cmp(&r.price_eur.unwrap())
    {
        std::cmp::Ordering::Equal => l.product_id.to_string().cmp(&r.product_id.to_string()),
        ord => ord,
    };
    items.sort_by(sorter);

    // first request
    let lambda_event_1 = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .query_string_parameter("sort", "price")
            .query_string_parameter("order", "asc")
            .query_string_parameter("size", "50")
            .body_serde(&search)
            .build(),
        context: Default::default(),
    };

    let response_1 = handler(
        lambda_event_1,
        &query_service,
        &access_token_verifier_service,
        &item_personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response_1.status_code);
    let json = extract_apigw_response_json_body!(response_1);
    let response_data: JsonCursoredData<PersonalizedData<GetItemData, ItemUserStateData>> =
        serde_json::from_value(json).unwrap();
    assert_eq!(50, response_data.size);
    assert_eq!(1370, response_data.total.unwrap());
    assert_eq!(
        items
            .clone()
            .into_iter()
            .take(50)
            .map(|item| item.product_id)
            .collect::<Vec<_>>(),
        response_data
            .items
            .into_iter()
            .map(|item| item.item.product_id)
            .collect::<Vec<_>>()
    );

    // second request following up on first
    let lambda_event_2 = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .query_string_parameter("sort", "price")
            .query_string_parameter("order", "asc")
            .query_string_parameter("size", "50")
            .query_string_parameter(
                "searchAfter",
                serde_json::to_string(&response_data.search_after.unwrap()).unwrap(),
            )
            .body_serde(&search)
            .build(),
        context: Default::default(),
    };

    let response_2 = handler(
        lambda_event_2,
        &query_service,
        &access_token_verifier_service,
        &item_personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response_2.status_code);
    let json_2 = extract_apigw_response_json_body!(response_2);
    let response_data_2: JsonCursoredData<PersonalizedData<GetItemData, ItemUserStateData>> =
        serde_json::from_value(json_2).unwrap();
    assert_eq!(50, response_data_2.size);
    assert_eq!(1370, response_data_2.total.unwrap());
    assert_eq!(
        items
            .clone()
            .into_iter()
            .skip(50)
            .take(50)
            .map(|item| item.product_id)
            .collect::<Vec<_>>(),
        response_data_2
            .items
            .into_iter()
            .map(|item| item.item.product_id)
            .collect::<Vec<_>>()
    );
}

#[localstack_test(services = [OpenSearch(), DynamoDB()])]
async fn should_200_when_following_search_after_from_previous_response_for_sort_price_desc() {
    let ddb_client = get_dynamodb_client().await;
    let watchlist_repository = WatchlistItemDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let item_personalization_service = ItemPersonalizationServiceImpl::new(&watchlist_repository);
    let opensearch_repository = ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let query_service = QueryItemServiceImpl::new(&opensearch_repository);
    let mut access_token_verifier_service = MockAccessTokenVerifierService::default();
    access_token_verifier_service
        .expect_verify_extract_user_id()
        .returning(|_| Box::pin(async { Ok(None) }));

    let search = ProductSearchData {
        language: common::language::data::LanguageData::De,
        currency: common::currency::data::CurrencyData::Eur,
        item_query: "Der erwartete Titel".try_into().unwrap(),
        shop_name_query: None,
        price_query: None,
        state_query: Default::default(),
        created_query: None,
        updated_query: None,
    };

    let mut items = fake::vec![ProductDocument; 1370];
    for item in &mut items {
        item.title_de = Some("Der erwartete Titel".to_string());
        item.price_eur = Some(rand::random_range(1..=10000000));
    }
    let create_res = opensearch_repository
        .create_item_documents(items.clone())
        .await
        .unwrap();
    assert!(!create_res.errors);
    refresh_index("items").await;
    tokio::time::sleep(std::time::Duration::from_secs(10)).await;

    let sorter = |l: &ProductDocument, r: &ProductDocument| match l
        .price_eur
        .unwrap()
        .cmp(&r.price_eur.unwrap())
        .reverse()
    {
        std::cmp::Ordering::Equal => l
            .product_id
            .to_string()
            .cmp(&r.product_id.to_string())
            .reverse(),
        ord => ord,
    };
    items.sort_by(sorter);

    // first request
    let lambda_event_1 = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .query_string_parameter("sort", "price")
            .query_string_parameter("order", "desc")
            .query_string_parameter("size", "50")
            .body_serde(&search)
            .build(),
        context: Default::default(),
    };

    let response_1 = handler(
        lambda_event_1,
        &query_service,
        &access_token_verifier_service,
        &item_personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response_1.status_code);
    let json = extract_apigw_response_json_body!(response_1);
    let response_data: JsonCursoredData<PersonalizedData<GetItemData, ItemUserStateData>> =
        serde_json::from_value(json).unwrap();
    assert_eq!(50, response_data.size);
    assert_eq!(1370, response_data.total.unwrap());
    assert_eq!(
        items
            .clone()
            .into_iter()
            .take(50)
            .map(|item| item.product_id)
            .collect::<Vec<_>>(),
        response_data
            .items
            .into_iter()
            .map(|item| item.item.product_id)
            .collect::<Vec<_>>()
    );

    // second request following up on first
    let lambda_event_2 = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .query_string_parameter("sort", "price")
            .query_string_parameter("order", "desc")
            .query_string_parameter("size", "50")
            .query_string_parameter(
                "searchAfter",
                serde_json::to_string(&response_data.search_after.unwrap()).unwrap(),
            )
            .body_serde(&search)
            .build(),
        context: Default::default(),
    };

    let response_2 = handler(
        lambda_event_2,
        &query_service,
        &access_token_verifier_service,
        &item_personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response_2.status_code);
    let json_2 = extract_apigw_response_json_body!(response_2);
    let response_data_2: JsonCursoredData<PersonalizedData<GetItemData, ItemUserStateData>> =
        serde_json::from_value(json_2).unwrap();
    assert_eq!(50, response_data_2.size);
    assert_eq!(1370, response_data_2.total.unwrap());
    assert_eq!(
        items
            .clone()
            .into_iter()
            .skip(50)
            .take(50)
            .map(|item| item.product_id)
            .collect::<Vec<_>>(),
        response_data_2
            .items
            .into_iter()
            .map(|item| item.item.product_id)
            .collect::<Vec<_>>()
    );
}

#[localstack_test(services = [OpenSearch(), DynamoDB()])]
async fn should_200_when_following_search_after_from_previous_response_for_implicit_sort_score() {
    let ddb_client = get_dynamodb_client().await;
    let watchlist_repository = WatchlistItemDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let item_personalization_service = ItemPersonalizationServiceImpl::new(&watchlist_repository);
    let opensearch_repository = ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let query_service = QueryItemServiceImpl::new(&opensearch_repository);
    let mut access_token_verifier_service = MockAccessTokenVerifierService::default();
    access_token_verifier_service
        .expect_verify_extract_user_id()
        .returning(|_| Box::pin(async { Ok(None) }));

    let search = ProductSearchData {
        language: common::language::data::LanguageData::De,
        currency: common::currency::data::CurrencyData::Usd,
        item_query: "Der erwartete Titel".try_into().unwrap(),
        shop_name_query: None,
        price_query: None,
        state_query: Default::default(),
        created_query: None,
        updated_query: None,
    };

    let mut items = fake::vec![ProductDocument; 1370];
    for item in &mut items {
        item.title_de = Some("Der erwartete Titel".to_string());
    }
    let create_res = opensearch_repository
        .create_item_documents(items.clone())
        .await
        .unwrap();
    assert!(!create_res.errors);
    refresh_index("items").await;
    tokio::time::sleep(std::time::Duration::from_secs(10)).await;

    // first request
    let lambda_event_1 = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .query_string_parameter("size", "50")
            .body_serde(&search)
            .build(),
        context: Default::default(),
    };

    let response_1 = handler(
        lambda_event_1,
        &query_service,
        &access_token_verifier_service,
        &item_personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response_1.status_code);
    let json = extract_apigw_response_json_body!(response_1);
    let response_data: JsonCursoredData<PersonalizedData<GetItemData, ItemUserStateData>> =
        serde_json::from_value(json).unwrap();
    assert_eq!(50, response_data.size);
    assert_eq!(1370, response_data.total.unwrap());

    // second request following up on first
    let lambda_event_2 = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .query_string_parameter("size", "50")
            .query_string_parameter(
                "searchAfter",
                serde_json::to_string(&response_data.search_after.unwrap()).unwrap(),
            )
            .body_serde(&search)
            .build(),
        context: Default::default(),
    };

    let response_2 = handler(
        lambda_event_2,
        &query_service,
        &access_token_verifier_service,
        &item_personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response_2.status_code);
    let json_2 = extract_apigw_response_json_body!(response_2);
    let response_data_2: JsonCursoredData<PersonalizedData<GetItemData, ItemUserStateData>> =
        serde_json::from_value(json_2).unwrap();
    assert_eq!(50, response_data_2.size);
    assert_eq!(1370, response_data_2.total.unwrap());

    assert!(response_data_2.items.iter().all(|item| {
        !response_data
            .items
            .iter()
            .map(|item| item.item.product_id)
            .collect::<Vec<_>>()
            .contains(&item.item.product_id)
    }))
}

#[localstack_test(services = [OpenSearch(), DynamoDB()])]
async fn should_200_when_following_search_after_from_previous_response_for_explicit_sort_score() {
    let ddb_client = get_dynamodb_client().await;
    let watchlist_repository = WatchlistItemDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let item_personalization_service = ItemPersonalizationServiceImpl::new(&watchlist_repository);
    let opensearch_repository = ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let query_service = QueryItemServiceImpl::new(&opensearch_repository);
    let mut access_token_verifier_service = MockAccessTokenVerifierService::default();
    access_token_verifier_service
        .expect_verify_extract_user_id()
        .returning(|_| Box::pin(async { Ok(None) }));

    let search = ProductSearchData {
        language: common::language::data::LanguageData::De,
        currency: common::currency::data::CurrencyData::Usd,
        item_query: "Der erwartete Titel".try_into().unwrap(),
        shop_name_query: None,
        price_query: None,
        state_query: Default::default(),
        created_query: None,
        updated_query: None,
    };

    let mut items = fake::vec![ProductDocument; 1370];
    for item in &mut items {
        item.title_de = Some("Der erwartete Titel".to_string());
    }
    let create_res = opensearch_repository
        .create_item_documents(items.clone())
        .await
        .unwrap();
    assert!(!create_res.errors);
    refresh_index("items").await;
    tokio::time::sleep(std::time::Duration::from_secs(10)).await;

    // first request
    let lambda_event_1 = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .query_string_parameter("size", "50")
            .query_string_parameter("sort", "score")
            .query_string_parameter("order", "desc")
            .body_serde(&search)
            .build(),
        context: Default::default(),
    };

    let response_1 = handler(
        lambda_event_1,
        &query_service,
        &access_token_verifier_service,
        &item_personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response_1.status_code);
    let json = extract_apigw_response_json_body!(response_1);
    let response_data: JsonCursoredData<PersonalizedData<GetItemData, ItemUserStateData>> =
        serde_json::from_value(json).unwrap();
    assert_eq!(50, response_data.size);
    assert_eq!(1370, response_data.total.unwrap());

    // second request following up on first
    let lambda_event_2 = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .query_string_parameter("size", "50")
            .query_string_parameter("sort", "score")
            .query_string_parameter("order", "desc")
            .query_string_parameter(
                "searchAfter",
                serde_json::to_string(&response_data.search_after.unwrap()).unwrap(),
            )
            .body_serde(&search)
            .build(),
        context: Default::default(),
    };

    let response_2 = handler(
        lambda_event_2,
        &query_service,
        &access_token_verifier_service,
        &item_personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response_2.status_code);
    let json_2 = extract_apigw_response_json_body!(response_2);
    let response_data_2: JsonCursoredData<PersonalizedData<GetItemData, ItemUserStateData>> =
        serde_json::from_value(json_2).unwrap();
    assert_eq!(50, response_data_2.size);
    assert_eq!(1370, response_data_2.total.unwrap());

    assert!(response_data_2.items.iter().all(|item| {
        !response_data
            .items
            .iter()
            .map(|item| item.item.product_id)
            .collect::<Vec<_>>()
            .contains(&item.item.product_id)
    }))
}

#[rstest::rstest]
#[test_attr(apply(test))]
#[case(None, None)]
#[case(None, Some(OffsetDateTime::now_utc().checked_add(time::Duration::seconds(60)).unwrap()))]
#[case(Some(datetime!(2000 - 01 - 05 0:00 UTC)), None)]
#[case(Some(datetime!(2025 - 01 - 05 0:00 UTC)), Some(OffsetDateTime::now_utc().checked_add(time::Duration::seconds(30)).unwrap()))]
#[localstack_test(services = [OpenSearch(), DynamoDB()])]
async fn should_200_when_created_query(
    #[case] min: Option<OffsetDateTime>,
    #[case] max: Option<OffsetDateTime>,
) {
    let ddb_client = get_dynamodb_client().await;
    let watchlist_repository = WatchlistItemDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let item_personalization_service = ItemPersonalizationServiceImpl::new(&watchlist_repository);
    let opensearch_repository = ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let query_service = QueryItemServiceImpl::new(&opensearch_repository);
    let mut access_token_verifier_service = MockAccessTokenVerifierService::default();
    access_token_verifier_service
        .expect_verify_extract_user_id()
        .returning(|_| Box::pin(async { Ok(None) }));

    let created = RangeQuery { min, max };
    let search = ProductSearchData {
        language: common::language::data::LanguageData::De,
        currency: common::currency::data::CurrencyData::Eur,
        item_query: "Der erwartete Titel".try_into().unwrap(),
        shop_name_query: None,
        price_query: None,
        state_query: Default::default(),
        created_query: Some(created),
        updated_query: None,
    };
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .body_serde(&search)
            .build(),
        context: Default::default(),
    };

    let mut items = fake::vec![ProductDocument; 1370];
    for item in &mut items {
        item.title_de = Some("Der erwartete Titel".to_string());
    }
    let create_res = opensearch_repository
        .create_item_documents(items.clone())
        .await
        .unwrap();
    assert!(!create_res.errors);
    refresh_index("items").await;
    tokio::time::sleep(std::time::Duration::from_secs(10)).await;

    let response = handler(
        lambda_event,
        &query_service,
        &access_token_verifier_service,
        &item_personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response.status_code);

    let json = extract_apigw_response_json_body!(response);
    let response_data: JsonCursoredData<PersonalizedData<GetItemData, ItemUserStateData>> =
        serde_json::from_value(json).unwrap();
    assert!(!response_data.items.is_empty());

    if let Some(min) = min {
        assert!(
            response_data
                .items
                .iter()
                .all(|item| item.item.created >= min)
        );
    }
    if let Some(max) = max {
        assert!(
            response_data
                .items
                .iter()
                .all(|item| item.item.created <= max)
        );
    }
}

#[rstest::rstest]
#[test_attr(apply(test))]
#[case(None, None)]
#[case(None, Some(OffsetDateTime::now_utc().checked_add(time::Duration::seconds(60)).unwrap()))]
#[case(Some(datetime!(2000 - 01 - 05 0:00 UTC)), None)]
#[case(Some(datetime!(2025 - 01 - 05 0:00 UTC)), Some(OffsetDateTime::now_utc().checked_add(time::Duration::seconds(30)).unwrap()))]
#[localstack_test(services = [OpenSearch(), DynamoDB()])]
async fn should_200_when_updated_query(
    #[case] min: Option<OffsetDateTime>,
    #[case] max: Option<OffsetDateTime>,
) {
    let ddb_client = get_dynamodb_client().await;
    let watchlist_repository = WatchlistItemDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let item_personalization_service = ItemPersonalizationServiceImpl::new(&watchlist_repository);
    let opensearch_repository = ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let query_service = QueryItemServiceImpl::new(&opensearch_repository);
    let mut access_token_verifier_service = MockAccessTokenVerifierService::default();
    access_token_verifier_service
        .expect_verify_extract_user_id()
        .returning(|_| Box::pin(async { Ok(None) }));

    let updated = RangeQuery { min, max };
    let search = ProductSearchData {
        language: common::language::data::LanguageData::De,
        currency: common::currency::data::CurrencyData::Eur,
        item_query: "Der erwartete Titel".try_into().unwrap(),
        shop_name_query: None,
        price_query: None,
        state_query: Default::default(),
        created_query: None,
        updated_query: Some(updated),
    };
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .body_serde(&search)
            .build(),
        context: Default::default(),
    };

    let mut items = fake::vec![ProductDocument; 1370];
    for item in &mut items {
        item.title_de = Some("Der erwartete Titel".to_string());
    }
    let create_res = opensearch_repository
        .create_item_documents(items.clone())
        .await
        .unwrap();
    assert!(!create_res.errors);
    refresh_index("items").await;
    tokio::time::sleep(std::time::Duration::from_secs(10)).await;

    let response = handler(
        lambda_event,
        &query_service,
        &access_token_verifier_service,
        &item_personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response.status_code);

    let json = extract_apigw_response_json_body!(response);
    let response_data: JsonCursoredData<PersonalizedData<GetItemData, ItemUserStateData>> =
        serde_json::from_value(json).unwrap();
    assert!(!response_data.items.is_empty());

    if let Some(min) = min {
        assert!(
            response_data
                .items
                .iter()
                .all(|item| item.item.updated >= min)
        );
    }
    if let Some(max) = max {
        assert!(
            response_data
                .items
                .iter()
                .all(|item| item.item.updated <= max)
        );
    }
}

#[localstack_test(services = [OpenSearch(), DynamoDB()])]
async fn should_200_personalized_when_authenticated_and_not_watching() {
    let ddb_client = get_dynamodb_client().await;
    let watchlist_repository = WatchlistItemDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let item_personalization_service = ItemPersonalizationServiceImpl::new(&watchlist_repository);
    let opensearch_repository = ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let query_service = QueryItemServiceImpl::new(&opensearch_repository);
    let mut access_token_verifier_service = MockAccessTokenVerifierService::default();
    access_token_verifier_service
        .expect_verify_extract_user_id()
        .returning(|_| Box::pin(async { Ok(Some(Faker.fake())) }));

    let search = ProductSearchData {
        language: common::language::data::LanguageData::De,
        currency: common::currency::data::CurrencyData::Eur,
        item_query: "Der erwartete Titel".try_into().unwrap(),
        shop_name_query: None,
        price_query: None,
        state_query: Default::default(),
        created_query: None,
        updated_query: None,
    };
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .body_serde(&search)
            .build(),
        context: Default::default(),
    };

    let mut items = fake::vec![ProductDocument; 1370];
    for item in &mut items {
        item.title_de = Some("Der erwartete Titel".to_string());
    }
    let create_res = opensearch_repository
        .create_item_documents(items.clone())
        .await
        .unwrap();
    assert!(!create_res.errors);
    refresh_index("items").await;
    tokio::time::sleep(std::time::Duration::from_secs(10)).await;

    let response = handler(
        lambda_event,
        &query_service,
        &access_token_verifier_service,
        &item_personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response.status_code);

    let json = extract_apigw_response_json_body!(response);
    assert!(json["items"][0]["userState"].is_object());
    let response_data: JsonCursoredData<PersonalizedData<GetItemData, ItemUserStateData>> =
        serde_json::from_value(json).unwrap();
    assert!(response_data.items.iter().all(|item| {
        let user_state = item.user_state.unwrap();
        !user_state.watchlist.notifications && !user_state.watchlist.watching
    }));
}
