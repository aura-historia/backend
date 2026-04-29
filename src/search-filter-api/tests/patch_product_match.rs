use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;
use common::user_search_filter_id::UserSearchFilterId;
use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use product::service::get_service::MockGetProductService;
use product_personalization::service::MockProductPersonalizationService;
use search_filter::dynamodb::repository::UserSearchFilterDynamoDbRepositoryImpl;
use search_filter::service::user_search_filter_service::UserSearchFilterServiceImpl;
use search_filter_api::handler;
use search_filter_api::patch_product_match::PatchUserSearchFilterMatchData;
use test_api::*;
use user::service::user_service::UserService;

#[localstack_test(services = [DynamoDB()])]
async fn should_404_when_search_filter_product_match_not_found() {
    let repository =
        UserSearchFilterDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let user_repository = user::dynamodb::repository::UserDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        "table_1",
    );
    let user_service = user::service::user_service::UserServiceImpl::new(&user_repository);
    let service = UserSearchFilterServiceImpl::new(&repository, &user_service);
    let get_product_service = MockGetProductService::default();
    let personalization_service = MockProductPersonalizationService::default();
    let user = user_service.create_user(Faker.fake()).await.unwrap();

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::PATCH)
            .route_key("PATCH /api/v1/me/search-filters/{userSearchFilterId}/products/{shopId}/{shopsProductId}")
            .jwt_claim("sub", user.user_id)
            .path_parameter("userSearchFilterId", UserSearchFilterId::new())
            .path_parameter("shopId", ShopId::new())
            .path_parameter("shopsProductId", ShopsProductId::new())
            .body_serde(&PatchUserSearchFilterMatchData {
                matches_feedback: Some(false),
            })
            .build(),
        context: Default::default(),
    };

    let response = handler(
        lambda_event,
        &service,
        &get_product_service,
        &personalization_service,
    )
    .await
    .unwrap();
    let json = extract_apigw_response_json_body!(response);

    assert_eq!(404, response.status_code);
    assert_eq!(404, json["status"]);
    assert_eq!("SEARCH_FILTER_NOT_FOUND", json["error"]);
}
