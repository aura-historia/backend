use aws_tests_common::get_cfn_output;
use common::api::collection::GetCollectionData;
use fake::{Fake, Faker};
use search_filter_core::search_filter::SearchFilter;
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
    let expected2 = Faker.fake::<SearchFilter>();
    service
        .save_search_filter(&user.sub.into(), expected1.clone())
        .await
        .unwrap();
    service
        .save_search_filter(&user.sub.into(), expected2.clone())
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
        .json::<GetCollectionData<UserSearchFilterData>>()
        .await
        .unwrap();
    assert_eq!(2, actual.pagination.total);

    let actual1: SearchFilter = actual.items.first().unwrap().clone().search_filter.into();
    let actual2: SearchFilter = actual.items.get(1).unwrap().clone().search_filter.into();
    assert_eq!(expected1, actual1);
    assert_eq!(expected2, actual2);
}
