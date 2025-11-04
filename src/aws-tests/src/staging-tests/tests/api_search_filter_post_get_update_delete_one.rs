use aws_tests_common::get_cfn_output;
use common::language::data::LanguageData;
use fake::{Fake, Faker};
use search_filter::data::user_search_filter_data::UserSearchFilterData;
use search_filter_api_patch_search_filter::patch::{
    PatchItemSearchData, PatchUserSearchFilterData,
};
use search_filter_api_post_search_filter::post::PostUserSearchFilterData;
use staging_tests::create_random_test_user;
use staging_tests_macros::staging_test;

#[staging_test]
async fn should_401_when_unauthorized_for_post() {
    let url = format!(
        "{}/api/v1/me/search-filters",
        get_cfn_output().api_gateway_endpoint_url,
    );
    let response = reqwest::Client::new().post(url).send().await.unwrap();
    assert_eq!(401, response.status());
}

#[staging_test]
async fn should_401_when_unauthorized_for_delete() {
    let url = format!(
        "{}/api/v1/me/search-filters/foo",
        get_cfn_output().api_gateway_endpoint_url,
    );
    let response = reqwest::Client::new().delete(url).send().await.unwrap();
    assert_eq!(401, response.status());
}

#[staging_test]
async fn should_401_when_unauthorized_for_get_one() {
    let url = format!(
        "{}/api/v1/me/search-filters/foo",
        get_cfn_output().api_gateway_endpoint_url,
    );
    let response = reqwest::Client::new().get(url).send().await.unwrap();
    assert_eq!(401, response.status());
}

#[staging_test]
async fn should_create_and_get_and_delete_and_verify_not_exists() {
    let user = create_random_test_user().await;

    // Create new
    let expected = Faker.fake::<PostUserSearchFilterData>();
    let post_url = format!(
        "{}/api/v1/me/search-filters",
        get_cfn_output().api_gateway_endpoint_url,
    );
    let post_response = reqwest::Client::new()
        .post(post_url)
        .json(&expected)
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(201, post_response.status());
    let posted = post_response.json::<UserSearchFilterData>().await.unwrap();
    assert_eq!(&expected.search_filter_name, &posted.name);
    assert_eq!(&expected.search_filter, &posted.search);
    assert_eq!(user.sub.to_string(), posted.user_id.to_string());

    // Get posted
    let get_url = format!(
        "{}/api/v1/me/search-filters/{}",
        get_cfn_output().api_gateway_endpoint_url,
        posted.search_filter_id
    );
    let get_response = reqwest::Client::new()
        .get(get_url)
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(200, get_response.status());
    let gotten = get_response.json::<UserSearchFilterData>().await.unwrap();
    assert_eq!(&expected.search_filter, &gotten.search);
    assert_eq!(posted.search_filter_id, gotten.search_filter_id);
    assert_eq!(user.sub.to_string(), gotten.user_id.to_string());

    // Update gotten
    let patch_url = format!(
        "{}/api/v1/me/search-filters/{}",
        get_cfn_output().api_gateway_endpoint_url,
        posted.search_filter_id
    );
    let patch = PatchUserSearchFilterData {
        name: None,
        search: Some(PatchItemSearchData {
            language: Some(LanguageData::Fr),
            currency: None,
            item_query: Some("weesl bee wuff".try_into().unwrap()),
            shop_name_query: None,
            price_query: None,
            state_query: None,
            created_query: None,
            updated_query: None,
        }),
    };
    let patch_response = reqwest::Client::new()
        .patch(patch_url)
        .bearer_auth(&user.access_token)
        .json(&patch)
        .send()
        .await
        .unwrap();
    assert_eq!(200, patch_response.status());
    let patched = patch_response.json::<UserSearchFilterData>().await.unwrap();
    assert_eq!(
        &patch.search.clone().unwrap().language.unwrap(),
        &patched.search.language
    );
    assert_eq!(
        &patch.search.unwrap().item_query.unwrap(),
        &patched.search.item_query
    );
    assert_eq!(posted.search_filter_id, patched.search_filter_id);
    assert_eq!(user.sub.to_string(), patched.user_id.to_string());

    // Delete patched
    let get_url = format!(
        "{}/api/v1/me/search-filters/{}",
        get_cfn_output().api_gateway_endpoint_url,
        patched.search_filter_id
    );
    let get_response = reqwest::Client::new()
        .delete(get_url)
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(204, get_response.status());

    // Get deleted
    let get_url = format!(
        "{}/api/v1/me/search-filters/{}",
        get_cfn_output().api_gateway_endpoint_url,
        posted.search_filter_id
    );
    let get_response = reqwest::Client::new()
        .get(get_url)
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(404, get_response.status());
    let json = get_response.json::<serde_json::Value>().await.unwrap();
    assert_eq!("SEARCH_FILTER_NOT_FOUND", json["error"]);
}
