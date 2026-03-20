use aws_tests_common::get_cfn_output;
use common::period_key::PeriodId;
use product_classification::period::{
    core::Period, data::get_period_summary_data::GetPeriodSummaryData,
    period_search::PeriodSearchData,
};
use test_api::*;

#[localstack_test(services = [Cloudformation()])]
async fn should_respond_404_when_period_does_not_exist() {
    let url = format!(
        "{}/api/v1/periods/{}",
        get_cfn_output().api_gateway_endpoint_url,
        PeriodId::from("non-existent-period-id")
    );
    let response = reqwest::get(url).await.unwrap();
    assert_eq!(404, response.status());

    let body = response.json::<serde_json::Value>().await.unwrap();
    assert_eq!(404, body["status"]);
    assert_eq!("NOT_FOUND", body["error"]);
}

#[localstack_test(services = [Cloudformation()])]
async fn should_get_all_periods() {
    let url = format!(
        "{}/api/v1/periods",
        get_cfn_output().api_gateway_endpoint_url
    );
    let response = reqwest::get(url).await.unwrap();
    assert_eq!(200, response.status());

    let body = response.json::<Vec<GetPeriodSummaryData>>().await.unwrap();
    assert!(body.is_empty());
}

#[localstack_test(services = [Cloudformation()])]
async fn should_search_periods_with_empty_query() {
    let url = format!(
        "{}/api/v1/periods/search",
        get_cfn_output().api_gateway_endpoint_url
    );
    let response = reqwest::Client::new()
        .post(url)
        .json(&PeriodSearchData::default())
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());

    let body = response.json::<Vec<GetPeriodSummaryData>>().await.unwrap();
    assert!(body.is_empty());
}

#[localstack_test(services = [Cloudformation()])]
async fn should_search_periods_with_name_query() {
    let periods = Period::load_periods();
    let expected = periods.first().unwrap();
    let name = expected.display_name.values().next().unwrap().to_string();

    let url = format!(
        "{}/api/v1/periods/search",
        get_cfn_output().api_gateway_endpoint_url
    );
    let search = PeriodSearchData {
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

    let body = response.json::<Vec<GetPeriodSummaryData>>().await.unwrap();
    assert!(body.is_empty());
}

#[localstack_test(services = [Cloudformation()])]
async fn should_search_periods_with_get_simple_search() {
    let url = format!(
        "{}/api/v1/periods?language=de&nameQuery=baroque",
        get_cfn_output().api_gateway_endpoint_url
    );
    let response = reqwest::get(url).await.unwrap();
    assert_eq!(200, response.status());
}
