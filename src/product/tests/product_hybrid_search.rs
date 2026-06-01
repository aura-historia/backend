use common::currency::domain::Currency;
use common::language::document::{LanguageDocument, TextDocument};
use common::language::domain::Language;
use common::pagination::cursor::Cursor;
use common::product_state::domain::ProductState;
use fake::{Fake, Faker};
use opensearch::http::Url;
use product::core::product_search::ProductSearch;
use product::opensearch::product_document::ProductDocument;
use product::opensearch::product_state_document::ProductStateDocument;
use product::opensearch::repository::{
    ProductOpenSearchRepository, ProductOpenSearchRepositoryImpl,
};
use product::service::hybrid_search::hybrid_search;
use shop::opensearch::shop_type_document::ShopTypeDocument;
use std::time::Duration;
use test_api::*;
use time::OffsetDateTime;

fn one_hot_embedding(slot: usize, value: f32) -> [f32; 768] {
    let mut v = [0.0_f32; 768];
    v[slot] = value;
    v
}

fn set_titles(doc: &mut ProductDocument, title: &str) {
    doc.title_en = Some(title.to_string());
    doc.title_native = TextDocument {
        text: title.to_string(),
        language: LanguageDocument::En,
    };
}

fn make_product_doc(customize: impl FnOnce(&mut ProductDocument)) -> ProductDocument {
    let mut doc: ProductDocument = Faker.fake();
    doc.embedding = None;
    doc.state = ProductStateDocument::Available;
    doc.shop_type = ShopTypeDocument::CommercialDealer;
    doc.url = Url::parse("https://example.com/product").unwrap();
    doc.created = OffsetDateTime::now_utc();
    doc.updated = OffsetDateTime::now_utc();
    customize(&mut doc);
    doc
}

fn search_with_query(query: &str) -> ProductSearch {
    ProductSearch {
        language: Language::En,
        currency: Currency::Eur,
        product_query: Some(query.try_into().unwrap()),
        shop_name_query: Default::default(),
        exclude_shop_name_query: Default::default(),
        seller_name_query: Default::default(),
        exclude_seller_name_query: Default::default(),
        shop_type_query: Default::default(),
        country_query: Default::default(),
        continent_query: Default::default(),
        geo_address_distance_query: None,
        price_query: None,
        state_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
        shop_slug_id_query: Default::default(),
        exclude_shop_slug_id_query: Default::default(),
        seller_slug_id_query: Default::default(),
        exclude_seller_slug_id_query: Default::default(),
    }
}

#[localstack_test(services = [OpenSearch()])]
async fn should_exclude_vector_only_noise_when_query_is_precision_for_dynamic_hybrid_search() {
    let query = "Rolex Submariner 1965";
    let exact_match = make_product_doc(|doc| {
        set_titles(doc, query);
        doc.embedding = Some(one_hot_embedding(31, 1.0).into());
    });
    let semantic_noise = make_product_doc(|doc| {
        set_titles(doc, "totally unrelated text");
        doc.embedding = Some(one_hot_embedding(30, 1.0).into());
    });

    let client = get_opensearch_client().await;
    let repository = ProductOpenSearchRepositoryImpl::new(client);
    repository
        .create_product_documents(vec![exact_match.clone(), semantic_noise.clone()])
        .await
        .unwrap();
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_secs(2)).await;

    let search = search_with_query(query);
    let outcome = hybrid_search(
        &repository,
        &search,
        &one_hot_embedding(30, 1.0),
        &None,
        &[search.language],
    )
    .await
    .unwrap();

    let returned_ids: std::collections::HashSet<_> = outcome
        .items
        .items
        .iter()
        .map(|item| item.product_id)
        .collect();
    assert_eq!(returned_ids.len(), 1);
    assert!(returned_ids.contains(&exact_match.product_id));
    assert!(
        !returned_ids.contains(&semantic_noise.product_id),
        "precision-oriented queries must not drift into unrelated vector-only results"
    );
}

#[localstack_test(services = [OpenSearch()])]
async fn should_drop_low_similarity_vector_hits_when_query_is_visual_for_dynamic_hybrid_search() {
    let query = "blue ceramic ornate vase";
    let bm25_anchor = make_product_doc(|doc| {
        set_titles(doc, query);
        doc.embedding = Some(one_hot_embedding(41, 1.0).into());
    });
    let vector_target = make_product_doc(|doc| {
        set_titles(doc, "totally unrelated text");
        doc.embedding = Some(one_hot_embedding(40, 1.0).into());
    });
    let vector_noise: Vec<ProductDocument> = (0..6)
        .map(|offset| {
            make_product_doc(|doc| {
                set_titles(doc, "totally unrelated text");
                doc.embedding = Some(one_hot_embedding(100 + offset, 1.0).into());
            })
        })
        .collect();

    let client = get_opensearch_client().await;
    let repository = ProductOpenSearchRepositoryImpl::new(client);
    let mut docs = vec![bm25_anchor.clone(), vector_target.clone()];
    docs.extend(vector_noise.clone());
    repository.create_product_documents(docs).await.unwrap();
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_secs(2)).await;

    let search = search_with_query(query);
    let outcome = hybrid_search(
        &repository,
        &search,
        &one_hot_embedding(40, 1.0),
        &None,
        &[search.language],
    )
    .await
    .unwrap();

    let returned_ids: std::collections::HashSet<_> = outcome
        .items
        .items
        .iter()
        .map(|item| item.product_id)
        .collect();
    assert!(returned_ids.contains(&bm25_anchor.product_id));
    assert!(returned_ids.contains(&vector_target.product_id));
    for noise in &vector_noise {
        assert!(
            !returned_ids.contains(&noise.product_id),
            "low-similarity vector-only tail hit must be dropped"
        );
    }
}

#[localstack_test(services = [OpenSearch()])]
async fn should_rank_dual_branch_match_first_for_dynamic_hybrid_search() {
    let query = "blue ceramic ornate vase";
    let dual_match = make_product_doc(|doc| {
        set_titles(doc, query);
        doc.embedding = Some(one_hot_embedding(60, 1.0).into());
    });
    let bm25_only = make_product_doc(|doc| {
        set_titles(doc, query);
        doc.embedding = Some(one_hot_embedding(61, 1.0).into());
    });
    let vector_only = make_product_doc(|doc| {
        set_titles(doc, "totally unrelated text");
        doc.embedding = Some(one_hot_embedding(60, 1.0).into());
    });

    let client = get_opensearch_client().await;
    let repository = ProductOpenSearchRepositoryImpl::new(client);
    repository
        .create_product_documents(vec![
            dual_match.clone(),
            bm25_only.clone(),
            vector_only.clone(),
        ])
        .await
        .unwrap();
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_secs(2)).await;

    let search = search_with_query(query);
    let outcome = hybrid_search(
        &repository,
        &search,
        &one_hot_embedding(60, 1.0),
        &None,
        &[search.language],
    )
    .await
    .unwrap();

    assert_eq!(
        outcome.items.items.first().unwrap().product_id,
        dual_match.product_id
    );
    let returned_ids: std::collections::HashSet<_> = outcome
        .items
        .items
        .iter()
        .map(|item| item.product_id)
        .collect();
    assert!(returned_ids.contains(&bm25_only.product_id));
    assert!(returned_ids.contains(&vector_only.product_id));
}

#[localstack_test(services = [OpenSearch()])]
async fn should_page_dynamic_hybrid_search_without_duplicate_products() {
    let query = "art deco lamp";
    let docs: Vec<ProductDocument> = (0..5)
        .map(|idx| {
            make_product_doc(|doc| {
                set_titles(doc, &format!("{query} {idx}"));
                doc.embedding = Some(one_hot_embedding(70 + idx, 1.0).into());
            })
        })
        .collect();

    let client = get_opensearch_client().await;
    let repository = ProductOpenSearchRepositoryImpl::new(client);
    repository
        .create_product_documents(docs.clone())
        .await
        .unwrap();
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_secs(2)).await;

    let search = search_with_query(query);
    let first = hybrid_search(
        &repository,
        &search,
        &one_hot_embedding(70, 1.0),
        &Some(Cursor {
            size: 2,
            search_after: None,
        }),
        &[search.language],
    )
    .await
    .unwrap();

    assert_eq!(first.items.items.len(), 2);
    assert!(first.items.cursor.search_after.is_some());

    let second = hybrid_search(
        &repository,
        &search,
        &one_hot_embedding(70, 1.0),
        &Some(first.items.cursor.clone()),
        &[search.language],
    )
    .await
    .unwrap();

    assert!(!second.items.items.is_empty());
    let first_ids: std::collections::HashSet<_> = first
        .items
        .items
        .iter()
        .map(|item| item.product_id)
        .collect();
    for item in &second.items.items {
        assert!(
            !first_ids.contains(&item.product_id),
            "cursor pagination must not repeat products from the previous page"
        );
    }
}

#[localstack_test(services = [OpenSearch()])]
async fn should_apply_filters_to_semantic_branch_for_dynamic_hybrid_search() {
    let query = "blue ceramic ornate vase";
    let bm25_anchor = make_product_doc(|doc| {
        set_titles(doc, query);
        doc.state = ProductStateDocument::Available;
        doc.embedding = Some(one_hot_embedding(81, 1.0).into());
    });
    let available_vector = make_product_doc(|doc| {
        set_titles(doc, "totally unrelated text");
        doc.state = ProductStateDocument::Available;
        doc.embedding = Some(one_hot_embedding(80, 1.0).into());
    });
    let sold_vector = make_product_doc(|doc| {
        set_titles(doc, "totally unrelated text");
        doc.state = ProductStateDocument::Sold;
        doc.embedding = Some(one_hot_embedding(80, 1.0).into());
    });

    let client = get_opensearch_client().await;
    let repository = ProductOpenSearchRepositoryImpl::new(client);
    repository
        .create_product_documents(vec![
            bm25_anchor.clone(),
            available_vector.clone(),
            sold_vector.clone(),
        ])
        .await
        .unwrap();
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_secs(2)).await;

    let mut search = search_with_query(query);
    search.state_query = std::collections::HashSet::from([ProductState::Available]).into();
    let outcome = hybrid_search(
        &repository,
        &search,
        &one_hot_embedding(80, 1.0),
        &None,
        &[search.language],
    )
    .await
    .unwrap();

    let returned_ids: std::collections::HashSet<_> = outcome
        .items
        .items
        .iter()
        .map(|item| item.product_id)
        .collect();
    assert!(returned_ids.contains(&bm25_anchor.product_id));
    assert!(returned_ids.contains(&available_vector.product_id));
    assert!(
        !returned_ids.contains(&sold_vector.product_id),
        "semantic branch must apply the same filters as the BM25 branch"
    );
}
