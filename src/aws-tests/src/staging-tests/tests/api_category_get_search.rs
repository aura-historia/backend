use aws_tests_common::get_cfn_output;
use common::category_key::CategoryId;
use product_classification::category::{
    category_search::CategorySearchData, core::Category,
    data::get_category_summary_data::GetCategorySummaryData,
};
use staging_tests::staging_test;

#[staging_test]
async fn should_respond_404_when_category_does_not_exist() {
    let url = format!(
        "{}/api/v1/categories/{}",
        get_cfn_output().api_gateway_endpoint_url,
        CategoryId::from("non-existent-category-id")
    );
    let response = reqwest::get(url).await.unwrap();
    assert_eq!(404, response.status());

    let body = response.json::<serde_json::Value>().await.unwrap();
    assert_eq!(404, body["status"]);
    assert_eq!("NOT_FOUND", body["error"]);
}

#[staging_test]
async fn should_get_all_categories() {
    let url = format!(
        "{}/api/v1/categories",
        get_cfn_output().api_gateway_endpoint_url
    );
    let response = reqwest::get(url).await.unwrap();
    assert_eq!(200, response.status());

    let body = response
        .json::<Vec<GetCategorySummaryData>>()
        .await
        .unwrap();
    assert!(body.is_empty());
}

#[staging_test]
async fn should_search_categories_with_empty_query() {
    let url = format!(
        "{}/api/v1/categories/search",
        get_cfn_output().api_gateway_endpoint_url
    );
    let response = reqwest::Client::new()
        .post(url)
        .json(&CategorySearchData::default())
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());

    let body = response
        .json::<Vec<GetCategorySummaryData>>()
        .await
        .unwrap();
    assert!(body.is_empty());
}

#[staging_test]
async fn should_search_categories_with_name_query() {
    let categories = Category::load_categories();
    let expected = categories.first().unwrap();
    let name = expected.display_name.values().next().unwrap().to_string();

    let url = format!(
        "{}/api/v1/categories/search",
        get_cfn_output().api_gateway_endpoint_url
    );
    let search = CategorySearchData {
        language: common::language::data::LanguageData::De,
        name_query: Some(name.try_into().unwrap()),
    };
    let response = reqwest::Client::new()
        .post(url)
        .json(&search)
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());

    let body = response
        .json::<Vec<GetCategorySummaryData>>()
        .await
        .unwrap();
    assert!(body.is_empty());
}

#[staging_test]
async fn should_search_categories_with_get_simple_search() {
    let url = format!(
        "{}/api/v1/categories?language=de&nameQuery=furniture",
        get_cfn_output().api_gateway_endpoint_url
    );
    let response = reqwest::get(url).await.unwrap();
    assert_eq!(200, response.status());
}
