use opensearch::indices::IndicesExistsParts;
use test_api::*;

#[aura_integration_test(services = [OpenSearch()])]
async fn should_run_without_errors() {}

#[aura_integration_test(services = [OpenSearch()])]
async fn should_set_up_indices() {
    for index in ["product-listings", "shops", "user_search_filters", "users"] {
        let exists_response = get_opensearch_client()
            .await
            .indices()
            .exists(IndicesExistsParts::Index(&[index]))
            .send()
            .await
            .expect("shouldn't fail retrieving indices-exist query")
            .error_for_status_code()
            .expect("shouldn't fail verifying indices-exist status");

        assert!(
            exists_response.status_code().is_success(),
            "OpenSearch index '{index}' should exist after setup"
        );
    }
}
