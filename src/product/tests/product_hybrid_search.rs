use common::currency::domain::Currency;
use common::language::document::{LanguageDocument, TextDocument};
use common::language::domain::Language;
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
