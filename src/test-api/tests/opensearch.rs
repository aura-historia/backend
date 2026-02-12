use opensearch::indices::IndicesExistsParts;
use test_api::*;

#[localstack_test(services = [OpenSearch()])]
async fn should_run_without_errors() {}

#[localstack_test(services = [OpenSearch()])]
async fn should_set_up_indices() {
    let exists_response = get_opensearch_client()
        .await
        .indices()
        .exists(IndicesExistsParts::Index(&["products"]))
        .send()
        .await
        .expect("shouldn't fail retrieving indices-exist query")
        .error_for_status_code()
        .expect("shouldn't fail verifying indices-exist status");

    assert!(exists_response.status_code().is_success())
}
