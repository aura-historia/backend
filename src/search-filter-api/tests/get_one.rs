use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use product::core::product_search::ProductSearch;
use product::service::get_service::MockGetProductService;
use product::service::query_service::MockQueryProductService;
use product_personalization::service::MockProductPersonalizationService;
use search_filter::dynamodb::repository::UserSearchFilterDynamoDbRepositoryImpl;
use search_filter::service::user_search_filter_service::{
    UserSearchFilterService, UserSearchFilterServiceImpl,
};
use search_filter_api::handle;
use test_api::*;
use user::core::tier::UserTier;
use user::service::command::UpdateUserCommand;
use user::service::user_service::UserService;

fn user_ctx(user_id: common::user_id::UserId) -> common::actor::RequestContext {
    common::actor::RequestContext {
        actor: common::actor::domain::Actor::User(user_id),
    }
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_actual_search_filter() {
    let repository =
        UserSearchFilterDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let user_repository = user::dynamodb::repository::UserDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        "table_1",
    );
    let user_service = user::service::user_service::UserServiceImpl::new(&user_repository);
    let service = UserSearchFilterServiceImpl::new(&repository, &user_service);
    let get_product_service = MockGetProductService::default();
    let query_product_service = MockQueryProductService::default();
    let personalization_service = MockProductPersonalizationService::default();

    let user = user_service
        .create_user(&user_ctx(common::user_id::UserId::new()), Faker.fake())
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

    let expected = service
        .create_user_search_filter(
            &user_ctx(user.user_id),
            &user.user_id,
            Faker.fake(),
            Faker.fake::<ProductSearch>(),
        )
        .await
        .unwrap();
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .route_key("GET /api/v1/me/search-filters/{userSearchFilterId}")
            .jwt_claim("sub", user.user_id)
            .path_parameter("userSearchFilterId", expected.user_search_filter_id)
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
    let json = extract_apigw_response_json_body!(response);

    assert_eq!(user.user_id.to_string(), json["userId"]);
    assert_eq!(
        expected.user_search_filter_id.to_string(),
        json["userSearchFilterId"]
    );
}
