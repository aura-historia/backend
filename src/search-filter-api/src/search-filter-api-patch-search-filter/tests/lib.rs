use common::query::range_query::RangeQuery;
use common::shop_name::ShopName;
use common::user_id::UserId;
use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use product::core::product_search::ProductSearch;
use search_filter::data::user_search_filter_data::UserSearchFilterData;
use search_filter::dynamodb::repository::UserSearchFilterDynamoDbRepositoryImpl;
use search_filter::service::user_search_filter_service::{
    UserSearchFilterService, UserSearchFilterServiceImpl,
};
use search_filter_api_patch_search_filter::{
    handler,
    patch::{PatchProductSearchData, PatchUserSearchFilterData},
};
use test_api::*;

#[localstack_test(services = [DynamoDB()])]
async fn should_update_search_filter() {
    let repository =
        UserSearchFilterDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let service = UserSearchFilterServiceImpl::new(&repository);

    let user_id = UserId::new();
    let initial = service
        .save_user_search_filter(&user_id, Faker.fake(), Faker.fake::<ProductSearch>())
        .await
        .unwrap();

    let patch = PatchUserSearchFilterData {
        name: Some("thorbens filter".into()),
        search: Some(PatchProductSearchData {
            language: None,
            currency: None,
            product_query: None,
            shop_name_query: Some(HashSet::from_iter([ShopName::from("Whoop boop woah")])),
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
            created_query: None,
            updated_query: None,
        }),
    };
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::PATCH)
            .jwt_claim("sub", user_id)
            .path_parameter("userSearchFilterId", initial.user_search_filter_id)
            .body_serde(&patch)
            .build(),
        context: Default::default(),
    };

    let response = handler(lambda_event, &service).await.unwrap();
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
