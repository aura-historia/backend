use aws_tests_common::get_cfn_output;
use common::pagination::page::api::PaginatedData;
use fake::{Fake, Faker};
use product::core::item_search::ItemSearch;
use search_filter::core::user_search_filter_name::UserSearchFilterName;
use search_filter::data::user_search_filter_data::UserSearchFilterData;
use search_filter::dynamodb::repository::UserSearchFilterDynamoDbRepositoryImpl;
use search_filter::service::user_search_filter_service::{
    UserSearchFilterService, UserSearchFilterServiceImpl,
};
use staging_tests::{create_random_test_user, get_dynamodb_client};
use staging_tests_macros::staging_test;

#[staging_test]
async fn should_401_when_unauthorized() {
    let url = format!(
        "{}/api/v1/me/search-filters?sort=created&order=asc",
        get_cfn_output().api_gateway_endpoint_url,
    );
    let response = reqwest::Client::new().get(url).send().await.unwrap();
    assert_eq!(401, response.status());
}

#[staging_test]
async fn should_return_actual_search_filters_when_authorized() {
    let repository = UserSearchFilterDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &get_cfn_output().dynamodb_table_1_name,
    );
    let service = UserSearchFilterServiceImpl::new(&repository);

    let user = create_random_test_user().await;
    let expected1 = Faker.fake::<ItemSearch>();
    let expected1_name = Faker.fake::<UserSearchFilterName>();
    let expected2 = Faker.fake::<ItemSearch>();
    let expected2_name = Faker.fake::<UserSearchFilterName>();
    service
        .save_user_search_filter(&user.sub.into(), expected1_name.clone(), expected1.clone())
        .await
        .unwrap();
    service
        .save_user_search_filter(&user.sub.into(), expected2_name.clone(), expected2.clone())
        .await
        .unwrap();

    let url = format!(
        "{}/api/v1/me/search-filters?sort=created&order=asc",
        get_cfn_output().api_gateway_endpoint_url,
    );
    let response = reqwest::Client::new()
        .get(url)
        .bearer_auth(user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());

    let actual = response
        .json::<PaginatedData<UserSearchFilterData>>()
        .await
        .unwrap();
    assert_eq!(2, actual.total.unwrap());

    let actual1 = actual.items.first().unwrap().clone();
    let actual2 = actual.items.get(1).unwrap().clone();
    assert_eq!(expected1, ItemSearch::from(actual1.search));
    assert_eq!(expected2, ItemSearch::from(actual2.search));
    assert_eq!(expected1_name, actual1.name);
    assert_eq!(expected2_name, actual2.name);
}
