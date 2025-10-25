use aws_tests_common::get_cfn_output;
use common::pagination::page::api::PaginatedData;
use fake::{Fake, Faker};
use search_filter_core::{search_filter::SearchFilter, search_filter_name::SearchFilterName};
use search_filter_data::user_search_filter_data::UserSearchFilterData;
use search_filter_dynamodb::repository::SearchFilterDynamoDbRepositoryImpl;
use search_filter_service::service::{SearchFilterService, SearchFilterServiceImpl};
use staging_tests::{create_random_test_user, get_dynamodb_client};
use staging_tests_macros::staging_test;

#[staging_test]
async fn should_401_when_unauthorized() {
    let url = format!(
        "{}/api/v1/search-filters?sort=created&order=asc",
        get_cfn_output().api_gateway_endpoint_url,
    );
    let response = reqwest::Client::new().get(url).send().await.unwrap();
    assert_eq!(401, response.status());
}

#[staging_test]
async fn should_return_actual_search_filters_when_authorized() {
    let repository = SearchFilterDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &get_cfn_output().dynamodb_table_1_name,
    );
    let service = SearchFilterServiceImpl::new(&repository);

    let user = create_random_test_user().await;
    let expected1 = Faker.fake::<SearchFilter>();
    let expected1_name = Faker.fake::<SearchFilterName>();
    let expected2 = Faker.fake::<SearchFilter>();
    let expected2_name = Faker.fake::<SearchFilterName>();
    service
        .save_search_filter(&user.sub.into(), expected1_name.clone(), expected1.clone())
        .await
        .unwrap();
    service
        .save_search_filter(&user.sub.into(), expected2_name.clone(), expected2.clone())
        .await
        .unwrap();

    let url = format!(
        "{}/api/v1/search-filters?sort=created&order=asc",
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
    assert_eq!(expected1, SearchFilter::from(actual1.search_filter));
    assert_eq!(expected2, SearchFilter::from(actual2.search_filter));
    assert_eq!(expected1_name, actual1.search_filter_name);
    assert_eq!(expected2_name, actual2.search_filter_name);
}
