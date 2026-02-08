use fake::{Fake, Faker};
use product_classification::category::{
    document::CategoryDocument,
    opensearch_repository::{CategoryOpenSearchRepository, CategoryOpenSearchRepositoryImpl},
};
use test_api::*;

#[localstack_test(services = [OpenSearch()])]
async fn should_respond_no_documents_when_index_empty_for_exact_knn() {
    let repository = CategoryOpenSearchRepositoryImpl::new(get_opensearch_client().await);

    let actual = repository
        .exact_k_nn(&fake::vec![f32; 1024], 3)
        .await
        .unwrap();

    assert!(actual.hits.hits.is_empty());
    assert_eq!(0, actual.hits.total.value);
}

#[localstack_test(services = [OpenSearch()])]
async fn should_respond_single_document_when_ingested_for_exact_knn() {
    let repository = CategoryOpenSearchRepositoryImpl::new(get_opensearch_client().await);

    let document = Faker.fake::<CategoryDocument>();
    let _ = repository
        .index_category_document(document.clone())
        .await
        .unwrap();
    refresh_index("categories").await;
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let actual = repository
        .exact_k_nn(&document.embedding.clone(), 3)
        .await
        .unwrap()
        .hits
        .hits
        .into_iter()
        .map(|hit| hit.source)
        .next()
        .unwrap();

    assert_eq!(document.category_id, actual.category_id);
    assert_eq!(document.meta_name, actual.meta_name);
    assert_eq!(document.meta_description, actual.meta_description);
}
