use common::language::domain::Language;
use common::sort::{Sort, SortOrder};
use fake::{Fake, Faker};
use product_classification::category::{
    category_search::CategorySearch,
    document::CategoryDocument,
    opensearch_repository::{CategoryOpenSearchRepository, CategoryOpenSearchRepositoryImpl},
    sort_category_field::SortCategoryField,
};
use std::time::Duration;
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

#[localstack_test(services = [OpenSearch()])]
async fn should_search_category_documents_when_name_query_supplied() {
    let repository = CategoryOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let mut expected = Faker.fake::<CategoryDocument>();
    expected.display_name_en = "UniqueTestCategoryName".to_string();

    let _ = repository
        .index_category_document(expected.clone())
        .await
        .unwrap();

    for doc in fake::vec![CategoryDocument; 20] {
        let _ = repository.index_category_document(doc).await.unwrap();
    }
    refresh_index("categories").await;
    tokio::time::sleep(Duration::from_secs(3)).await;

    let search = CategorySearch {
        language: Language::En,
        name_query: Some("UniqueTestCategoryName".try_into().unwrap()),
    };
    let actual = repository
        .search_category_documents(
            &search,
            &Sort {
                sort: SortCategoryField::Score,
                order: SortOrder::Desc,
            },
        )
        .await
        .unwrap();

    assert!(
        actual
            .hits
            .hits
            .iter()
            .any(|hit| hit.source.category_id == expected.category_id)
    );
}
