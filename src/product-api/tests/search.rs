use cognito::access_token_verifier_service::MockAccessTokenVerifierService;
use common::category_key::CategoryId;
use common::currency::data::CurrencyData;
use common::language::data::LanguageData;
use common::language::document::{LanguageDocument, TextDocument};
use common::language::domain::Language;
use common::period_key::PeriodId;
use common::personalized::api::PersonalizedData;
use common::user_id::UserId;
use common::year::Year;
use common::{pagination::cursor::api::JsonCursoredData, query::range_query::RangeQuery};
use fake::{Fake, Faker, rand};
use lambda_runtime::LambdaEvent;
use product::data::authenticity_data::AuthenticityData;
use product::data::condition_data::ConditionData;
use product::data::get_summary_data::GetProductSummaryData;
use product::data::product_search_data::ProductSearchData;
use product::data::provenance_data::ProvenanceData;
use product::data::restoration_data::RestorationData;
use product::data::user_state_data::ProductUserStateData;
use product::opensearch::{
    product_document::ProductDocument,
    repository::{ProductOpenSearchRepository, ProductOpenSearchRepositoryImpl},
};
use product::service::query_service::QueryProductServiceImpl;
use product_api::search::handle;
use product_personalization::service::ProductPersonalizationServiceImpl;
use product_watchlist::dynamodb::repository::WatchlistProductDynamoDbRepositoryImpl;
use shop::data::shop_type_data::ShopTypeData;
use std::collections::HashSet;
use std::time::Duration;
use test_api::*;
use time::OffsetDateTime;
use time::macros::datetime;
use user::dynamodb::repository::UserDynamoDbRepositoryImpl;
use user::service::user_service::{UserService, UserServiceImpl};

#[localstack_test(services = [OpenSearch(), DynamoDB()])]
async fn should_200_when_no_hits() {
    let ddb_client = get_dynamodb_client().await;
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let product_personalization_service =
        ProductPersonalizationServiceImpl::new(&watchlist_repository, &user_service);
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
async fn should_200_when_following_search_after_from_previous_response_for_sort_price_asc() {
    let ddb_client = get_dynamodb_client().await;
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let product_personalization_service =
        ProductPersonalizationServiceImpl::new(&watchlist_repository, &user_service);
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
        category_id: Default::default(),
        period_id: Default::default(),
        shop_name_query: Default::default(),
        exclude_shop_name_query: Default::default(),
        shop_type_query: Default::default(),
        price_query: None,
        state_query: Default::default(),
        origin_year_query: None,
        authenticity_query: Default::default(),
        condition_query: Default::default(),
        provenance_query: Default::default(),
        restoration_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
    };

    let mut products = fake::vec![ProductDocument; 1370];
    for product in &mut products {
        product.title_de = Some("Der erwartete Titel".to_string());
        product.title_native = TextDocument {
            text: "Der erwartete Titel".to_string(),
            language: LanguageDocument::De,
        };
        product.price_eur = Some(rand::random_range(1..=10000000));
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
    let product_personalization_service =
        ProductPersonalizationServiceImpl::new(&watchlist_repository, &user_service);
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
        category_id: Default::default(),
        period_id: Default::default(),
        shop_name_query: Default::default(),
        exclude_shop_name_query: Default::default(),
        shop_type_query: Default::default(),
        price_query: None,
        state_query: Default::default(),
        origin_year_query: None,
        authenticity_query: Default::default(),
        condition_query: Default::default(),
        provenance_query: Default::default(),
        restoration_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
    };

    let mut products = fake::vec![ProductDocument; 1370];
    for product in &mut products {
        product.title_de = Some("Der erwartete Titel".to_string());
        product.title_native = TextDocument {
            text: "Der erwartete Titel".to_string(),
            language: LanguageDocument::De,
        };
        product.price_eur = Some(rand::random_range(1..=10000000));
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
    let product_personalization_service =
        ProductPersonalizationServiceImpl::new(&watchlist_repository, &user_service);
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
        category_id: Default::default(),
        period_id: Default::default(),
        shop_name_query: Default::default(),
        exclude_shop_name_query: Default::default(),
        shop_type_query: Default::default(),
        price_query: None,
        state_query: Default::default(),
        origin_year_query: None,
        authenticity_query: Default::default(),
        condition_query: Default::default(),
        provenance_query: Default::default(),
        restoration_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
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
async fn should_200_when_following_search_after_from_previous_response_for_explicit_sort_score() {
    let ddb_client = get_dynamodb_client().await;
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let product_personalization_service =
        ProductPersonalizationServiceImpl::new(&watchlist_repository, &user_service);
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
        category_id: Default::default(),
        period_id: Default::default(),
        shop_name_query: Default::default(),
        exclude_shop_name_query: Default::default(),
        shop_type_query: Default::default(),
        price_query: None,
        state_query: Default::default(),
        origin_year_query: None,
        authenticity_query: Default::default(),
        condition_query: Default::default(),
        provenance_query: Default::default(),
        restoration_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
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
async fn should_200_when_following_search_after_from_previous_response_for_sort_year_asc() {
    let ddb_client = get_dynamodb_client().await;
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let product_personalization_service =
        ProductPersonalizationServiceImpl::new(&watchlist_repository, &user_service);
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
        category_id: Default::default(),
        period_id: Default::default(),
        shop_name_query: Default::default(),
        exclude_shop_name_query: Default::default(),
        shop_type_query: Default::default(),
        price_query: None,
        state_query: Default::default(),
        origin_year_query: None,
        authenticity_query: Default::default(),
        condition_query: Default::default(),
        provenance_query: Default::default(),
        restoration_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
    };

    let mut products = fake::vec![ProductDocument; 1370];
    for product in &mut products {
        product.title_de = Some("Der erwartete Titel".to_string());
        product.title_native = TextDocument {
            text: "Der erwartete Titel".to_string(),
            language: LanguageDocument::De,
        };
        product.origin_year = Some(rand::random_range(1300..=1925).into());
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
            .query_string_parameter("sort", "originYear")
            .query_string_parameter("order", "asc")
            .body_serde(&search)
            .build(),
        context: Default::default(),
    };

    let response_1 = handle(
        lambda_event_1,
        &query_service,
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
            .query_string_parameter("sort", "originYear")
            .query_string_parameter("order", "asc")
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
    }));
}

#[localstack_test(services = [OpenSearch(), DynamoDB()])]
async fn should_200_when_following_search_after_from_previous_response_for_sort_year_desc() {
    let ddb_client = get_dynamodb_client().await;
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let product_personalization_service =
        ProductPersonalizationServiceImpl::new(&watchlist_repository, &user_service);
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
        category_id: Default::default(),
        period_id: Default::default(),
        shop_name_query: Default::default(),
        exclude_shop_name_query: Default::default(),
        shop_type_query: Default::default(),
        price_query: None,
        state_query: Default::default(),
        origin_year_query: None,
        authenticity_query: Default::default(),
        condition_query: Default::default(),
        provenance_query: Default::default(),
        restoration_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
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
            .query_string_parameter("size", "5")
            .query_string_parameter("sort", "originYear")
            .query_string_parameter("order", "desc")
            .body_serde(&search)
            .build(),
        context: Default::default(),
    };

    let response_1 = handle(
        lambda_event_1,
        &query_service,
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
    assert_eq!(5, response_data.size);
    assert_eq!(1370, response_data.total.unwrap());

    // second request following up on first
    let lambda_event_2 = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .query_string_parameter("size", "5")
            .query_string_parameter("sort", "originYear")
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
    assert_eq!(5, response_data_2.size);
    assert_eq!(1370, response_data_2.total.unwrap());

    assert!(response_data_2.items.iter().all(|item| {
        !response_data
            .items
            .iter()
            .map(|item| item.item.product_id)
            .collect::<Vec<_>>()
            .contains(&item.item.product_id)
    }));
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
    let product_personalization_service =
        ProductPersonalizationServiceImpl::new(&watchlist_repository, &user_service);
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
        category_id: Default::default(),
        period_id: Default::default(),
        shop_name_query: Default::default(),
        exclude_shop_name_query: Default::default(),
        shop_type_query: Default::default(),
        price_query: None,
        state_query: Default::default(),
        origin_year_query: None,
        authenticity_query: Default::default(),
        condition_query: Default::default(),
        provenance_query: Default::default(),
        restoration_query: Default::default(),
        created_query: Some(created),
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
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
    let product_personalization_service =
        ProductPersonalizationServiceImpl::new(&watchlist_repository, &user_service);
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
        category_id: Default::default(),
        period_id: Default::default(),
        shop_name_query: Default::default(),
        exclude_shop_name_query: Default::default(),
        shop_type_query: Default::default(),
        price_query: None,
        state_query: Default::default(),
        origin_year_query: None,
        authenticity_query: Default::default(),
        condition_query: Default::default(),
        provenance_query: Default::default(),
        restoration_query: Default::default(),
        created_query: None,
        updated_query: Some(updated),
        auction_start_query: None,
        auction_end_query: None,
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

#[rstest::rstest]
#[test_attr(apply(test))]
#[case(None, None)]
#[case(Some(1813.into()), None)]
#[case(Some(1808.into()), None)]
#[case(None, Some(1813.into()))]
#[case(None, Some(1905.into()))]
#[case(Some(1800.into()), Some(1900.into()))]
#[case(Some(1809.into()), Some(1811.into()))]
#[case(Some(1807.into()), Some(1848.into()))]
#[trace]
#[localstack_test(services = [OpenSearch(), DynamoDB()])]
async fn should_200_when_year_query(#[case] min: Option<Year>, #[case] max: Option<Year>) {
    let ddb_client = get_dynamodb_client().await;
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let product_personalization_service =
        ProductPersonalizationServiceImpl::new(&watchlist_repository, &user_service);
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
        category_id: Default::default(),
        period_id: Default::default(),
        shop_name_query: Default::default(),
        exclude_shop_name_query: Default::default(),
        shop_type_query: Default::default(),
        price_query: None,
        state_query: Default::default(),
        origin_year_query: Some(RangeQuery { min, max }),
        authenticity_query: Default::default(),
        condition_query: Default::default(),
        provenance_query: Default::default(),
        restoration_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
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
}

#[rstest::rstest]
#[test_attr(apply(test))]
#[case([AuthenticityData::Original].into())]
#[case([AuthenticityData::Original, AuthenticityData::Questionable].into())]
#[case([AuthenticityData::LaterCopy, AuthenticityData::Reproduction].into())]
#[case([AuthenticityData::Original, AuthenticityData::LaterCopy, AuthenticityData::Reproduction].into())]
#[case([AuthenticityData::Original, AuthenticityData::LaterCopy, AuthenticityData::Reproduction, AuthenticityData::Questionable].into())]
#[case([AuthenticityData::Original, AuthenticityData::Reproduction, AuthenticityData::Questionable].into())]
#[case([AuthenticityData::Original, AuthenticityData::LaterCopy, AuthenticityData::Reproduction, AuthenticityData::Questionable, AuthenticityData::Unknown].into())]
#[trace]
#[localstack_test(services = [OpenSearch(), DynamoDB()])]
async fn should_200_when_authenticity_query(#[case] query: HashSet<AuthenticityData>) {
    let ddb_client = get_dynamodb_client().await;
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let product_personalization_service =
        ProductPersonalizationServiceImpl::new(&watchlist_repository, &user_service);
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
        category_id: Default::default(),
        period_id: Default::default(),
        shop_name_query: Default::default(),
        exclude_shop_name_query: Default::default(),
        shop_type_query: Default::default(),
        price_query: None,
        state_query: Default::default(),
        origin_year_query: None,
        authenticity_query: query.clone(),
        condition_query: Default::default(),
        provenance_query: Default::default(),
        restoration_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
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
}

#[rstest::rstest]
#[test_attr(apply(test))]
#[case([ConditionData::Excellent].into())]
#[case([ConditionData::Excellent, ConditionData::Great].into())]
#[case([ConditionData::Excellent, ConditionData::Poor].into())]
#[case([ConditionData::Excellent, ConditionData::Great, ConditionData::Good].into())]
#[case([ConditionData::Excellent, ConditionData::Fair, ConditionData::Good].into())]
#[case([ConditionData::Excellent, ConditionData::Great, ConditionData::Good, ConditionData::Fair].into())]
#[case([ConditionData::Excellent, ConditionData::Unknown, ConditionData::Good, ConditionData::Poor].into())]
#[case([ConditionData::Excellent, ConditionData::Great, ConditionData::Good, ConditionData::Fair].into())]
#[case([ConditionData::Excellent, ConditionData::Great, ConditionData::Good, ConditionData::Fair, ConditionData::Poor].into())]
#[case([ConditionData::Excellent, ConditionData::Great, ConditionData::Good, ConditionData::Fair, ConditionData::Poor, ConditionData::Unknown].into())]
#[trace]
#[localstack_test(services = [OpenSearch(), DynamoDB()])]
async fn should_200_when_condition_query(#[case] query: HashSet<ConditionData>) {
    let ddb_client = get_dynamodb_client().await;
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let product_personalization_service =
        ProductPersonalizationServiceImpl::new(&watchlist_repository, &user_service);
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
        category_id: Default::default(),
        period_id: Default::default(),
        shop_name_query: Default::default(),
        exclude_shop_name_query: Default::default(),
        shop_type_query: Default::default(),
        price_query: None,
        state_query: Default::default(),
        origin_year_query: None,
        authenticity_query: Default::default(),
        condition_query: query.clone(),
        provenance_query: Default::default(),
        restoration_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
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
}

#[rstest::rstest]
#[test_attr(apply(test))]
#[case([ProvenanceData::Complete].into())]
#[case([ProvenanceData::Unknown].into())]
#[case([ProvenanceData::Complete, ProvenanceData::Partial].into())]
#[case([ProvenanceData::Unknown, ProvenanceData::None].into())]
#[case([ProvenanceData::Complete, ProvenanceData::Partial, ProvenanceData::Claimed].into())]
#[case([ProvenanceData::Complete, ProvenanceData::Unknown, ProvenanceData::Claimed].into())]
#[case([ProvenanceData::Complete, ProvenanceData::Partial, ProvenanceData::Claimed, ProvenanceData::None].into())]
#[case([ProvenanceData::Complete, ProvenanceData::Partial, ProvenanceData::Claimed, ProvenanceData::None, ProvenanceData::Unknown].into())]
#[trace]
#[localstack_test(services = [OpenSearch(), DynamoDB()])]
async fn should_200_when_provenance_query(#[case] query: HashSet<ProvenanceData>) {
    let ddb_client = get_dynamodb_client().await;
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let product_personalization_service =
        ProductPersonalizationServiceImpl::new(&watchlist_repository, &user_service);
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
        category_id: Default::default(),
        period_id: Default::default(),
        shop_name_query: Default::default(),
        exclude_shop_name_query: Default::default(),
        shop_type_query: Default::default(),
        price_query: None,
        state_query: Default::default(),
        origin_year_query: None,
        authenticity_query: Default::default(),
        condition_query: Default::default(),
        provenance_query: query.clone(),
        restoration_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
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
}

#[rstest::rstest]
#[test_attr(apply(test))]
#[case([RestorationData::Major].into())]
#[case([RestorationData::Minor].into())]
#[case([RestorationData::None].into())]
#[case([RestorationData::Major, RestorationData::Minor].into())]
#[case([RestorationData::None, RestorationData::Minor].into())]
#[case([RestorationData::None, RestorationData::Unknown].into())]
#[case([RestorationData::Major, RestorationData::Minor, RestorationData::None].into())]
#[case([RestorationData::Major, RestorationData::Minor, RestorationData::None, RestorationData::Unknown].into())]
#[trace]
#[localstack_test(services = [OpenSearch(), DynamoDB()])]
async fn should_200_when_restoration_query(#[case] query: HashSet<RestorationData>) {
    let ddb_client = get_dynamodb_client().await;
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let product_personalization_service =
        ProductPersonalizationServiceImpl::new(&watchlist_repository, &user_service);
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
        category_id: Default::default(),
        period_id: Default::default(),
        shop_name_query: Default::default(),
        exclude_shop_name_query: Default::default(),
        shop_type_query: Default::default(),
        price_query: None,
        state_query: Default::default(),
        origin_year_query: None,
        authenticity_query: Default::default(),
        condition_query: Default::default(),
        provenance_query: Default::default(),
        restoration_query: query.clone(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
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
}

#[localstack_test(services = [OpenSearch(), DynamoDB()])]
async fn should_200_personalized_when_authenticated_and_not_watching() {
    let ddb_client = get_dynamodb_client().await;
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let product_personalization_service =
        ProductPersonalizationServiceImpl::new(&watchlist_repository, &user_service);
    let opensearch_repository = ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let query_service = QueryProductServiceImpl::new(&opensearch_repository);
    let mut access_token_verifier_service = MockAccessTokenVerifierService::default();
    let user_id = UserId::new();
    user_service
        .create_user(user::service::command::CreateUserCommand {
            id: user_id,
            email: "foo@bar.de".try_into().unwrap(),
        })
        .await
        .unwrap();
    access_token_verifier_service
        .expect_verify_extract_user_id()
        .returning(move |_| Box::pin(async move { Ok(Some(user_id)) }));

    let search = ProductSearchData {
        language: common::language::data::LanguageData::De,
        currency: common::currency::data::CurrencyData::Eur,
        product_query: Some("Der erwartete Titel".try_into().unwrap()),
        category_id: Default::default(),
        period_id: Default::default(),
        shop_name_query: Default::default(),
        exclude_shop_name_query: Default::default(),
        shop_type_query: Default::default(),
        price_query: None,
        state_query: Default::default(),
        origin_year_query: None,
        authenticity_query: Default::default(),
        condition_query: Default::default(),
        provenance_query: Default::default(),
        restoration_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
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
        let user_state = item.user_state.unwrap();
        !user_state.watchlist.notifications && !user_state.watchlist.watching
    }));
}

#[localstack_test(services = [OpenSearch(), DynamoDB()])]
async fn should_200_with_native_title_when_no_target_titles_exist_and_hit_due_to_description() {
    let ddb_client = get_dynamodb_client().await;
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let product_personalization_service =
        ProductPersonalizationServiceImpl::new(&watchlist_repository, &user_service);
    let opensearch_repository = ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let query_service = QueryProductServiceImpl::new(&opensearch_repository);
    let mut access_token_verifier_service = MockAccessTokenVerifierService::default();
    access_token_verifier_service
        .expect_verify_extract_user_id()
        .returning(|_| Box::pin(async { Ok(None) }));

    let mut document = Faker.fake::<ProductDocument>();
    document.title_native = TextDocument {
        text: "Non-german title".to_string(),
        language: LanguageDocument::Es,
    };
    document.title_de = None;
    document.title_en = None;
    document.title_fr = None;
    document.title_es = None;
    document.title_it = None;
    document.description_de = Some("Some german description that will result in a hit".to_string());
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
                language: LanguageData::De,
                currency: CurrencyData::Eur,
                product_query: Some("german description".try_into().unwrap()),
                category_id: Default::default(),
                period_id: Default::default(),
                shop_name_query: Default::default(),
                exclude_shop_name_query: Default::default(),
                shop_type_query: Default::default(),
                price_query: None,
                state_query: Default::default(),
                origin_year_query: None,
                authenticity_query: Default::default(),
                condition_query: Default::default(),
                provenance_query: Default::default(),
                restoration_query: Default::default(),
                created_query: None,
                updated_query: None,
                auction_start_query: None,
                auction_end_query: None,
            })
            .build(),
        context: Default::default(),
    };

    let response = handle(
        lambda_event,
        &query_service,
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
    assert_eq!(
        LanguageData::Es,
        response_data.items.first().unwrap().item.title.language,
    );
    assert_eq!(
        "Non-german title",
        response_data.items.first().unwrap().item.title.text,
    );
}

#[rstest::rstest]
#[test_attr(apply(test))]
#[case("de", "German title", Language::De)]
#[case("de-DE", "German title", Language::De)]
#[case("de-AT", "German title", Language::De)]
#[case("de;q=1.0", "German title", Language::De)]
#[case("de-DE,de;q=0.9,en;q=0.8", "German title", Language::De)]
#[case("en;q=0.5,de;q=1.0", "German title", Language::De)]
#[case("de,*;q=0.1", "German title", Language::De)]
#[case("en", "English title", Language::En)]
#[case("en-US", "English title", Language::En)]
#[case("en-GB", "English title", Language::En)]
#[case("en;q=0.7", "English title", Language::En)]
#[case("fr;q=0.3,en;q=0.9", "English title", Language::En)]
#[case("zh,ko;q=0.5,en;q=0.6", "English title", Language::En)]
#[case("*,en;q=0.8", "English title", Language::En)]
#[case("fr", "French title", Language::Fr)]
#[case("fr-FR", "French title", Language::Fr)]
#[case("fr-CA", "French title", Language::Fr)]
#[case("fr;q=1.0", "French title", Language::Fr)]
#[case("fr,en;q=0.4", "French title", Language::Fr)]
#[case("fr-BE,fr;q=0.9", "French title", Language::Fr)]
#[case("es;q=0.2,de;q=0.4,fr;q=0.8", "French title", Language::Fr)]
#[case("*,fr;q=0.7", "French title", Language::Fr)]
#[case("es", "Spanish title", Language::Es)]
#[case("es-ES", "Spanish title", Language::Es)]
#[case("es-MX", "Spanish title", Language::Es)]
#[case("es;q=1.0", "Spanish title", Language::Es)]
#[case("es,en;q=0.3", "Spanish title", Language::Es)]
#[case("es-AR,es;q=0.9", "Spanish title", Language::Es)]
#[case("fr;q=0.1,de;q=0.2,es;q=0.6", "Spanish title", Language::Es)]
#[case("*,es;q=0.5", "Spanish title", Language::Es)]
#[case("it", "Italian title", Language::It)]
#[case("it-IT", "Italian title", Language::It)]
#[case("it-CH", "Italian title", Language::It)]
#[case("it;q=1.0", "Italian title", Language::It)]
#[case("it,en;q=0.3", "Italian title", Language::It)]
#[case("it-IT,it;q=0.9", "Italian title", Language::It)]
#[case("de;q=0.1,fr;q=0.2,it;q=0.6", "Italian title", Language::It)]
#[case("*,it;q=0.5", "Italian title", Language::It)]
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
    let product_personalization_service =
        ProductPersonalizationServiceImpl::new(&watchlist_repository, &user_service);
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
    document.description_de = Some("German description".to_string());
    document.description_en = Some("English description".to_string());
    document.description_fr = Some("French description".to_string());
    document.description_es = Some("Spanish description".to_string());
    document.description_it = Some("Italian description".to_string());
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
                category_id: Default::default(),
                period_id: Default::default(),
                shop_name_query: Default::default(),
                exclude_shop_name_query: Default::default(),
                shop_type_query: Default::default(),
                price_query: None,
                state_query: Default::default(),
                origin_year_query: None,
                authenticity_query: Default::default(),
                condition_query: Default::default(),
                provenance_query: Default::default(),
                restoration_query: Default::default(),
                created_query: None,
                updated_query: None,
                auction_start_query: None,
                auction_end_query: None,
            })
            .build(),
        context: Default::default(),
    };

    let response = handle(
        lambda_event,
        &query_service,
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
    let product_personalization_service =
        ProductPersonalizationServiceImpl::new(&watchlist_repository, &user_service);
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
        category_id: Default::default(),
        period_id: Default::default(),
        shop_name_query: Default::default(),
        exclude_shop_name_query: Default::default(),
        shop_type_query: query.clone(),
        price_query: None,
        state_query: Default::default(),
        origin_year_query: None,
        authenticity_query: Default::default(),
        condition_query: Default::default(),
        provenance_query: Default::default(),
        restoration_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
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
    let product_personalization_service =
        ProductPersonalizationServiceImpl::new(&watchlist_repository, &user_service);
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
        category_id: Default::default(),
        period_id: Default::default(),
        shop_name_query: query.iter().map(|s| s.to_string().into()).collect(),
        exclude_shop_name_query: Default::default(),
        shop_type_query: Default::default(),
        price_query: None,
        state_query: Default::default(),
        origin_year_query: None,
        authenticity_query: Default::default(),
        condition_query: Default::default(),
        provenance_query: Default::default(),
        restoration_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
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
    let product_personalization_service =
        ProductPersonalizationServiceImpl::new(&watchlist_repository, &user_service);
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
        category_id: Default::default(),
        period_id: Default::default(),
        shop_name_query: Default::default(),
        exclude_shop_name_query: query.iter().map(|s| s.to_string().into()).collect(),
        shop_type_query: Default::default(),
        price_query: None,
        state_query: Default::default(),
        origin_year_query: None,
        authenticity_query: Default::default(),
        condition_query: Default::default(),
        provenance_query: Default::default(),
        restoration_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
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
async fn should_200_when_category_id_filter_is_given() {
    let ddb_client = get_dynamodb_client().await;
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let product_personalization_service =
        ProductPersonalizationServiceImpl::new(&watchlist_repository, &user_service);
    let opensearch_repository = ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let query_service = QueryProductServiceImpl::new(&opensearch_repository);
    let mut access_token_verifier_service = MockAccessTokenVerifierService::default();
    access_token_verifier_service
        .expect_verify_extract_user_id()
        .returning(|_| Box::pin(async { Ok(None) }));

    let category_id = CategoryId::from("furniture");
    let other_category_id = CategoryId::from("decorative-objects");
    let search = ProductSearchData {
        language: common::language::data::LanguageData::De,
        currency: common::currency::data::CurrencyData::Eur,
        product_query: Some("Der erwartete Titel".try_into().unwrap()),
        category_id: HashSet::from_iter([category_id.clone()]),
        period_id: Default::default(),
        shop_name_query: Default::default(),
        exclude_shop_name_query: Default::default(),
        shop_type_query: Default::default(),
        price_query: None,
        state_query: Default::default(),
        origin_year_query: None,
        authenticity_query: Default::default(),
        condition_query: Default::default(),
        provenance_query: Default::default(),
        restoration_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
    };
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .query_string_parameter("size", "100")
            .body_serde(&search)
            .build(),
        context: Default::default(),
    };

    let mut products_with_category = fake::vec![ProductDocument; 50];
    for product in &mut products_with_category {
        product.title_de = Some("Der erwartete Titel".to_string());
        product.category_id = Some(category_id.clone());
    }

    let mut products_with_other_category = fake::vec![ProductDocument; 40];
    for product in &mut products_with_other_category {
        product.title_de = Some("Der erwartete Titel".to_string());
        product.category_id = Some(other_category_id.clone());
    }

    let all_products = [products_with_category, products_with_other_category].concat();
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
    assert_eq!(50, response_data.items.len());
    assert_eq!(50, response_data.total.unwrap());
}

#[localstack_test(services = [OpenSearch(), DynamoDB()])]
async fn should_200_when_period_id_filter_is_given() {
    let ddb_client = get_dynamodb_client().await;
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let product_personalization_service =
        ProductPersonalizationServiceImpl::new(&watchlist_repository, &user_service);
    let opensearch_repository = ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let query_service = QueryProductServiceImpl::new(&opensearch_repository);
    let mut access_token_verifier_service = MockAccessTokenVerifierService::default();
    access_token_verifier_service
        .expect_verify_extract_user_id()
        .returning(|_| Box::pin(async { Ok(None) }));

    let period_id = PeriodId::from("furniture");
    let other_period_id = PeriodId::from("decorative-objects");
    let search = ProductSearchData {
        language: common::language::data::LanguageData::De,
        currency: common::currency::data::CurrencyData::Eur,
        product_query: Some("Der erwartete Titel".try_into().unwrap()),
        category_id: Default::default(),
        period_id: HashSet::from_iter([period_id.clone()]),
        shop_name_query: Default::default(),
        exclude_shop_name_query: Default::default(),
        shop_type_query: Default::default(),
        price_query: None,
        state_query: Default::default(),
        origin_year_query: None,
        authenticity_query: Default::default(),
        condition_query: Default::default(),
        provenance_query: Default::default(),
        restoration_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
    };
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .query_string_parameter("size", "100")
            .body_serde(&search)
            .build(),
        context: Default::default(),
    };

    let mut products_with_period = fake::vec![ProductDocument; 50];
    for product in &mut products_with_period {
        product.title_de = Some("Der erwartete Titel".to_string());
        product.period_id = Some(period_id.clone());
    }

    let mut products_with_other_period = fake::vec![ProductDocument; 40];
    for product in &mut products_with_other_period {
        product.title_de = Some("Der erwartete Titel".to_string());
        product.period_id = Some(other_period_id.clone());
    }

    let all_products = [products_with_period, products_with_other_period].concat();
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
    assert_eq!(50, response_data.items.len());
    assert_eq!(50, response_data.total.unwrap());
}

#[localstack_test(services = [OpenSearch(), DynamoDB()])]
async fn should_200_when_auction_start_range_is_given() {
    let ddb_client = get_dynamodb_client().await;
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let product_personalization_service =
        ProductPersonalizationServiceImpl::new(&watchlist_repository, &user_service);
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
        category_id: Default::default(),
        period_id: Default::default(),
        shop_name_query: Default::default(),
        exclude_shop_name_query: Default::default(),
        shop_type_query: Default::default(),
        price_query: None,
        state_query: Default::default(),
        origin_year_query: None,
        authenticity_query: Default::default(),
        condition_query: Default::default(),
        provenance_query: Default::default(),
        restoration_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: Some(RangeQuery {
            min: Some(datetime!(2026-01-01 0:00 UTC)),
            max: Some(datetime!(2026-03-31 23:59 UTC)),
        }),
        auction_end_query: None,
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
    let product_personalization_service =
        ProductPersonalizationServiceImpl::new(&watchlist_repository, &user_service);
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
        category_id: Default::default(),
        period_id: Default::default(),
        shop_name_query: Default::default(),
        exclude_shop_name_query: Default::default(),
        shop_type_query: Default::default(),
        price_query: None,
        state_query: Default::default(),
        origin_year_query: None,
        authenticity_query: Default::default(),
        condition_query: Default::default(),
        provenance_query: Default::default(),
        restoration_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: Some(RangeQuery {
            min: None,
            max: Some(datetime!(2026-02-28 23:59 UTC)),
        }),
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
