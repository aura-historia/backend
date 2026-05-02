use common::language::domain::Language;
use common::sort::{Sort, SortOrder};
use fake::{Fake, Faker};
use product::core::title::Title;
use product_classification::category::{
    category_search::CategorySearch,
    document::CategoryDocument,
    opensearch_repository::{CategoryOpenSearchRepository, CategoryOpenSearchRepositoryImpl},
    sort_category_field::SortCategoryField,
};
use product_classification::period::{
    document::PeriodDocument,
    opensearch_repository::{PeriodOpenSearchRepository, PeriodOpenSearchRepositoryImpl},
};
use std::time::Duration;
use test_api::*;

#[localstack_test(services = [OpenSearch()])]
async fn should_respond_no_documents_when_index_empty_for_exact_knn() {
    let repository = CategoryOpenSearchRepositoryImpl::new(get_opensearch_client().await);

    let actual = repository
        .exact_k_nn(&fake::vec![f32; 768], 3)
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

    for mut doc in fake::vec![CategoryDocument; 20] {
        doc.display_name_en = Faker.fake();
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

#[localstack_test(services = [OpenSearch()])]
async fn should_return_category_candidates_when_title_matches_metadata_for_hybrid_search() {
    let repository = CategoryOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let mut expected = Faker.fake::<CategoryDocument>();
    expected.meta_name = "Furniture Möbel Meubles Muebles Mobili".to_string();
    expected.meta_description =
        "Cabinet cupboard armoire Schrank buffet credenza antique storage furniture".to_string();
    expected.meta_keywords = vec![
        "cabinet".to_string(),
        "cupboard".to_string(),
        "Schrank".to_string(),
        "armoire".to_string(),
        "mueble".to_string(),
        "credenza".to_string(),
    ];

    let mut other = Faker.fake::<CategoryDocument>();
    other.meta_name = "Jewelry".to_string();
    other.meta_description = "Rings and necklaces".to_string();
    other.meta_keywords = vec!["ring".to_string(), "necklace".to_string()];

    let _ = repository
        .index_category_document(expected.clone())
        .await
        .unwrap();
    let _ = repository.index_category_document(other).await.unwrap();
    refresh_index("categories").await;
    tokio::time::sleep(Duration::from_secs(3)).await;

    let actual = repository
        .hybrid_search(
            &Title::from("Antique cabinet Schrank"),
            &expected.embedding,
            3,
        )
        .await
        .unwrap();

    assert_eq!(
        expected.category_id,
        actual.hits.hits.first().unwrap().source.category_id
    );
}

#[localstack_test(services = [OpenSearch()])]
async fn should_return_period_candidates_when_title_matches_metadata_for_hybrid_search() {
    let repository = PeriodOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let mut expected = Faker.fake::<PeriodDocument>();
    expected.meta_name = "Art Deco Art déco arte deco".to_string();
    expected.meta_description =
        "Geometric streamlined zigzag chrome 1920s 1930s design Moderne".to_string();
    expected.meta_keywords = vec![
        "art deco".to_string(),
        "zigzag".to_string(),
        "streamline".to_string(),
        "1920".to_string(),
        "1930".to_string(),
    ];

    let mut other = Faker.fake::<PeriodDocument>();
    other.meta_name = "Baroque".to_string();
    other.meta_description = "Dramatic carved gilded seventeenth century".to_string();
    other.meta_keywords = vec!["baroque".to_string(), "rocaille".to_string()];

    let _ = repository
        .index_period_document(expected.clone())
        .await
        .unwrap();
    let _ = repository.index_period_document(other).await.unwrap();
    refresh_index("periods").await;
    tokio::time::sleep(Duration::from_secs(3)).await;

    let actual = repository
        .hybrid_search(&Title::from("Art Deco chrome lamp"), &expected.embedding, 3)
        .await
        .unwrap();

    assert_eq!(
        expected.period_id,
        actual.hits.hits.first().unwrap().source.period_id
    );
}

#[localstack_test(services = [OpenSearch()])]
async fn should_sort_by_name_ascending_when_name_asc_for_search() {
    let repository = CategoryOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let mut doc_a = Faker.fake::<CategoryDocument>();
    doc_a.display_name_en = "Alpha".to_string();
    let mut doc_b = Faker.fake::<CategoryDocument>();
    doc_b.display_name_en = "Bravo".to_string();
    let mut doc_c = Faker.fake::<CategoryDocument>();
    doc_c.display_name_en = "Charlie".to_string();

    for doc in [doc_b.clone(), doc_c.clone(), doc_a.clone()] {
        let _ = repository.index_category_document(doc).await.unwrap();
    }
    refresh_index("categories").await;
    tokio::time::sleep(Duration::from_secs(3)).await;

    let search = CategorySearch {
        language: Language::En,
        name_query: None,
    };
    let actual: Vec<_> = repository
        .search_category_documents(
            &search,
            &Sort {
                sort: SortCategoryField::Name,
                order: SortOrder::Asc,
            },
        )
        .await
        .unwrap()
        .hits
        .hits
        .into_iter()
        .map(|hit| hit.source.display_name_en)
        .collect();

    assert_eq!(vec!["Alpha", "Bravo", "Charlie"], actual);
}

#[localstack_test(services = [OpenSearch()])]
async fn should_sort_by_name_descending_when_name_desc_for_search() {
    let repository = CategoryOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let mut doc_a = Faker.fake::<CategoryDocument>();
    doc_a.display_name_en = "Alpha".to_string();
    let mut doc_b = Faker.fake::<CategoryDocument>();
    doc_b.display_name_en = "Bravo".to_string();
    let mut doc_c = Faker.fake::<CategoryDocument>();
    doc_c.display_name_en = "Charlie".to_string();

    for doc in [doc_b.clone(), doc_a.clone(), doc_c.clone()] {
        let _ = repository.index_category_document(doc).await.unwrap();
    }
    refresh_index("categories").await;
    tokio::time::sleep(Duration::from_secs(3)).await;

    let search = CategorySearch {
        language: Language::En,
        name_query: None,
    };
    let actual: Vec<_> = repository
        .search_category_documents(
            &search,
            &Sort {
                sort: SortCategoryField::Name,
                order: SortOrder::Desc,
            },
        )
        .await
        .unwrap()
        .hits
        .hits
        .into_iter()
        .map(|hit| hit.source.display_name_en)
        .collect();

    assert_eq!(vec!["Charlie", "Bravo", "Alpha"], actual);
}

#[localstack_test(services = [OpenSearch()])]
async fn should_sort_by_created_ascending_when_created_asc_for_search() {
    let repository = CategoryOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let mut doc_old = Faker.fake::<CategoryDocument>();
    doc_old.created = time::macros::datetime!(2020-01-01 0:00 UTC);
    let mut doc_mid = Faker.fake::<CategoryDocument>();
    doc_mid.created = time::macros::datetime!(2022-06-15 0:00 UTC);
    let mut doc_new = Faker.fake::<CategoryDocument>();
    doc_new.created = time::macros::datetime!(2024-12-01 0:00 UTC);

    for doc in [doc_mid.clone(), doc_new.clone(), doc_old.clone()] {
        let _ = repository.index_category_document(doc).await.unwrap();
    }
    refresh_index("categories").await;
    tokio::time::sleep(Duration::from_secs(3)).await;

    let search = CategorySearch {
        language: Language::En,
        name_query: None,
    };
    let actual: Vec<_> = repository
        .search_category_documents(
            &search,
            &Sort {
                sort: SortCategoryField::Created,
                order: SortOrder::Asc,
            },
        )
        .await
        .unwrap()
        .hits
        .hits
        .into_iter()
        .map(|hit| hit.source.created)
        .collect();

    assert_eq!(
        vec![
            time::macros::datetime!(2020-01-01 0:00 UTC),
            time::macros::datetime!(2022-06-15 0:00 UTC),
            time::macros::datetime!(2024-12-01 0:00 UTC),
        ],
        actual
    );
}

#[localstack_test(services = [OpenSearch()])]
async fn should_sort_by_updated_descending_when_updated_desc_for_search() {
    let repository = CategoryOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let mut doc_old = Faker.fake::<CategoryDocument>();
    doc_old.updated = time::macros::datetime!(2020-01-01 0:00 UTC);
    let mut doc_mid = Faker.fake::<CategoryDocument>();
    doc_mid.updated = time::macros::datetime!(2022-06-15 0:00 UTC);
    let mut doc_new = Faker.fake::<CategoryDocument>();
    doc_new.updated = time::macros::datetime!(2024-12-01 0:00 UTC);

    for doc in [doc_mid.clone(), doc_old.clone(), doc_new.clone()] {
        let _ = repository.index_category_document(doc).await.unwrap();
    }
    refresh_index("categories").await;
    tokio::time::sleep(Duration::from_secs(3)).await;

    let search = CategorySearch {
        language: Language::En,
        name_query: None,
    };
    let actual: Vec<_> = repository
        .search_category_documents(
            &search,
            &Sort {
                sort: SortCategoryField::Updated,
                order: SortOrder::Desc,
            },
        )
        .await
        .unwrap()
        .hits
        .hits
        .into_iter()
        .map(|hit| hit.source.updated)
        .collect();

    assert_eq!(
        vec![
            time::macros::datetime!(2024-12-01 0:00 UTC),
            time::macros::datetime!(2022-06-15 0:00 UTC),
            time::macros::datetime!(2020-01-01 0:00 UTC),
        ],
        actual
    );
}
