use common::currency::domain::Currency;
use common::language::document::{LanguageDocument, TextDocument};
use common::language::domain::Language;
use common::pagination::cursor::api::JsonCursoredData;
use common::personalized::api::PersonalizedData;
use common::product_id::ProductId;
use common::shops_product_id::ShopsProductId;
use common::user_id::UserId;
use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use notification::dynamodb::repository::NotificationDynamoDbRepositoryImpl;
use notification::service::noop_adapters::{NoopS3Adapter, NoopSesAdapter};
use notification::service::notification_service::NotificationServiceImpl;
use product::core::product_search::ProductSearch;
use product::data::get_summary_data::GetProductSummaryData;
use product::data::user_state_data::ProductUserStateData;
use product::dynamodb::repository::ProductDynamoDbRepositoryImpl;
use product::opensearch::product_document::ProductDocument;
use product::opensearch::repository::{
    ProductOpenSearchRepository, ProductOpenSearchRepositoryImpl,
};
use product::service::get_service::GetProductServiceImpl;
use product::service::query_service::QueryProductServiceImpl;
use product_personalization::service::ProductPersonalizationServiceImpl;
use product_watchlist::dynamodb::repository::WatchlistProductDynamoDbRepositoryImpl;
use search_filter::service::user_search_filter_service::{
    UserSearchFilterService, UserSearchFilterServiceImpl,
};
use search_filter_api::handle;
use std::time::Duration;
use test_api::*;
use user::core::tier::UserTier;
use user::dynamodb::repository::UserDynamoDbRepositoryImpl;
use user::service::command::UpdateUserCommand;
use user::service::user_service::{UserService, UserServiceImpl};

fn user_ctx(user_id: UserId) -> common::actor::RequestContext {
    common::actor::RequestContext {
        actor: common::actor::domain::Actor::User(user_id),
    }
}

fn setup_services(
    client: &'static aws_sdk_dynamodb::Client,
    opensearch: &'static opensearch::OpenSearch,
) -> (
    UserSearchFilterServiceImpl<'static>,
    GetProductServiceImpl<'static>,
    QueryProductServiceImpl<'static>,
    ProductPersonalizationServiceImpl<'static>,
) {
    let product_dynamodb_repository = Box::leak(Box::new(ProductDynamoDbRepositoryImpl::new(
        client, "table_1",
    )));
    let product_opensearch_repository =
        Box::leak(Box::new(ProductOpenSearchRepositoryImpl::new(opensearch)));
    let watchlist_repository = Box::leak(Box::new(WatchlistProductDynamoDbRepositoryImpl::new(
        client, "table_1",
    )));
    let notification_repository = Box::leak(Box::new(NotificationDynamoDbRepositoryImpl::new(
        client, "table_1",
    )));
    let user_repository = Box::leak(Box::new(UserDynamoDbRepositoryImpl::new(client, "table_1")));
    let search_filter_repository = Box::leak(Box::new(
        search_filter::dynamodb::repository::UserSearchFilterDynamoDbRepositoryImpl::new(
            client, "table_1",
        ),
    ));
    let get_product_service = GetProductServiceImpl::new(product_dynamodb_repository);
    let query_product_service = QueryProductServiceImpl::new(product_opensearch_repository);
    let noop_ses: &'static NoopSesAdapter = Box::leak(Box::new(NoopSesAdapter));
    let noop_s3: &'static NoopS3Adapter = Box::leak(Box::new(NoopS3Adapter));
    let user_service: &'static UserServiceImpl<'static> =
        Box::leak(Box::new(UserServiceImpl::new(user_repository)));
    let notification_service: &'static NotificationServiceImpl<'static> =
        Box::leak(Box::new(NotificationServiceImpl::new(
            notification_repository,
            user_service,
            noop_ses,
            noop_s3,
            "",
            "",
            "",
        )));
    let personalization_service = ProductPersonalizationServiceImpl::new(
        watchlist_repository,
        notification_service,
        user_service,
        search_filter_repository,
    );
    let service = UserSearchFilterServiceImpl::new(search_filter_repository, user_service);
    (
        service,
        get_product_service,
        query_product_service,
        personalization_service,
    )
}

fn matching_product_document(product_id: ProductId, shops_product_id: &str) -> ProductDocument {
    let mut product: ProductDocument = Faker.fake();
    product.product_id = product_id;
    product.shops_product_id = ShopsProductId::from(shops_product_id);
    product.title_native = TextDocument {
        text: "golden cufflinks antique vintage".to_string(),
        language: LanguageDocument::En,
    };
    product.title_de = Some("golden cufflinks antique vintage".to_string());
    product.title_en = Some("golden cufflinks antique vintage".to_string());
    product.price_eur = Some(1_000);
    product.embedding = None;
    product
}

fn non_matching_product_document(product_id: ProductId, shops_product_id: &str) -> ProductDocument {
    let mut product = matching_product_document(product_id, shops_product_id);
    product.title_native.text = "silver tea set".to_string();
    product.title_de = Some("silver tea set".to_string());
    product.title_en = Some("silver tea set".to_string());
    product
}

async fn index_products(
    opensearch: &'static opensearch::OpenSearch,
    products: Vec<ProductDocument>,
) {
    let product_opensearch_repository = ProductOpenSearchRepositoryImpl::new(opensearch);
    let response = product_opensearch_repository
        .create_product_documents(products)
        .await
        .unwrap();
    assert!(!response.errors);
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_millis(3000)).await;
}

fn percolator_only_search() -> ProductSearch {
    ProductSearch::new(Language::De, Currency::Eur)
        .with_product_query("golden cufflinks antique vintage rare".try_into().unwrap())
}

async fn create_user(client: &'static aws_sdk_dynamodb::Client) -> UserId {
    let user_repository = UserDynamoDbRepositoryImpl::new(client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let user = user_service
        .create_user(&user_ctx(UserId::new()), Faker.fake())
        .await
        .unwrap();
    let update_cmd = UpdateUserCommand {
        tier: Some(UserTier::Ultimate),
        ..Default::default()
    };
    user_service
        .update_user(&user_ctx(user.user_id), &user.user_id, update_cmd)
        .await
        .unwrap();
    user.user_id
}

#[localstack_test(services = [DynamoDB(), OpenSearch()])]
async fn should_200_when_success_without_enhanced_description() {
    let client = get_dynamodb_client().await;
    let opensearch = get_opensearch_client().await;
    let (service, get_product_service, query_product_service, personalization_service) =
        setup_services(client, opensearch);

    let user_id = create_user(client).await;
    let search_filter = service
        .create_user_search_filter(&user_ctx(user_id), &user_id, Faker.fake(), Faker.fake())
        .await
        .unwrap();

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .route_key("GET /api/v1/me/search-filters/{userSearchFilterId}/products")
            .jwt_claim("sub", user_id)
            .path_parameter("userSearchFilterId", search_filter.user_search_filter_id)
            .query_string_parameter("language", "de")
            .query_string_parameter("currency", "EUR")
            .build(),
        context: Default::default(),
    };

    let response = handle(
        lambda_event,
        &service,
        &get_product_service,
        &query_product_service,
        None,
        &personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response.status_code);

    let actual: JsonCursoredData<PersonalizedData<GetProductSummaryData, ProductUserStateData>> =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert!(actual.items.is_empty());
}

#[localstack_test(services = [DynamoDB(), OpenSearch()])]
async fn should_preview_products_with_percolator_query_semantics() {
    let client = get_dynamodb_client().await;
    let opensearch = get_opensearch_client().await;
    let (service, get_product_service, query_product_service, personalization_service) =
        setup_services(client, opensearch);

    let expected_product_id = ProductId::new();
    index_products(
        opensearch,
        vec![
            matching_product_document(expected_product_id, "percolator-match"),
            non_matching_product_document(ProductId::new(), "percolator-miss"),
        ],
    )
    .await;

    let user_id = create_user(client).await;
    let search_filter = service
        .create_user_search_filter(
            &user_ctx(user_id),
            &user_id,
            Faker.fake(),
            percolator_only_search(),
        )
        .await
        .unwrap();

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .route_key("GET /api/v1/me/search-filters/{userSearchFilterId}/products")
            .jwt_claim("sub", user_id)
            .path_parameter("userSearchFilterId", search_filter.user_search_filter_id)
            .query_string_parameter("language", "de")
            .query_string_parameter("currency", "EUR")
            .build(),
        context: Default::default(),
    };

    let response = handle(
        lambda_event,
        &service,
        &get_product_service,
        &query_product_service,
        None,
        &personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response.status_code);

    let actual: JsonCursoredData<PersonalizedData<GetProductSummaryData, ProductUserStateData>> =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert_eq!(1, actual.items.len());
    assert_eq!(expected_product_id, actual.items[0].item.product_id);
    assert_eq!(1, actual.size);
    assert!(actual.search_after.is_none());
}

#[localstack_test(services = [DynamoDB(), OpenSearch()])]
async fn should_return_hardcoded_preview_size_without_pagination() {
    let client = get_dynamodb_client().await;
    let opensearch = get_opensearch_client().await;
    let (service, get_product_service, query_product_service, personalization_service) =
        setup_services(client, opensearch);

    let products = (0..11)
        .map(|idx| matching_product_document(ProductId::new(), &format!("percolator-match-{idx}")))
        .collect();
    index_products(opensearch, products).await;

    let user_id = create_user(client).await;
    let search_filter = service
        .create_user_search_filter(
            &user_ctx(user_id),
            &user_id,
            Faker.fake(),
            percolator_only_search(),
        )
        .await
        .unwrap();

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .route_key("GET /api/v1/me/search-filters/{userSearchFilterId}/products")
            .jwt_claim("sub", user_id)
            .path_parameter("userSearchFilterId", search_filter.user_search_filter_id)
            .query_string_parameter("language", "de")
            .query_string_parameter("currency", "EUR")
            .build(),
        context: Default::default(),
    };

    let response = handle(
        lambda_event,
        &service,
        &get_product_service,
        &query_product_service,
        None,
        &personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response.status_code);

    let actual: JsonCursoredData<PersonalizedData<GetProductSummaryData, ProductUserStateData>> =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert_eq!(10, actual.items.len());
    assert_eq!(10, actual.size);
    assert!(actual.search_after.is_none());
}

#[localstack_test(services = [DynamoDB(), OpenSearch()])]
async fn should_400_when_size_provided() {
    let client = get_dynamodb_client().await;
    let opensearch = get_opensearch_client().await;
    let (service, get_product_service, query_product_service, personalization_service) =
        setup_services(client, opensearch);

    let user_id = create_user(client).await;
    let search_filter = service
        .create_user_search_filter(&user_ctx(user_id), &user_id, Faker.fake(), Faker.fake())
        .await
        .unwrap();

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .route_key("GET /api/v1/me/search-filters/{userSearchFilterId}/products")
            .jwt_claim("sub", user_id)
            .path_parameter("userSearchFilterId", search_filter.user_search_filter_id)
            .query_string_parameter("language", "de")
            .query_string_parameter("currency", "EUR")
            .query_string_parameter("size", "5")
            .build(),
        context: Default::default(),
    };

    let actual = handle(
        lambda_event,
        &service,
        &get_product_service,
        &query_product_service,
        None,
        &personalization_service,
    )
    .await
    .unwrap_err();
    assert_eq!(400, actual.status);
}

#[localstack_test(services = [DynamoDB(), OpenSearch()])]
async fn should_400_when_search_after_provided() {
    let client = get_dynamodb_client().await;
    let opensearch = get_opensearch_client().await;
    let (service, get_product_service, query_product_service, personalization_service) =
        setup_services(client, opensearch);

    let user_id = create_user(client).await;
    let search_filter = service
        .create_user_search_filter(&user_ctx(user_id), &user_id, Faker.fake(), Faker.fake())
        .await
        .unwrap();

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .route_key("GET /api/v1/me/search-filters/{userSearchFilterId}/products")
            .jwt_claim("sub", user_id)
            .path_parameter("userSearchFilterId", search_filter.user_search_filter_id)
            .query_string_parameter("language", "de")
            .query_string_parameter("currency", "EUR")
            .query_string_parameter("size", "5")
            .query_string_parameter("searchAfter", "1234567890")
            .build(),
        context: Default::default(),
    };

    let actual = handle(
        lambda_event,
        &service,
        &get_product_service,
        &query_product_service,
        None,
        &personalization_service,
    )
    .await
    .unwrap_err();
    assert_eq!(400, actual.status);
}
