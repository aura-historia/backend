use common::pagination::cursor::api::JsonCursoredData;
use common::personalized::api::PersonalizedData;
use common::user_id::UserId;
use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use notification::dynamodb::repository::NotificationDynamoDbRepositoryImpl;
use notification::service::noop_adapters::{NoopS3Adapter, NoopSesAdapter};
use notification::service::notification_service::NotificationServiceImpl;
use product::data::get_summary_data::GetProductSummaryData;
use product::data::user_state_data::ProductUserStateData;
use product::dynamodb::repository::ProductDynamoDbRepositoryImpl;
use product::opensearch::repository::ProductOpenSearchRepositoryImpl;
use product::service::get_service::GetProductServiceImpl;
use product::service::query_service::QueryProductServiceImpl;
use product_personalization::service::ProductPersonalizationServiceImpl;
use product_watchlist::dynamodb::repository::WatchlistProductDynamoDbRepositoryImpl;
use search_filter::service::user_search_filter_service::{
    UserSearchFilterService, UserSearchFilterServiceImpl,
};
use search_filter_api::handle;
use test_api::*;
use user::core::tier::UserTier;
use user::dynamodb::repository::UserDynamoDbRepositoryImpl;
use user::service::command::UpdateUserCommand;
use user::service::user_service::{UserService, UserServiceImpl};

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

async fn create_user(client: &'static aws_sdk_dynamodb::Client) -> UserId {
    let user_repository = UserDynamoDbRepositoryImpl::new(client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let user = user_service.create_user(Faker.fake()).await.unwrap();
    let update_cmd = UpdateUserCommand {
        tier: Some(UserTier::Ultimate),
        ..Default::default()
    };
    user_service
        .update_user(&user.user_id, update_cmd)
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
        .create_user_search_filter(&user_id, Faker.fake(), Faker.fake(), None)
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
async fn should_400_when_search_after_provided() {
    let client = get_dynamodb_client().await;
    let opensearch = get_opensearch_client().await;
    let (service, get_product_service, query_product_service, personalization_service) =
        setup_services(client, opensearch);

    let user_id = create_user(client).await;
    let search_filter = service
        .create_user_search_filter(&user_id, Faker.fake(), Faker.fake(), None)
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
        None,
        &personalization_service,
    )
    .await
    .unwrap_err();
    assert_eq!(400, actual.status);
}
