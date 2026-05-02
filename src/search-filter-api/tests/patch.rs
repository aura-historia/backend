use common::query::range_query::RangeQuery;
use common::shop_name::ShopName;
use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use product::core::product_search::ProductSearch;
use product::service::get_service::MockGetProductService;
use product_personalization::service::MockProductPersonalizationService;
use search_filter::data::user_search_filter_data::UserSearchFilterData;
use search_filter::dynamodb::repository::UserSearchFilterDynamoDbRepositoryImpl;
use search_filter::service::user_search_filter_service::{
    UserSearchFilterService, UserSearchFilterServiceImpl,
};
use search_filter_api::handle;
use search_filter_api::patch_types::{PatchProductSearchData, PatchUserSearchFilterData};
use test_api::*;
use user::core::tier::UserTier;
use user::service::command::UpdateUserCommand;
use user::service::user_service::UserService;

#[localstack_test(services = [DynamoDB()])]
async fn should_update_search_filter() {
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
    let update_cmd = UpdateUserCommand {
        tier: Some(UserTier::Ultimate),
        ..Default::default()
    };
    user_service
        .update_user(&user.user_id, update_cmd)
        .await
        .unwrap();

    let initial = service
        .create_user_search_filter(
            &user.user_id,
            Faker.fake(),
            Faker.fake::<ProductSearch>(),
            Faker.fake(),
        )
        .await
        .unwrap();

    let patch = PatchUserSearchFilterData {
        name: Some("thorbens filter".into()),
        enhanced_search_description: None,
        notifications: None,
        state: None,
        search: Some(PatchProductSearchData {
            language: None,
            currency: None,
            product_query: None,
            category_id: None,
            period_id: None,
            shop_name_query: Some(HashSet::from_iter([ShopName::from("Whoop boop woah")])),
            exclude_shop_name_query: None,
            seller_name_query: None,
            exclude_seller_name_query: None,
            shop_slug_id_query: None,
            exclude_shop_slug_id_query: None,
            seller_slug_id_query: None,
            exclude_seller_slug_id_query: None,
            shop_type_query: None,
            price_query: Some(RangeQuery {
                min: Some(37),
                max: Some(42),
            }),
            state_query: None,
            origin_year_query: None,
            authenticity_query: Default::default(),
            condition_query: Default::default(),
            provenance_query: Default::default(),
            restoration_query: Default::default(),
            auction_start_query: None,
            auction_end_query: None,
            created_query: None,
            updated_query: None,
        }),
    };
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::PATCH)
            .route_key("PATCH /api/v1/me/search-filters/{userSearchFilterId}")
            .jwt_claim("sub", user.user_id)
            .path_parameter("userSearchFilterId", initial.user_search_filter_id)
            .body_serde(&patch)
            .build(),
        context: Default::default(),
    };

    let response = handle(
        lambda_event,
        &service,
        &get_product_service,
        &personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response.status_code);

    let json = extract_apigw_response_json_body!(response);
    let actual: UserSearchFilterData = serde_json::from_value(json).unwrap();
    assert_eq!(patch.name.unwrap(), actual.name);
    assert_eq!(
        patch.search.clone().unwrap().shop_name_query.unwrap(),
        actual.search.shop_name_query
    );
    assert_eq!(
        patch.search.clone().unwrap().price_query.unwrap(),
        actual.search.price_query.unwrap()
    );
}
