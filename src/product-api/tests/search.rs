use cognito::access_token_verifier_service::MockAccessTokenVerifierService;
use common::currency::data::CurrencyData;
use common::distance::data::{DistanceData, DistanceUnitData, GeoDistanceQueryData};
use common::language::data::LanguageData;
use common::language::document::{LanguageDocument, TextDocument};
use common::language::domain::Language;
use common::personalized::api::PersonalizedData;
use common::user_id::UserId;
use common::{pagination::cursor::api::JsonCursoredData, query::range_query::RangeQuery};
use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use notification::service::notification_service::MockNotificationService;
use product::data::get_summary_data::GetProductSummaryData;
use product::data::product_search_data::ProductSearchData;
use product::data::user_state_data::ProductUserStateData;
use product::opensearch::{
    product_document::ProductDocument,
    repository::{ProductOpenSearchRepository, ProductOpenSearchRepositoryImpl},
};
use product::service::query_service::QueryProductServiceImpl;
use product_api::search::handle;
use product_personalization::service::ProductPersonalizationServiceImpl;
use product_pipeline_embed_text::service::MockMultimodalEmbeddingService;
use product_watchlist::dynamodb::repository::WatchlistProductDynamoDbRepositoryImpl;
use search_filter::dynamodb::repository::MockUserSearchFilterDynamoDbRepository;
use shop::data::shop_type_data::ShopTypeData;
use shop::opensearch::continent_document::ContinentDocument;
use std::collections::HashSet;
use std::time::Duration;
use test_api::*;
use time::OffsetDateTime;
use time::macros::datetime;
use user::dynamodb::repository::UserDynamoDbRepositoryImpl;
use user::service::user_service::{UserService, UserServiceImpl};

fn one_hot_embedding(slot: usize) -> Vec<f32> {
    let mut embedding = vec![0.0_f32; 768];
    embedding[slot] = 1.0;
    embedding
}

#[localstack_test(services = [OpenSearch(), DynamoDB()])]
async fn should_200_when_no_hits() {
    let ddb_client = get_dynamodb_client().await;
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let notification_service = MockNotificationService::default();
    let search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
    let product_personalization_service = ProductPersonalizationServiceImpl::new(
        &watchlist_repository,
        &notification_service,
        &user_service,
        &search_filter_repository,
    );
    let opensearch_repository = ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let query_service = QueryProductServiceImpl::new(&opensearch_repository);
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

    let response = handle(
        lambda_event,
        &query_service,
        None,
        &access_token_verifier_service,
        &product_personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response.status_code);

    let json = extract_apigw_response_json_body!(response);
    let response_data: JsonCursoredData<
        PersonalizedData<GetProductSummaryData, ProductUserStateData>,
    > = serde_json::from_value(json).unwrap();
    assert!(response_data.items.is_empty());
    assert_eq!(0, response_data.total.unwrap());
}

#[localstack_test(services = [OpenSearch(), DynamoDB()])]
async fn should_200_filter_products_when_geo_filters_are_given() {
    let ddb_client = get_dynamodb_client().await;
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let notification_service = MockNotificationService::default();
    let search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
    let product_personalization_service = ProductPersonalizationServiceImpl::new(
        &watchlist_repository,
        &notification_service,
        &user_service,
        &search_filter_repository,
    );
    let opensearch_repository = ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let query_service = QueryProductServiceImpl::new(&opensearch_repository);
    let mut access_token_verifier_service = MockAccessTokenVerifierService::default();
    access_token_verifier_service
        .expect_verify_extract_user_id()
        .returning(|_| Box::pin(async { Ok(None) }));
    let mut expected = Faker.fake::<ProductDocument>();
    expected.structured_address_country = Some(isocountry::CountryCode::DEU);
    expected.structured_address_continent = Some(ContinentDocument::Europe);
    expected.geo_address = Some("52.5200,13.4050".to_string());
    let mut other = Faker.fake::<ProductDocument>();
    other.structured_address_country = Some(isocountry::CountryCode::USA);
    other.structured_address_continent = Some(ContinentDocument::NorthAmerica);
    other.geo_address = Some("40.7128,-74.0060".to_string());
    let create_res = opensearch_repository
        .create_product_documents(vec![expected.clone(), other])
        .await
        .unwrap();
    assert!(!create_res.errors);
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let search = ProductSearchData {
        language: LanguageData::En,
        currency: CurrencyData::Eur,
        product_query: None,
        shop_name_query: Default::default(),
        exclude_shop_name_query: Default::default(),
        seller_name_query: Default::default(),
        exclude_seller_name_query: Default::default(),
        shop_type_query: Default::default(),
        country_query: HashSet::from_iter([isocountry::CountryCode::DEU]),
        continent_query: HashSet::from_iter([geo::data::continent_data::ContinentData::Europe]),
        geo_address_distance_query: Some(GeoDistanceQueryData {
            lat: 52.5200,
            lon: 13.4050,
            distance: DistanceData {
                amount: 50.0,
                unit: DistanceUnitData::Kilometers,
            },
        }),
        price_query: None,
        state_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
        shop_slug_id_query: Default::default(),
        exclude_shop_slug_id_query: Default::default(),
        seller_slug_id_query: Default::default(),
        exclude_seller_slug_id_query: Default::default(),
    };
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .body_serde(&search)
            .build(),
        context: Default::default(),
    };

    let response = handle(
        lambda_event,
        &query_service,
        None,
        &access_token_verifier_service,
        &product_personalization_service,
    )
    .await
    .unwrap();

    assert_eq!(200, response.status_code);
    let response_data: JsonCursoredData<
        PersonalizedData<GetProductSummaryData, ProductUserStateData>,
    > = serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert_eq!(Some(1), response_data.total);
    assert_eq!(expected.product_id, response_data.items[0].item.product_id);
}

#[localstack_test(services = [OpenSearch(), DynamoDB()])]
async fn should_200_when_following_search_after_from_previous_response_for_sort_price_asc() {
    let ddb_client = get_dynamodb_client().await;
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let notification_service = MockNotificationService::default();
    let search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
    let product_personalization_service = ProductPersonalizationServiceImpl::new(
        &watchlist_repository,
        &notification_service,
        &user_service,
        &search_filter_repository,
    );
    let opensearch_repository = ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let query_service = QueryProductServiceImpl::new(&opensearch_repository);
    let mut access_token_verifier_service = MockAccessTokenVerifierService::default();
    access_token_verifier_service
        .expect_verify_extract_user_id()
        .returning(|_| Box::pin(async { Ok(None) }));

    let search = ProductSearchData {
        language: common::language::data::LanguageData::De,
        currency: common::currency::data::CurrencyData::Eur,
        product_query: Some("Der erwartete Titel".try_into().unwrap()),
        shop_name_query: Default::default(),
        exclude_shop_name_query: Default::default(),
        seller_name_query: Default::default(),
        exclude_seller_name_query: Default::default(),
        shop_type_query: Default::default(),
        country_query: Default::default(),
        continent_query: Default::default(),
        geo_address_distance_query: None,
        price_query: None,
        state_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
        shop_slug_id_query: Default::default(),
        exclude_shop_slug_id_query: Default::default(),
        seller_slug_id_query: Default::default(),
        exclude_seller_slug_id_query: Default::default(),
    };

    let mut products = fake::vec![ProductDocument; 1370];
    for (idx, product) in products.iter_mut().enumerate() {
        product.title_de = Some("Der erwartete Titel".to_string());
        product.title_native = TextDocument {
            text: "Der erwartete Titel".to_string(),
            language: LanguageDocument::De,
        };
        product.price_eur = Some(1 + idx as u64);
    }
    let create_res = opensearch_repository
        .create_product_documents(products.clone())
        .await
        .unwrap();
    assert!(!create_res.errors);
    refresh_index("products").await;
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    let sorter = |l: &ProductDocument, r: &ProductDocument| match l
        .price_eur
        .unwrap()
        .cmp(&r.price_eur.unwrap())
    {
        std::cmp::Ordering::Equal => l.product_id.to_string().cmp(&r.product_id.to_string()),
        ord => ord,
    };
    products.sort_by(sorter);

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

    let response_1 = handle(
        lambda_event_1,
        &query_service,
        None,
        &access_token_verifier_service,
        &product_personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response_1.status_code);
    let json = extract_apigw_response_json_body!(response_1);
    let response_data: JsonCursoredData<
        PersonalizedData<GetProductSummaryData, ProductUserStateData>,
    > = serde_json::from_value(json).unwrap();
    assert_eq!(50, response_data.size);
    assert_eq!(1370, response_data.total.unwrap());
    assert_eq!(
        products
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

    let response_2 = handle(
        lambda_event_2,
        &query_service,
        None,
        &access_token_verifier_service,
        &product_personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response_2.status_code);
    let json_2 = extract_apigw_response_json_body!(response_2);
    let response_data_2: JsonCursoredData<
        PersonalizedData<GetProductSummaryData, ProductUserStateData>,
    > = serde_json::from_value(json_2).unwrap();
    assert_eq!(50, response_data_2.size);
    assert_eq!(1370, response_data_2.total.unwrap());
    assert_eq!(
        products
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
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let notification_service = MockNotificationService::default();
    let search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
    let product_personalization_service = ProductPersonalizationServiceImpl::new(
        &watchlist_repository,
        &notification_service,
        &user_service,
        &search_filter_repository,
    );
    let opensearch_repository = ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let query_service = QueryProductServiceImpl::new(&opensearch_repository);
    let mut access_token_verifier_service = MockAccessTokenVerifierService::default();
    access_token_verifier_service
        .expect_verify_extract_user_id()
        .returning(|_| Box::pin(async { Ok(None) }));

    let search = ProductSearchData {
        language: common::language::data::LanguageData::De,
        currency: common::currency::data::CurrencyData::Eur,
        product_query: Some("Der erwartete Titel".try_into().unwrap()),
        shop_name_query: Default::default(),
        exclude_shop_name_query: Default::default(),
        seller_name_query: Default::default(),
        exclude_seller_name_query: Default::default(),
        shop_type_query: Default::default(),
        country_query: Default::default(),
        continent_query: Default::default(),
        geo_address_distance_query: None,
        price_query: None,
        state_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
        shop_slug_id_query: Default::default(),
        exclude_shop_slug_id_query: Default::default(),
        seller_slug_id_query: Default::default(),
        exclude_seller_slug_id_query: Default::default(),
    };

    let mut products = fake::vec![ProductDocument; 1370];
    for (idx, product) in products.iter_mut().enumerate() {
        product.title_de = Some("Der erwartete Titel".to_string());
        product.title_native = TextDocument {
            text: "Der erwartete Titel".to_string(),
            language: LanguageDocument::De,
        };
        product.price_eur = Some(1 + idx as u64);
    }
    let create_res = opensearch_repository
        .create_product_documents(products.clone())
        .await
        .unwrap();
    assert!(!create_res.errors);
    refresh_index("products").await;
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

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
    products.sort_by(sorter);

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

    let response_1 = handle(
        lambda_event_1,
        &query_service,
        None,
        &access_token_verifier_service,
        &product_personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response_1.status_code);
    let json = extract_apigw_response_json_body!(response_1);
    let response_data: JsonCursoredData<
        PersonalizedData<GetProductSummaryData, ProductUserStateData>,
    > = serde_json::from_value(json).unwrap();
    assert_eq!(50, response_data.size);
    assert_eq!(1370, response_data.total.unwrap());
    assert_eq!(
        products
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

    let response_2 = handle(
        lambda_event_2,
        &query_service,
        None,
        &access_token_verifier_service,
        &product_personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response_2.status_code);
    let json_2 = extract_apigw_response_json_body!(response_2);
    let response_data_2: JsonCursoredData<
        PersonalizedData<GetProductSummaryData, ProductUserStateData>,
    > = serde_json::from_value(json_2).unwrap();
    assert_eq!(50, response_data_2.size);
    assert_eq!(1370, response_data_2.total.unwrap());
    assert_eq!(
        products
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
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let notification_service = MockNotificationService::default();
    let search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
    let product_personalization_service = ProductPersonalizationServiceImpl::new(
        &watchlist_repository,
        &notification_service,
        &user_service,
        &search_filter_repository,
    );
    let opensearch_repository = ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let query_service = QueryProductServiceImpl::new(&opensearch_repository);
    let mut access_token_verifier_service = MockAccessTokenVerifierService::default();
    access_token_verifier_service
        .expect_verify_extract_user_id()
        .returning(|_| Box::pin(async { Ok(None) }));

    let search = ProductSearchData {
        language: common::language::data::LanguageData::De,
        currency: common::currency::data::CurrencyData::Usd,
        product_query: Some("Der erwartete Titel".try_into().unwrap()),
        shop_name_query: Default::default(),
        exclude_shop_name_query: Default::default(),
        seller_name_query: Default::default(),
        exclude_seller_name_query: Default::default(),
        shop_type_query: Default::default(),
        country_query: Default::default(),
        continent_query: Default::default(),
        geo_address_distance_query: None,
        price_query: None,
        state_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
        shop_slug_id_query: Default::default(),
        exclude_shop_slug_id_query: Default::default(),
        seller_slug_id_query: Default::default(),
        exclude_seller_slug_id_query: Default::default(),
    };

    let mut products = fake::vec![ProductDocument; 1370];
    for product in &mut products {
        product.title_de = Some("Der erwartete Titel".to_string());
        product.title_native = TextDocument {
            text: "Der erwartete Titel".to_string(),
            language: LanguageDocument::De,
        };
    }
    let create_res = opensearch_repository
        .create_product_documents(products.clone())
        .await
        .unwrap();
    assert!(!create_res.errors);
    refresh_index("products").await;
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // first request
    let lambda_event_1 = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .query_string_parameter("size", "50")
            .body_serde(&search)
            .build(),
        context: Default::default(),
    };

    let response_1 = handle(
        lambda_event_1,
        &query_service,
        None,
        &access_token_verifier_service,
        &product_personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response_1.status_code);
    let json = extract_apigw_response_json_body!(response_1);
    let response_data: JsonCursoredData<
        PersonalizedData<GetProductSummaryData, ProductUserStateData>,
    > = serde_json::from_value(json).unwrap();
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

    let response_2 = handle(
        lambda_event_2,
        &query_service,
        None,
        &access_token_verifier_service,
        &product_personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response_2.status_code);
    let json_2 = extract_apigw_response_json_body!(response_2);
    let response_data_2: JsonCursoredData<
        PersonalizedData<GetProductSummaryData, ProductUserStateData>,
    > = serde_json::from_value(json_2).unwrap();
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
async fn should_200_when_following_search_after_for_native_hybrid_product_api() {
    let ddb_client = get_dynamodb_client().await;
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let notification_service = MockNotificationService::default();
    let search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
    let product_personalization_service = ProductPersonalizationServiceImpl::new(
        &watchlist_repository,
        &notification_service,
        &user_service,
        &search_filter_repository,
    );
    let opensearch_repository = ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let query_service = QueryProductServiceImpl::new(&opensearch_repository);
    let mut access_token_verifier_service = MockAccessTokenVerifierService::default();
    access_token_verifier_service
        .expect_verify_extract_user_id()
        .returning(|_| Box::pin(async { Ok(None) }));
    let mut embedding_service = MockMultimodalEmbeddingService::default();
    embedding_service
        .expect_embed_query()
        .times(3)
        .returning(|_| Box::pin(async { Ok(one_hot_embedding(0)) }));

    let search = ProductSearchData {
        language: LanguageData::En,
        currency: CurrencyData::Eur,
        product_query: Some("art deco lamp".try_into().unwrap()),
        shop_name_query: Default::default(),
        exclude_shop_name_query: Default::default(),
        seller_name_query: Default::default(),
        exclude_seller_name_query: Default::default(),
        shop_type_query: Default::default(),
        country_query: Default::default(),
        continent_query: Default::default(),
        geo_address_distance_query: None,
        price_query: None,
        state_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
        shop_slug_id_query: Default::default(),
        exclude_shop_slug_id_query: Default::default(),
        seller_slug_id_query: Default::default(),
        exclude_seller_slug_id_query: Default::default(),
    };

    let mut products = fake::vec![ProductDocument; 75];
    for product in &mut products {
        product.title_en = Some("art deco lamp".to_string());
        product.title_native = TextDocument {
            text: "art deco lamp".to_string(),
            language: LanguageDocument::En,
        };
        product.embedding = Some(one_hot_embedding(0));
    }
    let create_res = opensearch_repository
        .create_product_documents(products)
        .await
        .unwrap();
    assert!(!create_res.errors);
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_secs(3)).await;

    let response_1 = handle(
        LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .query_string_parameter("size", "30")
                .query_string_parameter("sort", "score")
                .query_string_parameter("order", "desc")
                .body_serde(&search)
                .build(),
            context: Default::default(),
        },
        &query_service,
        Some(&embedding_service),
        &access_token_verifier_service,
        &product_personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response_1.status_code);
    let response_data_1: JsonCursoredData<
        PersonalizedData<GetProductSummaryData, ProductUserStateData>,
    > = serde_json::from_value(extract_apigw_response_json_body!(response_1)).unwrap();
    assert_eq!(30, response_data_1.size);
    assert_eq!(30, response_data_1.items.len());
    assert!(response_data_1.total.is_none());
    let search_after_1 = response_data_1.search_after.clone().unwrap();
    assert!(search_after_1.is_array());

    let ids_1: HashSet<_> = response_data_1
        .items
        .iter()
        .map(|item| item.item.product_id)
        .collect();

    let response_2 = handle(
        LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .query_string_parameter("size", "30")
                .query_string_parameter("sort", "score")
                .query_string_parameter("order", "desc")
                .query_string_parameter(
                    "searchAfter",
                    serde_json::to_string(&search_after_1).unwrap(),
                )
                .body_serde(&search)
                .build(),
            context: Default::default(),
        },
        &query_service,
        Some(&embedding_service),
        &access_token_verifier_service,
        &product_personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response_2.status_code);
    let response_data_2: JsonCursoredData<
        PersonalizedData<GetProductSummaryData, ProductUserStateData>,
    > = serde_json::from_value(extract_apigw_response_json_body!(response_2)).unwrap();
    assert_eq!(30, response_data_2.size);
    assert_eq!(30, response_data_2.items.len());
    assert!(response_data_2.total.is_none());
    let search_after_2 = response_data_2.search_after.clone().unwrap();
    assert!(search_after_2.is_array());
    assert_ne!(search_after_1, search_after_2);

    let ids_2: HashSet<_> = response_data_2
        .items
        .iter()
        .map(|item| item.item.product_id)
        .collect();
    assert!(ids_1.is_disjoint(&ids_2));

    let response_3 = handle(
        LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .query_string_parameter("size", "30")
                .query_string_parameter("sort", "score")
                .query_string_parameter("order", "desc")
                .query_string_parameter(
                    "searchAfter",
                    serde_json::to_string(&search_after_2).unwrap(),
                )
                .body_serde(&search)
                .build(),
            context: Default::default(),
        },
        &query_service,
        Some(&embedding_service),
        &access_token_verifier_service,
        &product_personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response_3.status_code);
    let response_data_3: JsonCursoredData<
        PersonalizedData<GetProductSummaryData, ProductUserStateData>,
    > = serde_json::from_value(extract_apigw_response_json_body!(response_3)).unwrap();
    assert_eq!(15, response_data_3.size);
    assert_eq!(15, response_data_3.items.len());
    assert!(response_data_3.total.is_none());
    assert!(response_data_3.search_after.is_none());

    let ids_3: HashSet<_> = response_data_3
        .items
        .iter()
        .map(|item| item.item.product_id)
        .collect();
    assert!(ids_1.is_disjoint(&ids_3));
    assert!(ids_2.is_disjoint(&ids_3));
    assert_eq!(75, ids_1.len() + ids_2.len() + ids_3.len());
}

#[localstack_test(services = [OpenSearch(), DynamoDB()])]
async fn should_200_when_following_search_after_from_previous_response_for_explicit_sort_score() {
    let ddb_client = get_dynamodb_client().await;
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let notification_service = MockNotificationService::default();
    let search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
    let product_personalization_service = ProductPersonalizationServiceImpl::new(
        &watchlist_repository,
        &notification_service,
        &user_service,
        &search_filter_repository,
    );
    let opensearch_repository = ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let query_service = QueryProductServiceImpl::new(&opensearch_repository);
    let mut access_token_verifier_service = MockAccessTokenVerifierService::default();
    access_token_verifier_service
        .expect_verify_extract_user_id()
        .returning(|_| Box::pin(async { Ok(None) }));

    let search = ProductSearchData {
        language: common::language::data::LanguageData::De,
        currency: common::currency::data::CurrencyData::Usd,
        product_query: Some("Der erwartete Titel".try_into().unwrap()),
        shop_name_query: Default::default(),
        exclude_shop_name_query: Default::default(),
        seller_name_query: Default::default(),
        exclude_seller_name_query: Default::default(),
        shop_type_query: Default::default(),
        country_query: Default::default(),
        continent_query: Default::default(),
        geo_address_distance_query: None,
        price_query: None,
        state_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
        shop_slug_id_query: Default::default(),
        exclude_shop_slug_id_query: Default::default(),
        seller_slug_id_query: Default::default(),
        exclude_seller_slug_id_query: Default::default(),
    };

    let mut products = fake::vec![ProductDocument; 1370];
    for product in &mut products {
        product.title_de = Some("Der erwartete Titel".to_string());
        product.title_native = TextDocument {
            text: "Der erwartete Titel".to_string(),
            language: LanguageDocument::De,
        };
    }
    let create_res = opensearch_repository
        .create_product_documents(products.clone())
        .await
        .unwrap();
    assert!(!create_res.errors);
    refresh_index("products").await;
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

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

    let response_1 = handle(
        lambda_event_1,
        &query_service,
        None,
        &access_token_verifier_service,
        &product_personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response_1.status_code);
    let json = extract_apigw_response_json_body!(response_1);
    let response_data: JsonCursoredData<
        PersonalizedData<GetProductSummaryData, ProductUserStateData>,
    > = serde_json::from_value(json).unwrap();
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

    let response_2 = handle(
        lambda_event_2,
        &query_service,
        None,
        &access_token_verifier_service,
        &product_personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response_2.status_code);
    let json_2 = extract_apigw_response_json_body!(response_2);
    let response_data_2: JsonCursoredData<
        PersonalizedData<GetProductSummaryData, ProductUserStateData>,
    > = serde_json::from_value(json_2).unwrap();
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
#[trace]
#[localstack_test(services = [OpenSearch(), DynamoDB()])]
async fn should_200_when_created_query(
    #[case] min: Option<OffsetDateTime>,
    #[case] max: Option<OffsetDateTime>,
) {
    let ddb_client = get_dynamodb_client().await;
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let notification_service = MockNotificationService::default();
    let search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
    let product_personalization_service = ProductPersonalizationServiceImpl::new(
        &watchlist_repository,
        &notification_service,
        &user_service,
        &search_filter_repository,
    );
    let opensearch_repository = ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let query_service = QueryProductServiceImpl::new(&opensearch_repository);
    let mut access_token_verifier_service = MockAccessTokenVerifierService::default();
    access_token_verifier_service
        .expect_verify_extract_user_id()
        .returning(|_| Box::pin(async { Ok(None) }));

    let created = RangeQuery { min, max };
    let search = ProductSearchData {
        language: common::language::data::LanguageData::De,
        currency: common::currency::data::CurrencyData::Eur,
        product_query: Some("Der erwartete Titel".try_into().unwrap()),
        shop_name_query: Default::default(),
        exclude_shop_name_query: Default::default(),
        seller_name_query: Default::default(),
        exclude_seller_name_query: Default::default(),
        shop_type_query: Default::default(),
        country_query: Default::default(),
        continent_query: Default::default(),
        geo_address_distance_query: None,
        price_query: None,
        state_query: Default::default(),
        created_query: Some(created),
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
        shop_slug_id_query: Default::default(),
        exclude_shop_slug_id_query: Default::default(),
        seller_slug_id_query: Default::default(),
        exclude_seller_slug_id_query: Default::default(),
    };
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .body_serde(&search)
            .build(),
        context: Default::default(),
    };

    let mut products = fake::vec![ProductDocument; 1370];
    for product in &mut products {
        product.title_de = Some("Der erwartete Titel".to_string());
        product.title_native = TextDocument {
            text: "Der erwartete Titel".to_string(),
            language: LanguageDocument::De,
        };
    }
    let create_res = opensearch_repository
        .create_product_documents(products.clone())
        .await
        .unwrap();
    assert!(!create_res.errors);
    refresh_index("products").await;
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    let response = handle(
        lambda_event,
        &query_service,
        None,
        &access_token_verifier_service,
        &product_personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response.status_code);

    let json = extract_apigw_response_json_body!(response);
    let response_data: JsonCursoredData<
        PersonalizedData<GetProductSummaryData, ProductUserStateData>,
    > = serde_json::from_value(json).unwrap();
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
#[trace]
#[localstack_test(services = [OpenSearch(), DynamoDB()])]
async fn should_200_when_updated_query(
    #[case] min: Option<OffsetDateTime>,
    #[case] max: Option<OffsetDateTime>,
) {
    let ddb_client = get_dynamodb_client().await;
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let notification_service = MockNotificationService::default();
    let search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
    let product_personalization_service = ProductPersonalizationServiceImpl::new(
        &watchlist_repository,
        &notification_service,
        &user_service,
        &search_filter_repository,
    );
    let opensearch_repository = ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let query_service = QueryProductServiceImpl::new(&opensearch_repository);
    let mut access_token_verifier_service = MockAccessTokenVerifierService::default();
    access_token_verifier_service
        .expect_verify_extract_user_id()
        .returning(|_| Box::pin(async { Ok(None) }));

    let updated = RangeQuery { min, max };
    let search = ProductSearchData {
        language: common::language::data::LanguageData::De,
        currency: common::currency::data::CurrencyData::Eur,
        product_query: Some("Der erwartete Titel".try_into().unwrap()),
        shop_name_query: Default::default(),
        exclude_shop_name_query: Default::default(),
        seller_name_query: Default::default(),
        exclude_seller_name_query: Default::default(),
        shop_type_query: Default::default(),
        country_query: Default::default(),
        continent_query: Default::default(),
        geo_address_distance_query: None,
        price_query: None,
        state_query: Default::default(),
        created_query: None,
        updated_query: Some(updated),
        auction_start_query: None,
        auction_end_query: None,
        shop_slug_id_query: Default::default(),
        exclude_shop_slug_id_query: Default::default(),
        seller_slug_id_query: Default::default(),
        exclude_seller_slug_id_query: Default::default(),
    };
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .body_serde(&search)
            .build(),
        context: Default::default(),
    };

    let mut products = fake::vec![ProductDocument; 1370];
    for product in &mut products {
        product.title_de = Some("Der erwartete Titel".to_string());
        product.title_native = TextDocument {
            text: "Der erwartete Titel".to_string(),
            language: LanguageDocument::De,
        };
    }
    let create_res = opensearch_repository
        .create_product_documents(products.clone())
        .await
        .unwrap();
    assert!(!create_res.errors);
    refresh_index("products").await;
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    let response = handle(
        lambda_event,
        &query_service,
        None,
        &access_token_verifier_service,
        &product_personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response.status_code);

    let json = extract_apigw_response_json_body!(response);
    let response_data: JsonCursoredData<
        PersonalizedData<GetProductSummaryData, ProductUserStateData>,
    > = serde_json::from_value(json).unwrap();
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
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let mut notification_service = MockNotificationService::default();
    notification_service
        .expect_find_notifications_by_product()
        .returning(|_, _, _, _| Box::pin(async { Ok(vec![]) }));
    let mut search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
    search_filter_repository
        .expect_query_user_search_filter_match_records_all()
        .returning(|_| Box::pin(async { Ok(vec![]) }));
    let product_personalization_service = ProductPersonalizationServiceImpl::new(
        &watchlist_repository,
        &notification_service,
        &user_service,
        &search_filter_repository,
    );
    let opensearch_repository = ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let query_service = QueryProductServiceImpl::new(&opensearch_repository);
    let mut access_token_verifier_service = MockAccessTokenVerifierService::default();
    let user_id = UserId::new();
    let user_ctx = common::actor::RequestContext {
        actor: common::actor::domain::Actor::User(user_id),
    };
    user_service
        .create_user(
            &user_ctx,
            user::service::command::CreateUserCommand {
                id: user_id,
                email: "foo@bar.de".try_into().unwrap(),
            },
        )
        .await
        .unwrap();
    access_token_verifier_service
        .expect_verify_extract_user_id()
        .returning(move |_| Box::pin(async move { Ok(Some(user_id)) }));

    let search = ProductSearchData {
        language: common::language::data::LanguageData::De,
        currency: common::currency::data::CurrencyData::Eur,
        product_query: Some("Der erwartete Titel".try_into().unwrap()),
        shop_name_query: Default::default(),
        exclude_shop_name_query: Default::default(),
        seller_name_query: Default::default(),
        exclude_seller_name_query: Default::default(),
        shop_type_query: Default::default(),
        country_query: Default::default(),
        continent_query: Default::default(),
        geo_address_distance_query: None,
        price_query: None,
        state_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
        shop_slug_id_query: Default::default(),
        exclude_shop_slug_id_query: Default::default(),
        seller_slug_id_query: Default::default(),
        exclude_seller_slug_id_query: Default::default(),
    };
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .body_serde(&search)
            .build(),
        context: Default::default(),
    };

    let mut products = fake::vec![ProductDocument; 1370];
    for product in &mut products {
        product.title_de = Some("Der erwartete Titel".to_string());
        product.title_native = TextDocument {
            text: "Der erwartete Titel".to_string(),
            language: LanguageDocument::De,
        };
    }
    let create_res = opensearch_repository
        .create_product_documents(products.clone())
        .await
        .unwrap();
    assert!(!create_res.errors);
    refresh_index("products").await;
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    let response = handle(
        lambda_event,
        &query_service,
        None,
        &access_token_verifier_service,
        &product_personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response.status_code);

    let json = extract_apigw_response_json_body!(response);
    assert!(json["items"][0]["userState"].is_object());
    let response_data: JsonCursoredData<
        PersonalizedData<GetProductSummaryData, ProductUserStateData>,
    > = serde_json::from_value(json).unwrap();
    assert!(response_data.items.iter().all(|item| {
        let user_state = item.user_state.clone().unwrap();
        !user_state.watchlist.notifications && !user_state.watchlist.watching
    }));
}

#[rstest::rstest]
#[test_attr(apply(test))]
#[case("de", "German title", Language::De)]
#[case("en", "English title", Language::En)]
#[case("fr", "French title", Language::Fr)]
#[case("es", "Spanish title", Language::Es)]
#[case("it", "Italian title", Language::It)]
#[trace]
#[localstack_test(services = [OpenSearch(), DynamoDB()])]
async fn should_respond_200_and_respect_language_query_param(
    #[case] _language_query: &str,
    #[case] expected_title: &str,
    #[case] expected_title_lang: Language,
) {
    let ddb_client = get_dynamodb_client().await;
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let notification_service = MockNotificationService::default();
    let search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
    let product_personalization_service = ProductPersonalizationServiceImpl::new(
        &watchlist_repository,
        &notification_service,
        &user_service,
        &search_filter_repository,
    );
    let opensearch_repository = ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let query_service = QueryProductServiceImpl::new(&opensearch_repository);
    let mut access_token_verifier_service = MockAccessTokenVerifierService::default();
    access_token_verifier_service
        .expect_verify_extract_user_id()
        .returning(|_| Box::pin(async { Ok(None) }));

    let mut document = Faker.fake::<ProductDocument>();
    document.title_native = TextDocument {
        text: "German title".to_string(),
        language: LanguageDocument::De,
    };
    document.title_de = Some("German title".to_string());
    document.title_en = Some("English title".to_string());
    document.title_fr = Some("French title".to_string());
    document.title_es = Some("Spanish title".to_string());
    document.title_it = Some("Italian title".to_string());
    let create_res = opensearch_repository
        .create_product_documents(vec![document])
        .await
        .unwrap();
    assert!(!create_res.errors);
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_secs(3)).await;

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .body_serde(&ProductSearchData {
                language: expected_title_lang.into(),
                currency: CurrencyData::Eur,
                product_query: Some(expected_title.try_into().unwrap()),
                shop_name_query: Default::default(),
                exclude_shop_name_query: Default::default(),
                seller_name_query: Default::default(),
                exclude_seller_name_query: Default::default(),
                shop_type_query: Default::default(),
                country_query: Default::default(),
                continent_query: Default::default(),
                geo_address_distance_query: None,
                price_query: None,
                state_query: Default::default(),
                created_query: None,
                updated_query: None,
                auction_start_query: None,
                auction_end_query: None,
                shop_slug_id_query: Default::default(),
                exclude_shop_slug_id_query: Default::default(),
                seller_slug_id_query: Default::default(),
                exclude_seller_slug_id_query: Default::default(),
            })
            .build(),
        context: Default::default(),
    };

    let response = handle(
        lambda_event,
        &query_service,
        None,
        &access_token_verifier_service,
        &product_personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response.status_code);

    let json = extract_apigw_response_json_body!(response);
    let response_data: JsonCursoredData<
        PersonalizedData<GetProductSummaryData, ProductUserStateData>,
    > = serde_json::from_value(json).unwrap();
    assert_eq!(1, response_data.total.unwrap());

    let actual = response_data.items.first().unwrap().item.clone();
    assert_eq!(expected_title_lang, actual.title.language.into(),);
    assert_eq!(expected_title, actual.title.text,);
}

#[rstest::rstest]
#[test_attr(apply(test))]
#[case([ShopTypeData::AuctionHouse].into())]
#[case([ShopTypeData::CommercialDealer].into())]
#[case([ShopTypeData::Marketplace].into())]
#[case([ShopTypeData::AuctionHouse, ShopTypeData::CommercialDealer].into())]
#[trace]
#[localstack_test(services = [OpenSearch(), DynamoDB()])]
async fn should_200_when_shop_type_query(#[case] query: HashSet<ShopTypeData>) {
    let ddb_client = get_dynamodb_client().await;
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let notification_service = MockNotificationService::default();
    let search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
    let product_personalization_service = ProductPersonalizationServiceImpl::new(
        &watchlist_repository,
        &notification_service,
        &user_service,
        &search_filter_repository,
    );
    let opensearch_repository = ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let query_service = QueryProductServiceImpl::new(&opensearch_repository);
    let mut access_token_verifier_service = MockAccessTokenVerifierService::default();
    access_token_verifier_service
        .expect_verify_extract_user_id()
        .returning(|_| Box::pin(async { Ok(None) }));

    let search = ProductSearchData {
        language: common::language::data::LanguageData::De,
        currency: common::currency::data::CurrencyData::Eur,
        product_query: Some("Der erwartete Titel".try_into().unwrap()),
        shop_name_query: Default::default(),
        exclude_shop_name_query: Default::default(),
        seller_name_query: Default::default(),
        exclude_seller_name_query: Default::default(),
        shop_type_query: query.clone(),
        country_query: Default::default(),
        continent_query: Default::default(),
        geo_address_distance_query: None,
        price_query: None,
        state_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
        shop_slug_id_query: Default::default(),
        exclude_shop_slug_id_query: Default::default(),
        seller_slug_id_query: Default::default(),
        exclude_seller_slug_id_query: Default::default(),
    };
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .body_serde(&search)
            .build(),
        context: Default::default(),
    };

    let mut products = fake::vec![ProductDocument; 1370];
    for product in &mut products {
        product.title_de = Some("Der erwartete Titel".to_string());
        product.title_native = TextDocument {
            text: "Der erwartete Titel".to_string(),
            language: LanguageDocument::De,
        };
    }
    let create_res = opensearch_repository
        .create_product_documents(products.clone())
        .await
        .unwrap();
    assert!(!create_res.errors);
    refresh_index("products").await;
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    let response = handle(
        lambda_event,
        &query_service,
        None,
        &access_token_verifier_service,
        &product_personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response.status_code);

    let json = extract_apigw_response_json_body!(response);
    let response_data: JsonCursoredData<
        PersonalizedData<GetProductSummaryData, ProductUserStateData>,
    > = serde_json::from_value(json).unwrap();
    assert!(!response_data.items.is_empty());
    assert!(
        response_data
            .items
            .iter()
            .map(|item| item.item.shop_type)
            .all(|actual| query.contains(&actual))
    );
}

#[rstest::rstest]
#[test_attr(apply(test))]
#[case(["Sotheby's"].into())]
#[case(["Christie's"].into())]
#[case(["Heritage Auctions"].into())]
#[case(["Sotheby's", "Christie's"].into())]
#[case(["Sotheby's", "Christie's", "Heritage Auctions"].into())]
#[trace]
#[localstack_test(services = [OpenSearch(), DynamoDB()])]
async fn should_200_when_shop_name_query_for_keyword_filter(#[case] query: HashSet<&str>) {
    let ddb_client = get_dynamodb_client().await;
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let notification_service = MockNotificationService::default();
    let search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
    let product_personalization_service = ProductPersonalizationServiceImpl::new(
        &watchlist_repository,
        &notification_service,
        &user_service,
        &search_filter_repository,
    );
    let opensearch_repository = ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let query_service = QueryProductServiceImpl::new(&opensearch_repository);
    let mut access_token_verifier_service = MockAccessTokenVerifierService::default();
    access_token_verifier_service
        .expect_verify_extract_user_id()
        .returning(|_| Box::pin(async { Ok(None) }));

    let search = ProductSearchData {
        language: common::language::data::LanguageData::De,
        currency: common::currency::data::CurrencyData::Eur,
        product_query: Some("Der erwartete Titel".try_into().unwrap()),
        shop_name_query: query.iter().map(|s| s.to_string().into()).collect(),
        exclude_shop_name_query: Default::default(),
        seller_name_query: Default::default(),
        exclude_seller_name_query: Default::default(),
        shop_type_query: Default::default(),
        country_query: Default::default(),
        continent_query: Default::default(),
        geo_address_distance_query: None,
        price_query: None,
        state_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
        shop_slug_id_query: Default::default(),
        exclude_shop_slug_id_query: Default::default(),
        seller_slug_id_query: Default::default(),
        exclude_seller_slug_id_query: Default::default(),
    };
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .body_serde(&search)
            .build(),
        context: Default::default(),
    };

    let shop_names_vec: Vec<&str> = query.iter().copied().collect();
    let mut products_with_target_shops = fake::vec![ProductDocument; 685];
    for (idx, product) in products_with_target_shops.iter_mut().enumerate() {
        product.title_de = Some("Der erwartete Titel".to_string());
        product.shop_name = shop_names_vec[idx % shop_names_vec.len()].to_string();
    }

    let mut products_with_other_shops = fake::vec![ProductDocument; 685];
    for product in &mut products_with_other_shops {
        product.title_de = Some("Der erwartete Titel".to_string());
        product.shop_name = "Other Auction House".to_string();
    }

    let all_products = [products_with_target_shops, products_with_other_shops].concat();
    let create_res = opensearch_repository
        .create_product_documents(all_products)
        .await
        .unwrap();
    assert!(!create_res.errors);
    refresh_index("products").await;
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    let response = handle(
        lambda_event,
        &query_service,
        None,
        &access_token_verifier_service,
        &product_personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response.status_code);

    let json = extract_apigw_response_json_body!(response);
    let response_data: JsonCursoredData<
        PersonalizedData<GetProductSummaryData, ProductUserStateData>,
    > = serde_json::from_value(json).unwrap();
    assert!(!response_data.items.is_empty());
    assert_eq!(685, response_data.total.unwrap());
    assert!(
        response_data
            .items
            .iter()
            .map(|item| item.item.shop_name.as_str())
            .all(|actual| query.contains(actual))
    );
}

#[rstest::rstest]
#[test_attr(apply(test))]
#[case(["Sotheby's"].into())]
#[case(["Christie's"].into())]
#[case(["Heritage Auctions"].into())]
#[case(["Sotheby's", "Christie's"].into())]
#[case(["Sotheby's", "Christie's", "Heritage Auctions"].into())]
#[trace]
#[localstack_test(services = [OpenSearch(), DynamoDB()])]
async fn should_200_when_exclude_shop_name_query(#[case] query: HashSet<&str>) {
    let ddb_client = get_dynamodb_client().await;
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let notification_service = MockNotificationService::default();
    let search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
    let product_personalization_service = ProductPersonalizationServiceImpl::new(
        &watchlist_repository,
        &notification_service,
        &user_service,
        &search_filter_repository,
    );
    let opensearch_repository = ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let query_service = QueryProductServiceImpl::new(&opensearch_repository);
    let mut access_token_verifier_service = MockAccessTokenVerifierService::default();
    access_token_verifier_service
        .expect_verify_extract_user_id()
        .returning(|_| Box::pin(async { Ok(None) }));

    let search = ProductSearchData {
        language: common::language::data::LanguageData::De,
        currency: common::currency::data::CurrencyData::Eur,
        product_query: Some("Der erwartete Titel".try_into().unwrap()),
        shop_name_query: Default::default(),
        exclude_shop_name_query: query.iter().map(|s| s.to_string().into()).collect(),
        seller_name_query: Default::default(),
        exclude_seller_name_query: Default::default(),
        shop_type_query: Default::default(),
        country_query: Default::default(),
        continent_query: Default::default(),
        geo_address_distance_query: None,
        price_query: None,
        state_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
        shop_slug_id_query: Default::default(),
        exclude_shop_slug_id_query: Default::default(),
        seller_slug_id_query: Default::default(),
        exclude_seller_slug_id_query: Default::default(),
    };
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .body_serde(&search)
            .build(),
        context: Default::default(),
    };

    let shop_names_vec: Vec<&str> = query.iter().copied().collect();
    let mut products_with_target_shops = fake::vec![ProductDocument; 685];
    for (idx, product) in products_with_target_shops.iter_mut().enumerate() {
        product.title_de = Some("Der erwartete Titel".to_string());
        product.shop_name = shop_names_vec[idx % shop_names_vec.len()].to_string();
    }

    let mut products_with_other_shops = fake::vec![ProductDocument; 685];
    for product in &mut products_with_other_shops {
        product.title_de = Some("Der erwartete Titel".to_string());
        product.shop_name = "Other Auction House".to_string();
    }

    let all_products = [products_with_target_shops, products_with_other_shops].concat();
    let create_res = opensearch_repository
        .create_product_documents(all_products)
        .await
        .unwrap();
    assert!(!create_res.errors);
    refresh_index("products").await;
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    let response = handle(
        lambda_event,
        &query_service,
        None,
        &access_token_verifier_service,
        &product_personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response.status_code);

    let json = extract_apigw_response_json_body!(response);
    let response_data: JsonCursoredData<
        PersonalizedData<GetProductSummaryData, ProductUserStateData>,
    > = serde_json::from_value(json).unwrap();
    assert!(!response_data.items.is_empty());
    assert_eq!(685, response_data.total.unwrap());
    assert!(
        response_data
            .items
            .iter()
            .map(|item| item.item.shop_name.as_str())
            .all(|actual| !query.contains(actual))
    );
}

#[localstack_test(services = [OpenSearch(), DynamoDB()])]
async fn should_200_when_auction_start_range_is_given() {
    let ddb_client = get_dynamodb_client().await;
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let notification_service = MockNotificationService::default();
    let search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
    let product_personalization_service = ProductPersonalizationServiceImpl::new(
        &watchlist_repository,
        &notification_service,
        &user_service,
        &search_filter_repository,
    );
    let opensearch_repository = ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let query_service = QueryProductServiceImpl::new(&opensearch_repository);
    let mut access_token_verifier_service = MockAccessTokenVerifierService::default();
    access_token_verifier_service
        .expect_verify_extract_user_id()
        .returning(|_| Box::pin(async { Ok(None) }));

    let search = ProductSearchData {
        language: LanguageData::De,
        currency: CurrencyData::Eur,
        product_query: Some("Auction test product".try_into().unwrap()),
        shop_name_query: Default::default(),
        exclude_shop_name_query: Default::default(),
        seller_name_query: Default::default(),
        exclude_seller_name_query: Default::default(),
        shop_type_query: Default::default(),
        country_query: Default::default(),
        continent_query: Default::default(),
        geo_address_distance_query: None,
        price_query: None,
        state_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: Some(RangeQuery {
            min: Some(datetime!(2026-01-01 0:00 UTC)),
            max: Some(datetime!(2026-03-31 23:59 UTC)),
        }),
        auction_end_query: None,
        shop_slug_id_query: Default::default(),
        exclude_shop_slug_id_query: Default::default(),
        seller_slug_id_query: Default::default(),
        exclude_seller_slug_id_query: Default::default(),
    };

    let mut early_products = fake::vec![ProductDocument; 30];
    for product in &mut early_products {
        product.title_de = Some("Auction test product".to_string());
        product.auction_start = Some(datetime!(2026-02-15 10:00 UTC));
        product.auction_end = Some(datetime!(2026-02-15 14:00 UTC));
    }

    let mut late_products = fake::vec![ProductDocument; 30];
    for product in &mut late_products {
        product.title_de = Some("Auction test product".to_string());
        product.auction_start = Some(datetime!(2026-06-20 10:00 UTC));
        product.auction_end = Some(datetime!(2026-06-20 14:00 UTC));
    }

    let all_products = [early_products.clone(), late_products].concat();
    let create_res = opensearch_repository
        .create_product_documents(all_products)
        .await
        .unwrap();
    assert!(!create_res.errors);
    refresh_index("products").await;
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .query_string_parameter("size", "50")
            .body_serde(&search)
            .build(),
        context: Default::default(),
    };

    let response = handle(
        lambda_event,
        &query_service,
        None,
        &access_token_verifier_service,
        &product_personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response.status_code);

    let json = extract_apigw_response_json_body!(response);
    let response_data: JsonCursoredData<
        PersonalizedData<GetProductSummaryData, ProductUserStateData>,
    > = serde_json::from_value(json).unwrap();
    assert_eq!(30, response_data.items.len());
}

#[localstack_test(services = [OpenSearch(), DynamoDB()])]
async fn should_200_when_auction_end_range_is_given() {
    let ddb_client = get_dynamodb_client().await;
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let notification_service = MockNotificationService::default();
    let search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
    let product_personalization_service = ProductPersonalizationServiceImpl::new(
        &watchlist_repository,
        &notification_service,
        &user_service,
        &search_filter_repository,
    );
    let opensearch_repository = ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let query_service = QueryProductServiceImpl::new(&opensearch_repository);
    let mut access_token_verifier_service = MockAccessTokenVerifierService::default();
    access_token_verifier_service
        .expect_verify_extract_user_id()
        .returning(|_| Box::pin(async { Ok(None) }));

    let search = ProductSearchData {
        language: LanguageData::De,
        currency: CurrencyData::Eur,
        product_query: Some("Auction end test".try_into().unwrap()),
        shop_name_query: Default::default(),
        exclude_shop_name_query: Default::default(),
        seller_name_query: Default::default(),
        exclude_seller_name_query: Default::default(),
        shop_type_query: Default::default(),
        country_query: Default::default(),
        continent_query: Default::default(),
        geo_address_distance_query: None,
        price_query: None,
        state_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: Some(RangeQuery {
            min: None,
            max: Some(datetime!(2026-02-28 23:59 UTC)),
        }),
        shop_slug_id_query: Default::default(),
        exclude_shop_slug_id_query: Default::default(),
        seller_slug_id_query: Default::default(),
        exclude_seller_slug_id_query: Default::default(),
    };

    let mut early_products = fake::vec![ProductDocument; 25];
    for product in &mut early_products {
        product.title_de = Some("Auction end test".to_string());
        product.auction_start = Some(datetime!(2026-01-15 10:00 UTC));
        product.auction_end = Some(datetime!(2026-01-15 14:00 UTC));
    }

    let mut late_products = fake::vec![ProductDocument; 25];
    for product in &mut late_products {
        product.title_de = Some("Auction end test".to_string());
        product.auction_start = Some(datetime!(2026-05-10 10:00 UTC));
        product.auction_end = Some(datetime!(2026-05-10 14:00 UTC));
    }

    let all_products = [early_products.clone(), late_products].concat();
    let create_res = opensearch_repository
        .create_product_documents(all_products)
        .await
        .unwrap();
    assert!(!create_res.errors);
    refresh_index("products").await;
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .query_string_parameter("size", "50")
            .body_serde(&search)
            .build(),
        context: Default::default(),
    };

    let response = handle(
        lambda_event,
        &query_service,
        None,
        &access_token_verifier_service,
        &product_personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response.status_code);

    let json = extract_apigw_response_json_body!(response);
    let response_data: JsonCursoredData<
        PersonalizedData<GetProductSummaryData, ProductUserStateData>,
    > = serde_json::from_value(json).unwrap();
    assert_eq!(25, response_data.items.len());
}

#[rstest::rstest]
#[test_attr(apply(test))]
#[case(["Sotheby's"].into())]
#[case(["Sotheby's", "Christie's"].into())]
#[trace]
#[localstack_test(services = [OpenSearch(), DynamoDB()])]
async fn should_200_when_seller_name_query_for_keyword_filter(#[case] query: HashSet<&str>) {
    let ddb_client = get_dynamodb_client().await;
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let notification_service = MockNotificationService::default();
    let search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
    let product_personalization_service = ProductPersonalizationServiceImpl::new(
        &watchlist_repository,
        &notification_service,
        &user_service,
        &search_filter_repository,
    );
    let opensearch_repository = ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let query_service = QueryProductServiceImpl::new(&opensearch_repository);
    let mut access_token_verifier_service = MockAccessTokenVerifierService::default();
    access_token_verifier_service
        .expect_verify_extract_user_id()
        .returning(|_| Box::pin(async { Ok(None) }));

    let search = ProductSearchData {
        language: LanguageData::De,
        currency: CurrencyData::Eur,
        product_query: Some("Seller name keyword filter test".try_into().unwrap()),
        shop_name_query: Default::default(),
        exclude_shop_name_query: Default::default(),
        seller_name_query: query.iter().map(|s| s.to_string().into()).collect(),
        exclude_seller_name_query: Default::default(),
        shop_type_query: Default::default(),
        country_query: Default::default(),
        continent_query: Default::default(),
        geo_address_distance_query: None,
        price_query: None,
        state_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
        shop_slug_id_query: Default::default(),
        exclude_shop_slug_id_query: Default::default(),
        seller_slug_id_query: Default::default(),
        exclude_seller_slug_id_query: Default::default(),
    };

    let seller_names_vec: Vec<&str> = query.iter().copied().collect();

    let mut products_with_target_sellers = fake::vec![ProductDocument; 685];
    for (idx, product) in products_with_target_sellers.iter_mut().enumerate() {
        product.title_de = Some("Seller name keyword filter test".to_string());
        product.seller_name = seller_names_vec[idx % seller_names_vec.len()].to_string();
    }

    let mut products_with_other_sellers = fake::vec![ProductDocument; 685];
    for product in &mut products_with_other_sellers {
        product.title_de = Some("Seller name keyword filter test".to_string());
        product.seller_name = "Other Seller House".to_string();
    }

    let all_products = [products_with_target_sellers, products_with_other_sellers].concat();
    let create_res = opensearch_repository
        .create_product_documents(all_products)
        .await
        .unwrap();
    assert!(!create_res.errors);
    refresh_index("products").await;
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .body_serde(&search)
            .build(),
        context: Default::default(),
    };

    let response = handle(
        lambda_event,
        &query_service,
        None,
        &access_token_verifier_service,
        &product_personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response.status_code);

    let json = extract_apigw_response_json_body!(response);
    let response_data: JsonCursoredData<
        PersonalizedData<GetProductSummaryData, ProductUserStateData>,
    > = serde_json::from_value(json).unwrap();
    assert!(!response_data.items.is_empty());
    assert_eq!(685, response_data.total.unwrap());
    assert!(
        response_data
            .items
            .iter()
            .map(|item| item.item.seller_name.as_str())
            .all(|actual| query.contains(actual))
    );
}

#[rstest::rstest]
#[test_attr(apply(test))]
#[case(["Sotheby's"].into())]
#[case(["Sotheby's", "Christie's"].into())]
#[trace]
#[localstack_test(services = [OpenSearch(), DynamoDB()])]
async fn should_200_when_exclude_seller_name_query(#[case] query: HashSet<&str>) {
    let ddb_client = get_dynamodb_client().await;
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let notification_service = MockNotificationService::default();
    let search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
    let product_personalization_service = ProductPersonalizationServiceImpl::new(
        &watchlist_repository,
        &notification_service,
        &user_service,
        &search_filter_repository,
    );
    let opensearch_repository = ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let query_service = QueryProductServiceImpl::new(&opensearch_repository);
    let mut access_token_verifier_service = MockAccessTokenVerifierService::default();
    access_token_verifier_service
        .expect_verify_extract_user_id()
        .returning(|_| Box::pin(async { Ok(None) }));

    let search = ProductSearchData {
        language: LanguageData::De,
        currency: CurrencyData::Eur,
        product_query: Some("Exclude seller name filter test".try_into().unwrap()),
        shop_name_query: Default::default(),
        exclude_shop_name_query: Default::default(),
        seller_name_query: Default::default(),
        exclude_seller_name_query: query.iter().map(|s| s.to_string().into()).collect(),
        shop_type_query: Default::default(),
        country_query: Default::default(),
        continent_query: Default::default(),
        geo_address_distance_query: None,
        price_query: None,
        state_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
        shop_slug_id_query: Default::default(),
        exclude_shop_slug_id_query: Default::default(),
        seller_slug_id_query: Default::default(),
        exclude_seller_slug_id_query: Default::default(),
    };

    let seller_names_vec: Vec<&str> = query.iter().copied().collect();

    let mut products_with_target_sellers = fake::vec![ProductDocument; 685];
    for (idx, product) in products_with_target_sellers.iter_mut().enumerate() {
        product.title_de = Some("Exclude seller name filter test".to_string());
        product.seller_name = seller_names_vec[idx % seller_names_vec.len()].to_string();
    }

    let mut products_with_other_sellers = fake::vec![ProductDocument; 685];
    for product in &mut products_with_other_sellers {
        product.title_de = Some("Exclude seller name filter test".to_string());
        product.seller_name = "Other Seller House".to_string();
    }

    let all_products = [products_with_target_sellers, products_with_other_sellers].concat();
    let create_res = opensearch_repository
        .create_product_documents(all_products)
        .await
        .unwrap();
    assert!(!create_res.errors);
    refresh_index("products").await;
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .body_serde(&search)
            .build(),
        context: Default::default(),
    };

    let response = handle(
        lambda_event,
        &query_service,
        None,
        &access_token_verifier_service,
        &product_personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response.status_code);

    let json = extract_apigw_response_json_body!(response);
    let response_data: JsonCursoredData<
        PersonalizedData<GetProductSummaryData, ProductUserStateData>,
    > = serde_json::from_value(json).unwrap();
    assert!(!response_data.items.is_empty());
    assert_eq!(685, response_data.total.unwrap());
    assert!(
        response_data
            .items
            .iter()
            .map(|item| item.item.seller_name.as_str())
            .all(|actual| !query.contains(actual))
    );
}
