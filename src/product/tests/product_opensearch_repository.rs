use common::currency::domain::Currency;
use common::distance::domain::{Distance, DistanceUnit, GeoDistanceQuery};
use common::event_id::EventId;
use common::language::document::{LanguageDocument, TextDocument};
use common::language::domain::Language;
use common::pagination::cursor::Cursor;
use common::price::domain::MonetaryAmount;
use common::product_id::ProductId;
use common::product_lifecycle::document::ProductLifecycleDocument;
use common::product_state::domain::ProductState;
use common::query::any_of_query::AnyOfQuery;
use common::query::range_query::RangeQuery;
use common::seller_slug_id::SellerSlugId;
use common::shop_slug_id::ShopSlugId;
use common::shops_product_id::ShopsProductId;
use common::sort::{Sort, SortOrder};
use fake::{Fake, Faker};
use geo::core::continent::Continent;
use opensearch::http::Url;
use product::core::product_search::ProductSearch;
use product::core::sort_product_field::SortProductField;
use product::opensearch::product_document::ProductDocument;
use product::opensearch::product_state_document::ProductStateDocument;
use product::opensearch::product_update_document::ProductUpdateDocument;
use product::opensearch::repository::{
    ProductOpenSearchRepository, ProductOpenSearchRepositoryImpl,
};
use serde_json::json;
use shop::core::shop_type::ShopType;
use shop::opensearch::continent_document::ContinentDocument;
use shop::opensearch::shop_type_document::ShopTypeDocument;
use std::collections::HashMap;
use std::collections::HashSet;
use std::time::Duration;
use std::vec;
use test_api::*;
use time::OffsetDateTime;
use time::macros::datetime;

#[localstack_test(services = [OpenSearch()])]
async fn should_create_product_document() {
    let product_id = ProductId::new();
    let expected = ProductDocument {
        product_id,
        product_slug_id: Faker.fake(),
        shop_slug_id: Faker.fake(),
        seller_slug_id: Faker.fake(),
        event_id: Default::default(),
        shop_id: Default::default(),
        seller_id: Default::default(),
        shops_product_id: ShopsProductId::from("abcdefgh"),
        shop_name: "Foo".to_string(),
        seller_name: "Bar".to_string(),
        shop_type: ShopTypeDocument::CommercialDealer,
        structured_address_addressline: None,
        structured_address_addressline_extra: None,
        structured_address_locality: None,
        structured_address_region: None,
        structured_address_postal_code: None,
        structured_address_country: None,
        structured_address_continent: None,
        geo_address: None,
        title_native: TextDocument {
            text: "Foo".to_string(),
            language: LanguageDocument::Fr,
        },
        title_de: Some("Bar".to_string()),
        title_en: Some("Baz".to_string()),
        title_fr: Some("Bat".to_string()),
        title_es: Some("Bao".to_string()),
        title_it: Some("Bao".to_string()),
        price_eur: Some(99),
        price_usd: None,
        price_gbp: None,
        price_aud: None,
        price_cad: None,
        price_nzd: None,
        price_cny: None,
        price_brl: None,
        price_pln: None,
        price_try: None,
        price_jpy: None,
        price_czk: None,
        price_rub: None,
        price_aed: None,
        price_sar: None,
        price_hkd: None,
        price_sgd: None,
        price_chf: None,
        price_estimate_min_eur: None,
        price_estimate_min_usd: None,
        price_estimate_min_gbp: None,
        price_estimate_min_aud: None,
        price_estimate_min_cad: None,
        price_estimate_min_nzd: None,
        price_estimate_min_cny: None,
        price_estimate_min_brl: None,
        price_estimate_min_pln: None,
        price_estimate_min_try: None,
        price_estimate_min_jpy: None,
        price_estimate_min_czk: None,
        price_estimate_min_rub: None,
        price_estimate_min_aed: None,
        price_estimate_min_sar: None,
        price_estimate_min_hkd: None,
        price_estimate_min_sgd: None,
        price_estimate_min_chf: None,
        price_estimate_max_eur: None,
        price_estimate_max_usd: None,
        price_estimate_max_gbp: None,
        price_estimate_max_aud: None,
        price_estimate_max_cad: None,
        price_estimate_max_nzd: None,
        price_estimate_max_cny: None,
        price_estimate_max_brl: None,
        price_estimate_max_pln: None,
        price_estimate_max_try: None,
        price_estimate_max_jpy: None,
        price_estimate_max_czk: None,
        price_estimate_max_rub: None,
        price_estimate_max_aed: None,
        price_estimate_max_sar: None,
        price_estimate_max_hkd: None,
        price_estimate_max_sgd: None,
        price_estimate_max_chf: None,
        state: ProductStateDocument::Listed,
        lifecycle: ProductLifecycleDocument::Active,
        url: Url::parse("https://foo.com/bar").unwrap(),
        view_url: Url::parse("https://foo.com/bar?utm_source=aura_historia&utm_medium=referral")
            .unwrap(),
        images: Default::default(),
        embedding: None,
        created_by: common::actor::document::ActorDocument::System,
        updated_by: common::actor::document::ActorDocument::System,
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
        auction_start: None,
        auction_end: None,
    };
    let client = get_opensearch_client().await;
    let repository = ProductOpenSearchRepositoryImpl::new(client);
    let response = repository
        .create_product_documents(vec![expected.clone()])
        .await
        .unwrap();
    assert!(!response.errors);
    refresh_index("products").await;
    let actual = read_by_id("products", product_id).await;

    assert_eq!(expected, actual);
}

#[localstack_test(services = [OpenSearch()])]
async fn should_create_product_documents() {
    let product_id1 = ProductId::new();
    let expected1 = ProductDocument {
        product_id: product_id1,
        product_slug_id: Faker.fake(),
        shop_slug_id: Faker.fake(),
        seller_slug_id: Faker.fake(),
        event_id: Default::default(),
        shop_id: Default::default(),
        seller_id: Default::default(),
        shops_product_id: ShopsProductId::from("abcdefgh"),
        shop_name: "Foo".to_string(),
        seller_name: "Bar".to_string(),
        shop_type: ShopTypeDocument::CommercialDealer,
        structured_address_addressline: None,
        structured_address_addressline_extra: None,
        structured_address_locality: None,
        structured_address_region: None,
        structured_address_postal_code: None,
        structured_address_country: None,
        structured_address_continent: None,
        geo_address: None,
        title_native: TextDocument {
            text: "Foo".to_string(),
            language: LanguageDocument::Fr,
        },
        title_de: Some("Bar".to_string()),
        title_en: Some("Baz".to_string()),
        title_fr: Some("Bat".to_string()),
        title_es: Some("Bao".to_string()),
        title_it: Some("Bao".to_string()),
        price_eur: Some(99),
        price_usd: None,
        price_gbp: None,
        price_aud: None,
        price_cad: None,
        price_nzd: None,
        price_cny: None,
        price_brl: None,
        price_pln: None,
        price_try: None,
        price_jpy: None,
        price_czk: None,
        price_rub: None,
        price_aed: None,
        price_sar: None,
        price_hkd: None,
        price_sgd: None,
        price_chf: None,
        price_estimate_min_eur: None,
        price_estimate_min_usd: None,
        price_estimate_min_gbp: None,
        price_estimate_min_aud: None,
        price_estimate_min_cad: None,
        price_estimate_min_nzd: None,
        price_estimate_min_cny: None,
        price_estimate_min_brl: None,
        price_estimate_min_pln: None,
        price_estimate_min_try: None,
        price_estimate_min_jpy: None,
        price_estimate_min_czk: None,
        price_estimate_min_rub: None,
        price_estimate_min_aed: None,
        price_estimate_min_sar: None,
        price_estimate_min_hkd: None,
        price_estimate_min_sgd: None,
        price_estimate_min_chf: None,
        price_estimate_max_eur: None,
        price_estimate_max_usd: None,
        price_estimate_max_gbp: None,
        price_estimate_max_aud: None,
        price_estimate_max_cad: None,
        price_estimate_max_nzd: None,
        price_estimate_max_cny: None,
        price_estimate_max_brl: None,
        price_estimate_max_pln: None,
        price_estimate_max_try: None,
        price_estimate_max_jpy: None,
        price_estimate_max_czk: None,
        price_estimate_max_rub: None,
        price_estimate_max_aed: None,
        price_estimate_max_sar: None,
        price_estimate_max_hkd: None,
        price_estimate_max_sgd: None,
        price_estimate_max_chf: None,
        state: ProductStateDocument::Listed,
        lifecycle: ProductLifecycleDocument::Active,
        url: Url::parse("https://foo.com/bar").unwrap(),
        view_url: Url::parse("https://foo.com/bar?utm_source=aura_historia&utm_medium=referral")
            .unwrap(),
        images: Default::default(),
        embedding: None,
        created_by: common::actor::document::ActorDocument::System,
        updated_by: common::actor::document::ActorDocument::System,
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
        auction_start: None,
        auction_end: None,
    };
    let product_id2 = ProductId::new();
    let expected2 = ProductDocument {
        product_id: product_id2,
        product_slug_id: Faker.fake(),
        shop_slug_id: Faker.fake(),
        seller_slug_id: Faker.fake(),
        event_id: Default::default(),
        shop_id: Default::default(),
        seller_id: Default::default(),
        shops_product_id: ShopsProductId::from("abcdefgh"),
        shop_name: "Foo".to_string(),
        seller_name: "Bar".to_string(),
        shop_type: ShopTypeDocument::CommercialDealer,
        structured_address_addressline: None,
        structured_address_addressline_extra: None,
        structured_address_locality: None,
        structured_address_region: None,
        structured_address_postal_code: None,
        structured_address_country: None,
        structured_address_continent: None,
        geo_address: None,
        title_native: TextDocument {
            text: "Foo".to_string(),
            language: LanguageDocument::Fr,
        },
        title_de: Some("Bar".to_string()),
        title_en: Some("Baz".to_string()),
        title_fr: Some("Bat".to_string()),
        title_es: Some("Bao".to_string()),
        title_it: Some("Bao".to_string()),
        price_eur: Some(99),
        price_usd: None,
        price_gbp: None,
        price_aud: None,
        price_cad: None,
        price_nzd: None,
        price_cny: None,
        price_brl: None,
        price_pln: None,
        price_try: None,
        price_jpy: None,
        price_czk: None,
        price_rub: None,
        price_aed: None,
        price_sar: None,
        price_hkd: None,
        price_sgd: None,
        price_chf: None,
        price_estimate_min_eur: None,
        price_estimate_min_usd: None,
        price_estimate_min_gbp: None,
        price_estimate_min_aud: None,
        price_estimate_min_cad: None,
        price_estimate_min_nzd: None,
        price_estimate_min_cny: None,
        price_estimate_min_brl: None,
        price_estimate_min_pln: None,
        price_estimate_min_try: None,
        price_estimate_min_jpy: None,
        price_estimate_min_czk: None,
        price_estimate_min_rub: None,
        price_estimate_min_aed: None,
        price_estimate_min_sar: None,
        price_estimate_min_hkd: None,
        price_estimate_min_sgd: None,
        price_estimate_min_chf: None,
        price_estimate_max_eur: None,
        price_estimate_max_usd: None,
        price_estimate_max_gbp: None,
        price_estimate_max_aud: None,
        price_estimate_max_cad: None,
        price_estimate_max_nzd: None,
        price_estimate_max_cny: None,
        price_estimate_max_brl: None,
        price_estimate_max_pln: None,
        price_estimate_max_try: None,
        price_estimate_max_jpy: None,
        price_estimate_max_czk: None,
        price_estimate_max_rub: None,
        price_estimate_max_aed: None,
        price_estimate_max_sar: None,
        price_estimate_max_hkd: None,
        price_estimate_max_sgd: None,
        price_estimate_max_chf: None,
        state: ProductStateDocument::Listed,
        lifecycle: ProductLifecycleDocument::Active,
        url: Url::parse("https://foo.com/bar").unwrap(),
        view_url: Url::parse("https://foo.com/bar?utm_source=aura_historia&utm_medium=referral")
            .unwrap(),
        images: Default::default(),
        embedding: None,
        created_by: common::actor::document::ActorDocument::System,
        updated_by: common::actor::document::ActorDocument::System,
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
        auction_start: None,
        auction_end: None,
    };
    let client = get_opensearch_client().await;
    let repository = ProductOpenSearchRepositoryImpl::new(client);
    let response = repository
        .create_product_documents(vec![expected1.clone(), expected2.clone()])
        .await
        .unwrap();
    assert!(!response.errors);
    refresh_index("products").await;
    let actual1 = read_by_id("products", product_id1).await;
    let actual2 = read_by_id("products", product_id2).await;

    assert_eq!(expected1, actual1);
    assert_eq!(expected2, actual2);
}

#[localstack_test(services = [OpenSearch()])]
async fn should_update_product_document() {
    let product_id = ProductId::new();
    let initial = ProductDocument {
        product_id,
        product_slug_id: Faker.fake(),
        shop_slug_id: Faker.fake(),
        seller_slug_id: Faker.fake(),
        event_id: Default::default(),
        shop_id: Default::default(),
        seller_id: Default::default(),
        shops_product_id: ShopsProductId::from("abcdefgh"),
        shop_name: "Foo".to_string(),
        seller_name: "Bar".to_string(),
        shop_type: ShopTypeDocument::CommercialDealer,
        structured_address_addressline: None,
        structured_address_addressline_extra: None,
        structured_address_locality: None,
        structured_address_region: None,
        structured_address_postal_code: None,
        structured_address_country: None,
        structured_address_continent: None,
        geo_address: None,
        title_native: TextDocument {
            text: "Foo".to_string(),
            language: LanguageDocument::Fr,
        },
        title_de: Some("Bar".to_string()),
        title_en: Some("Baz".to_string()),
        title_fr: Some("Bat".to_string()),
        title_es: Some("Bao".to_string()),
        title_it: Some("Bao".to_string()),
        price_eur: Some(99),
        price_usd: None,
        price_gbp: None,
        price_aud: None,
        price_cad: None,
        price_nzd: None,
        price_cny: None,
        price_brl: None,
        price_pln: None,
        price_try: None,
        price_jpy: None,
        price_czk: None,
        price_rub: None,
        price_aed: None,
        price_sar: None,
        price_hkd: None,
        price_sgd: None,
        price_chf: None,
        price_estimate_min_eur: None,
        price_estimate_min_usd: None,
        price_estimate_min_gbp: None,
        price_estimate_min_aud: None,
        price_estimate_min_cad: None,
        price_estimate_min_nzd: None,
        price_estimate_min_cny: None,
        price_estimate_min_brl: None,
        price_estimate_min_pln: None,
        price_estimate_min_try: None,
        price_estimate_min_jpy: None,
        price_estimate_min_czk: None,
        price_estimate_min_rub: None,
        price_estimate_min_aed: None,
        price_estimate_min_sar: None,
        price_estimate_min_hkd: None,
        price_estimate_min_sgd: None,
        price_estimate_min_chf: None,
        price_estimate_max_eur: None,
        price_estimate_max_usd: None,
        price_estimate_max_gbp: None,
        price_estimate_max_aud: None,
        price_estimate_max_cad: None,
        price_estimate_max_nzd: None,
        price_estimate_max_cny: None,
        price_estimate_max_brl: None,
        price_estimate_max_pln: None,
        price_estimate_max_try: None,
        price_estimate_max_jpy: None,
        price_estimate_max_czk: None,
        price_estimate_max_rub: None,
        price_estimate_max_aed: None,
        price_estimate_max_sar: None,
        price_estimate_max_hkd: None,
        price_estimate_max_sgd: None,
        price_estimate_max_chf: None,
        state: ProductStateDocument::Listed,
        lifecycle: ProductLifecycleDocument::Active,
        url: Url::parse("https://foo.com/bar").unwrap(),
        view_url: Url::parse("https://foo.com/bar?utm_source=aura_historia&utm_medium=referral")
            .unwrap(),
        images: Default::default(),
        embedding: None,
        created_by: common::actor::document::ActorDocument::System,
        updated_by: common::actor::document::ActorDocument::System,
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
        auction_start: None,
        auction_end: None,
    };
    let client = get_opensearch_client().await;
    let repository = ProductOpenSearchRepositoryImpl::new(client);
    let write_response = repository
        .create_product_documents(vec![initial.clone()])
        .await
        .unwrap();
    assert!(!write_response.errors);
    refresh_index("products").await;

    let updated_event_id = EventId::new();
    let updated_update_ts = OffsetDateTime::now_utc();
    let update = ProductUpdateDocument {
        event_id: Some(updated_event_id),
        price_eur: None,
        price_usd: None,
        price_gbp: None,
        price_aud: None,
        price_cad: None,
        price_nzd: None,
        price_cny: None,
        price_brl: None,
        price_pln: None,
        price_try: None,
        price_jpy: None,
        price_czk: None,
        price_rub: None,
        price_aed: None,
        price_sar: None,
        price_hkd: None,
        price_sgd: None,
        price_chf: None,
        state: Some(ProductStateDocument::Sold),
        lifecycle: None,
        title_de: None,
        title_en: None,
        title_fr: None,
        title_es: None,
        title_it: None,
        images: None,
        price_estimate_min_eur: None,
        price_estimate_min_usd: None,
        price_estimate_min_gbp: None,
        price_estimate_min_aud: None,
        price_estimate_min_cad: None,
        price_estimate_min_nzd: None,
        price_estimate_min_cny: None,
        price_estimate_min_brl: None,
        price_estimate_min_pln: None,
        price_estimate_min_try: None,
        price_estimate_min_jpy: None,
        price_estimate_min_czk: None,
        price_estimate_min_rub: None,
        price_estimate_min_aed: None,
        price_estimate_min_sar: None,
        price_estimate_min_hkd: None,
        price_estimate_min_sgd: None,
        price_estimate_min_chf: None,
        price_estimate_max_eur: None,
        price_estimate_max_usd: None,
        price_estimate_max_gbp: None,
        price_estimate_max_aud: None,
        price_estimate_max_cad: None,
        price_estimate_max_nzd: None,
        price_estimate_max_cny: None,
        price_estimate_max_brl: None,
        price_estimate_max_pln: None,
        price_estimate_max_try: None,
        price_estimate_max_jpy: None,
        price_estimate_max_czk: None,
        price_estimate_max_rub: None,
        price_estimate_max_aed: None,
        price_estimate_max_sar: None,
        price_estimate_max_hkd: None,
        price_estimate_max_sgd: None,
        price_estimate_max_chf: None,
        url: None,
        auction_start: None,
        auction_end: None,
        embedding: None,
        updated: updated_update_ts,
    };
    let repository = ProductOpenSearchRepositoryImpl::new(client);
    let update_response = repository
        .update_product_documents(HashMap::from([(product_id, update)]))
        .await
        .unwrap();
    assert!(!update_response.errors);
    refresh_index("products").await;

    let mut expected = initial;
    expected.event_id = updated_event_id;
    expected.state = ProductStateDocument::Sold;
    expected.updated = updated_update_ts;

    let actual = read_by_id("products", product_id).await;

    assert_eq!(expected, actual);
}

#[localstack_test(services = [OpenSearch()])]
async fn should_search_product_documents() {
    let expected = ProductDocument {
        product_id: Default::default(),
        product_slug_id: Faker.fake(),
        shop_slug_id: Faker.fake(),
        seller_slug_id: Faker.fake(),
        event_id: Default::default(),
        shop_id: Default::default(),
        seller_id: Default::default(),
        shops_product_id: ShopsProductId::from("abcdefgh"),
        shop_name: "Foo".to_string(),
        seller_name: "Bar".to_string(),
        shop_type: ShopTypeDocument::CommercialDealer,
        structured_address_addressline: None,
        structured_address_addressline_extra: None,
        structured_address_locality: None,
        structured_address_region: None,
        structured_address_postal_code: None,
        structured_address_country: None,
        structured_address_continent: None,
        geo_address: None,
        title_native: TextDocument {
            text: "Foo".to_string(),
            language: LanguageDocument::Fr,
        },
        title_de: Some("Hallo Welt".to_string()),
        title_en: Some("Baz".to_string()),
        title_fr: Some("Bat".to_string()),
        title_es: None,
        title_it: None,
        price_eur: Some(99),
        price_usd: None,
        price_gbp: None,
        price_aud: None,
        price_cad: None,
        price_nzd: None,
        price_cny: None,
        price_brl: None,
        price_pln: None,
        price_try: None,
        price_jpy: None,
        price_czk: None,
        price_rub: None,
        price_aed: None,
        price_sar: None,
        price_hkd: None,
        price_sgd: None,
        price_chf: None,
        price_estimate_min_eur: None,
        price_estimate_min_usd: None,
        price_estimate_min_gbp: None,
        price_estimate_min_aud: None,
        price_estimate_min_cad: None,
        price_estimate_min_nzd: None,
        price_estimate_min_cny: None,
        price_estimate_min_brl: None,
        price_estimate_min_pln: None,
        price_estimate_min_try: None,
        price_estimate_min_jpy: None,
        price_estimate_min_czk: None,
        price_estimate_min_rub: None,
        price_estimate_min_aed: None,
        price_estimate_min_sar: None,
        price_estimate_min_hkd: None,
        price_estimate_min_sgd: None,
        price_estimate_min_chf: None,
        price_estimate_max_eur: None,
        price_estimate_max_usd: None,
        price_estimate_max_gbp: None,
        price_estimate_max_aud: None,
        price_estimate_max_cad: None,
        price_estimate_max_nzd: None,
        price_estimate_max_cny: None,
        price_estimate_max_brl: None,
        price_estimate_max_pln: None,
        price_estimate_max_try: None,
        price_estimate_max_jpy: None,
        price_estimate_max_czk: None,
        price_estimate_max_rub: None,
        price_estimate_max_aed: None,
        price_estimate_max_sar: None,
        price_estimate_max_hkd: None,
        price_estimate_max_sgd: None,
        price_estimate_max_chf: None,
        state: ProductStateDocument::Available,
        lifecycle: ProductLifecycleDocument::Active,
        url: Url::parse("https://foo.com/bar").unwrap(),
        view_url: Url::parse("https://foo.com/bar?utm_source=aura_historia&utm_medium=referral")
            .unwrap(),
        images: Default::default(),
        embedding: None,
        created_by: common::actor::document::ActorDocument::System,
        updated_by: common::actor::document::ActorDocument::System,
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
        auction_start: None,
        auction_end: None,
    };
    let client = get_opensearch_client().await;
    let repository = ProductOpenSearchRepositoryImpl::new(client);
    let response = repository
        .create_product_documents(vec![expected.clone()])
        .await
        .unwrap();
    assert!(!response.errors);
    refresh_index("products").await;

    tokio::time::sleep(Duration::from_millis(3000)).await;

    let search_filter = ProductSearch {
        language: Language::De,
        currency: Currency::Eur,
        product_query: vec!["Hallo Welt".try_into().unwrap()],
        enhanced_search_description: None,
        exclude_product_id_query: Default::default(),
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
        lifecycle_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
        shop_slug_id_query: Default::default(),
        exclude_shop_slug_id_query: Default::default(),
        seller_slug_id_query: Default::default(),
        exclude_seller_slug_id_query: Default::default(),
    };
    let response = repository
        .search_product_documents(
            &search_filter,
            &Sort {
                sort: SortProductField::Score,
                order: SortOrder::Desc,
            },
            &None,
        )
        .await
        .unwrap();

    assert_eq!(
        vec![expected],
        response
            .hits
            .hits
            .into_iter()
            .map(|hit| hit.source)
            .collect::<Vec<_>>()
    )
}

#[localstack_test(services = [OpenSearch()])]
async fn should_search_product_documents_when_any_product_query_matches() {
    let mut madonna = Faker.fake::<ProductDocument>();
    madonna.title_en = Some("Madonna oil painting renaissance artwork".into());
    madonna.title_native = TextDocument {
        text: "Madonna oil painting renaissance artwork".into(),
        language: LanguageDocument::En,
    };

    let mut virgin_mary = Faker.fake::<ProductDocument>();
    virgin_mary.title_en = Some("Virgin Mary oil painting antique icon".into());
    virgin_mary.title_native = TextDocument {
        text: "Virgin Mary oil painting antique icon".into(),
        language: LanguageDocument::En,
    };

    let mut unrelated = Faker.fake::<ProductDocument>();
    unrelated.title_en = Some("Bronze garden sculpture".into());
    unrelated.title_native = TextDocument {
        text: "Bronze garden sculpture".into(),
        language: LanguageDocument::En,
    };

    let client = get_opensearch_client().await;
    let repository = ProductOpenSearchRepositoryImpl::new(client);
    let response = repository
        .create_product_documents(vec![madonna.clone(), virgin_mary.clone(), unrelated])
        .await
        .unwrap();
    assert!(!response.errors);
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_millis(3000)).await;

    let search_filter = ProductSearch::new(Language::En, Currency::Eur)
        .with_product_query("Madonna oil painting".try_into().unwrap())
        .with_product_query("Virgin Mary oil painting".try_into().unwrap());

    let response = repository
        .search_product_documents(
            &search_filter,
            &Sort {
                sort: SortProductField::Score,
                order: SortOrder::Desc,
            },
            &None,
        )
        .await
        .unwrap();

    let actual_ids = response
        .hits
        .hits
        .into_iter()
        .map(|hit| hit.source.product_id)
        .collect::<HashSet<_>>();
    assert_eq!(
        HashSet::from_iter([madonna.product_id, virgin_mary.product_id]),
        actual_ids
    );
}

#[localstack_test(services = [OpenSearch()])]
async fn should_search_product_documents_with_percolator_query() {
    let expected_product_id = ProductId::new();
    let expected = ProductDocument {
        product_id: expected_product_id,
        product_slug_id: Faker.fake(),
        shop_slug_id: Faker.fake(),
        seller_slug_id: Faker.fake(),
        event_id: Default::default(),
        shop_id: Default::default(),
        seller_id: Default::default(),
        shops_product_id: ShopsProductId::from("percolator-match"),
        shop_name: "Foo".to_string(),
        seller_name: "Bar".to_string(),
        shop_type: ShopTypeDocument::CommercialDealer,
        structured_address_addressline: None,
        structured_address_addressline_extra: None,
        structured_address_locality: None,
        structured_address_region: None,
        structured_address_postal_code: None,
        structured_address_country: None,
        structured_address_continent: None,
        geo_address: None,
        title_native: TextDocument {
            text: "golden cufflinks antique vintage".to_string(),
            language: LanguageDocument::En,
        },
        title_de: Some("golden cufflinks antique vintage".to_string()),
        title_en: Some("golden cufflinks antique vintage".to_string()),
        title_fr: None,
        title_es: None,
        title_it: None,
        price_eur: Some(99),
        price_usd: None,
        price_gbp: None,
        price_aud: None,
        price_cad: None,
        price_nzd: None,
        price_cny: None,
        price_brl: None,
        price_pln: None,
        price_try: None,
        price_jpy: None,
        price_czk: None,
        price_rub: None,
        price_aed: None,
        price_sar: None,
        price_hkd: None,
        price_sgd: None,
        price_chf: None,
        price_estimate_min_eur: None,
        price_estimate_min_usd: None,
        price_estimate_min_gbp: None,
        price_estimate_min_aud: None,
        price_estimate_min_cad: None,
        price_estimate_min_nzd: None,
        price_estimate_min_cny: None,
        price_estimate_min_brl: None,
        price_estimate_min_pln: None,
        price_estimate_min_try: None,
        price_estimate_min_jpy: None,
        price_estimate_min_czk: None,
        price_estimate_min_rub: None,
        price_estimate_min_aed: None,
        price_estimate_min_sar: None,
        price_estimate_min_hkd: None,
        price_estimate_min_sgd: None,
        price_estimate_min_chf: None,
        price_estimate_max_eur: None,
        price_estimate_max_usd: None,
        price_estimate_max_gbp: None,
        price_estimate_max_aud: None,
        price_estimate_max_cad: None,
        price_estimate_max_nzd: None,
        price_estimate_max_cny: None,
        price_estimate_max_brl: None,
        price_estimate_max_pln: None,
        price_estimate_max_try: None,
        price_estimate_max_jpy: None,
        price_estimate_max_czk: None,
        price_estimate_max_rub: None,
        price_estimate_max_aed: None,
        price_estimate_max_sar: None,
        price_estimate_max_hkd: None,
        price_estimate_max_sgd: None,
        price_estimate_max_chf: None,
        state: ProductStateDocument::Available,
        lifecycle: ProductLifecycleDocument::Active,
        url: Url::parse("https://foo.com/percolator-match").unwrap(),
        view_url: Url::parse(
            "https://foo.com/percolator-match?utm_source=aura_historia&utm_medium=referral",
        )
        .unwrap(),
        images: Default::default(),
        embedding: None,
        created_by: common::actor::document::ActorDocument::System,
        updated_by: common::actor::document::ActorDocument::System,
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
        auction_start: None,
        auction_end: None,
    };
    let mut non_matching = expected.clone();
    non_matching.product_id = ProductId::new();
    non_matching.shops_product_id = ShopsProductId::from("percolator-miss");
    non_matching.title_de = Some("silver tea set".to_string());
    non_matching.title_en = Some("silver tea set".to_string());
    non_matching.title_native.text = "silver tea set".to_string();

    let client = get_opensearch_client().await;
    let repository = ProductOpenSearchRepositoryImpl::new(client);
    let response = repository
        .create_product_documents(vec![expected.clone(), non_matching])
        .await
        .unwrap();
    assert!(!response.errors);
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_millis(3000)).await;

    let search = ProductSearch::new(Language::De, Currency::Eur)
        .with_product_query("golden cufflinks antique vintage rare".try_into().unwrap());
    let response = repository
        .search_product_documents_with_percolator_query(&search, 10)
        .await
        .unwrap();

    assert_eq!(
        vec![expected_product_id],
        response
            .hits
            .hits
            .into_iter()
            .map(|hit| hit.source.product_id)
            .collect::<Vec<_>>()
    );
}

#[localstack_test(services = [OpenSearch()])]
async fn should_search_product_documents_when_all_arguments_are_given() {
    let products = fake::vec![ProductDocument; 1000];
    let client = get_opensearch_client().await;
    let repository = ProductOpenSearchRepositoryImpl::new(client);
    let response = repository
        .create_product_documents(products.clone())
        .await
        .unwrap();
    assert!(!response.errors);
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_millis(3000)).await;

    let search_filter = ProductSearch {
        language: Language::De,
        currency: Currency::Eur,
        product_query: vec!["Lorem".try_into().unwrap()],
        enhanced_search_description: None,
        exclude_product_id_query: Default::default(),
        shop_type_query: Default::default(),
        shop_name_query: HashSet::from_iter(["Wyoming LLC".into()]).into(),
        exclude_shop_name_query: HashSet::from_iter(["Berlin GmbH".into()]).into(),
        seller_name_query: Default::default(),
        exclude_seller_name_query: Default::default(),
        country_query: Default::default(),
        continent_query: Default::default(),
        geo_address_distance_query: None,
        price_query: Some(RangeQuery {
            min: Some(100u64.into()),
            max: Some(999999u64.into()),
        }),
        state_query: AnyOfQuery::from(HashSet::from_iter([
            ProductState::Available,
            ProductState::Listed,
        ])),
        lifecycle_query: Default::default(),
        created_query: Some(RangeQuery {
            min: Some(datetime!(1000-01-01 0:00 UTC)),
            max: Some(datetime!(3000-01-01 0:00 UTC)),
        }),
        updated_query: Some(RangeQuery {
            min: Some(datetime!(1000-01-01 0:00 UTC)),
            max: Some(datetime!(3000-01-01 0:00 UTC)),
        }),
        auction_start_query: None,
        auction_end_query: None,
        shop_slug_id_query: Default::default(),
        exclude_shop_slug_id_query: Default::default(),
        seller_slug_id_query: Default::default(),
        exclude_seller_slug_id_query: Default::default(),
    };
    let sort = Sort {
        sort: SortProductField::Price,
        order: SortOrder::Asc,
    };
    let page = Cursor {
        size: 20,
        search_after: None,
    };
    let response = repository
        .search_product_documents(&search_filter, &sort, &Some(page))
        .await;

    assert!(response.is_ok());
}

#[rstest::rstest]
#[trace]
#[test_attr(apply(test))]
#[case(&[ProductState::Available])]
#[case(&[ProductState::Reserved, ProductState::Listed, ProductState::Removed])]
#[localstack_test(services = [OpenSearch()])]
async fn should_search_product_documents_when_states_are_given(#[case] states: &[ProductState]) {
    let products = fake::vec![ProductDocument; 3000]
        .into_iter()
        .map(|mut item| {
            item.title_de = Some("The same title".into());
            item
        })
        .collect::<Vec<_>>();
    let client = get_opensearch_client().await;
    let repository = ProductOpenSearchRepositoryImpl::new(client);
    let response = repository
        .create_product_documents(products.clone())
        .await
        .unwrap();
    assert!(!response.errors);
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_millis(3000)).await;

    let search_filter = ProductSearch {
        language: Language::De,
        currency: Currency::Eur,
        product_query: vec!["The same title".try_into().unwrap()],
        enhanced_search_description: None,
        exclude_product_id_query: Default::default(),
        shop_name_query: Default::default(),
        exclude_shop_name_query: Default::default(),
        seller_name_query: Default::default(),
        exclude_seller_name_query: Default::default(),
        shop_type_query: Default::default(),
        country_query: Default::default(),
        continent_query: Default::default(),
        geo_address_distance_query: None,
        price_query: None,
        state_query: AnyOfQuery::from(HashSet::from_iter(states.iter().copied())),
        lifecycle_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
        shop_slug_id_query: Default::default(),
        exclude_shop_slug_id_query: Default::default(),
        seller_slug_id_query: Default::default(),
        exclude_seller_slug_id_query: Default::default(),
    };
    let response = repository
        .search_product_documents(
            &search_filter,
            &Sort {
                sort: SortProductField::Score,
                order: SortOrder::Desc,
            },
            &None,
        )
        .await
        .unwrap();

    assert!(response.hits.total.value > 0);
    assert!(
        response
            .hits
            .hits
            .iter()
            .all(|hit| { states.contains(&ProductState::from(hit.source.state)) })
    );
}

#[localstack_test(services = [OpenSearch()])]
async fn should_search_product_documents_when_no_states_are_given() {
    let items = fake::vec![ProductDocument; 100]
        .into_iter()
        .map(|mut item| {
            item.title_de = Some("The same title".into());
            item
        })
        .collect::<Vec<_>>();
    let client = get_opensearch_client().await;
    let repository = ProductOpenSearchRepositoryImpl::new(client);
    let response = repository
        .create_product_documents(items.clone())
        .await
        .unwrap();
    assert!(!response.errors);
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_millis(3000)).await;

    let search_filter = ProductSearch {
        language: Language::De,
        currency: Currency::Eur,
        product_query: vec!["The same title".try_into().unwrap()],
        enhanced_search_description: None,
        exclude_product_id_query: Default::default(),
        shop_name_query: Default::default(),
        exclude_shop_name_query: Default::default(),
        seller_name_query: Default::default(),
        exclude_seller_name_query: Default::default(),
        shop_type_query: Default::default(),
        country_query: Default::default(),
        continent_query: Default::default(),
        geo_address_distance_query: None,
        price_query: None,
        state_query: AnyOfQuery::from(HashSet::new()),
        lifecycle_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
        shop_slug_id_query: Default::default(),
        exclude_shop_slug_id_query: Default::default(),
        seller_slug_id_query: Default::default(),
        exclude_seller_slug_id_query: Default::default(),
    };
    let response = repository
        .search_product_documents(
            &search_filter,
            &Sort {
                sort: SortProductField::Score,
                order: SortOrder::Desc,
            },
            &None,
        )
        .await
        .unwrap();

    assert_eq!(100u64, response.hits.total.value);
}

#[rstest::rstest]
#[trace]
#[test_attr(apply(test))]
#[case(RangeQuery { min: Some(300u64.into()), max: Some(5000u64.into()) })]
#[case(RangeQuery { min: Some(500u64.into()), max: None })]
#[case(RangeQuery { min: None, max: Some(8888u64.into()) })]
#[case(RangeQuery { min: None, max: None })]
#[localstack_test(services = [OpenSearch()])]
async fn should_search_product_documents_when_price_range_is_given(
    #[case] price_query: RangeQuery<MonetaryAmount>,
) {
    let cheap_products = fake::vec![ProductDocument; 50]
        .into_iter()
        .enumerate()
        .map(|(idx, mut product)| {
            product.title_de = Some("The same title".into());
            product.price_eur = Some(150 + (idx as u64 % 851));
            product
        })
        .collect::<Vec<_>>();
    let expensive_products = fake::vec![ProductDocument; 50]
        .into_iter()
        .enumerate()
        .map(|(idx, mut product)| {
            product.title_de = Some("The same title".into());
            product.price_eur = Some(1500 + (idx as u64 % 18_501));
            product
        })
        .collect::<Vec<_>>();
    let products = [cheap_products, expensive_products].concat();
    let client = get_opensearch_client().await;
    let repository = ProductOpenSearchRepositoryImpl::new(client);
    let response = repository
        .create_product_documents(products.clone())
        .await
        .unwrap();
    assert!(!response.errors);
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_millis(3000)).await;

    let search_filter = ProductSearch {
        language: Language::De,
        currency: Currency::Eur,
        product_query: vec!["The same title".try_into().unwrap()],
        enhanced_search_description: None,
        exclude_product_id_query: Default::default(),
        shop_name_query: Default::default(),
        exclude_shop_name_query: Default::default(),
        seller_name_query: Default::default(),
        exclude_seller_name_query: Default::default(),
        shop_type_query: Default::default(),
        country_query: Default::default(),
        continent_query: Default::default(),
        geo_address_distance_query: None,
        price_query: Some(price_query),
        state_query: Default::default(),
        lifecycle_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
        shop_slug_id_query: Default::default(),
        exclude_shop_slug_id_query: Default::default(),
        seller_slug_id_query: Default::default(),
        exclude_seller_slug_id_query: Default::default(),
    };
    let response = repository
        .search_product_documents(
            &search_filter,
            &Sort {
                sort: SortProductField::Score,
                order: SortOrder::Desc,
            },
            &Some(Cursor {
                size: 100,
                search_after: None,
            }),
        )
        .await
        .unwrap();
    let actual_items = response
        .hits
        .hits
        .into_iter()
        .map(|hit| hit.source)
        .collect::<Vec<_>>();
    let expected_products = products
        .into_iter()
        .filter(|product| {
            let mut filter = true;
            if let Some(min) = price_query.min {
                filter = filter && product.price_eur.unwrap() >= *min;
            }
            if let Some(max) = price_query.max {
                filter = filter && product.price_eur.unwrap() <= *max;
            }
            filter
        })
        .collect::<Vec<_>>();

    assert_eq!(expected_products.len(), actual_items.len());
    assert!(
        expected_products
            .iter()
            .all(|product| actual_items.contains(product))
    );
}

#[localstack_test(services = [OpenSearch()])]
async fn should_search_product_documents_respecting_paging_when_sorted_by_price() {
    let products = fake::vec![ProductDocument; 1000]
        .into_iter()
        .enumerate()
        .map(|(idx, mut product)| {
            product.title_en = Some("The same title".into());
            product.price_usd = Some(1500 + (idx as u64 % 18_501));
            product
        })
        .collect::<Vec<_>>();
    let client = get_opensearch_client().await;
    let repository = ProductOpenSearchRepositoryImpl::new(client);
    let response = repository
        .create_product_documents(products.clone())
        .await
        .unwrap();
    assert!(!response.errors);
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_millis(3000)).await;

    let search_filter = ProductSearch {
        language: Language::En,
        currency: Currency::Usd,
        product_query: vec!["The same title".try_into().unwrap()],
        enhanced_search_description: None,
        exclude_product_id_query: Default::default(),
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
        lifecycle_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
        shop_slug_id_query: Default::default(),
        exclude_shop_slug_id_query: Default::default(),
        seller_slug_id_query: Default::default(),
        exclude_seller_slug_id_query: Default::default(),
    };
    let response = repository
        .search_product_documents(
            &search_filter,
            &Sort {
                sort: SortProductField::Price,
                order: SortOrder::Asc,
            },
            &Some(Cursor {
                size: 17,
                search_after: None,
            }),
        )
        .await
        .unwrap();
    let mut actual_items = response
        .hits
        .hits
        .into_iter()
        .map(|hit| hit.source)
        .collect::<Vec<_>>();
    let sorter = |l: &ProductDocument, r: &ProductDocument| match l
        .price_usd
        .unwrap()
        .cmp(&r.price_usd.unwrap())
    {
        std::cmp::Ordering::Equal => l.product_id.to_string().cmp(&r.product_id.to_string()),
        ord => ord,
    };
    actual_items.sort_by(sorter);

    let mut expected_products = products;
    expected_products.sort_by(sorter);
    let expected_products = expected_products.into_iter().take(17).collect::<Vec<_>>();

    assert_eq!(expected_products, actual_items);
}

#[localstack_test(services = [OpenSearch()])]
async fn should_search_product_documents_respecting_search_after_when_sorted_by_price() {
    let mut expected_products = fake::vec![ProductDocument; 200]
        .into_iter()
        .enumerate()
        .map(|(idx, mut product)| {
            product.title_en = Some("The same title".into());
            product.price_usd = Some(1500 + (idx as u64 % 18_501));
            product
        })
        .collect::<Vec<_>>();
    let client = get_opensearch_client().await;
    let repository = ProductOpenSearchRepositoryImpl::new(client);
    let response = repository
        .create_product_documents(expected_products.clone())
        .await
        .unwrap();
    assert!(!response.errors);
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_millis(3000)).await;

    let sorter = |l: &ProductDocument, r: &ProductDocument| match l
        .price_usd
        .unwrap()
        .cmp(&r.price_usd.unwrap())
    {
        std::cmp::Ordering::Equal => l.product_id.to_string().cmp(&r.product_id.to_string()),
        ord => ord,
    };
    expected_products.sort_by(sorter);
    let search_filter = ProductSearch {
        language: Language::En,
        currency: Currency::Usd,
        product_query: vec!["The same title".try_into().unwrap()],
        enhanced_search_description: None,
        exclude_product_id_query: Default::default(),
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
        lifecycle_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
        shop_slug_id_query: Default::default(),
        exclude_shop_slug_id_query: Default::default(),
        seller_slug_id_query: Default::default(),
        exclude_seller_slug_id_query: Default::default(),
    };
    let response = repository
        .search_product_documents(
            &search_filter,
            &Sort {
                sort: SortProductField::Price,
                order: SortOrder::Asc,
            },
            &Some(Cursor {
                size: 15,
                search_after: Some(json!([
                    expected_products[1].price_usd.unwrap(),
                    expected_products[1].product_id.to_string()
                ])),
            }),
        )
        .await
        .unwrap();
    let actual_items = response
        .hits
        .hits
        .into_iter()
        .map(|hit| hit.source)
        .collect::<Vec<_>>();
    let expected_products = expected_products
        .into_iter()
        .skip(2)
        .take(15)
        .collect::<Vec<_>>();

    assert_eq!(expected_products, actual_items);
}

#[localstack_test(services = [OpenSearch()])]
async fn should_search_product_documents_when_country_query_is_given() {
    let repository = ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let expected_country = isocountry::CountryCode::DEU;
    let other_country = isocountry::CountryCode::FRA;
    let mut expected = Faker.fake::<ProductDocument>();
    expected.structured_address_country = Some(expected_country);
    expected.structured_address_continent =
        Some(ContinentDocument::from(Continent::from(expected_country)));
    let mut other = Faker.fake::<ProductDocument>();
    other.structured_address_country = Some(other_country);
    other.structured_address_continent =
        Some(ContinentDocument::from(Continent::from(other_country)));
    let create_res = repository
        .create_product_documents(vec![expected.clone(), other])
        .await
        .unwrap();
    assert!(!create_res.errors);
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let search_filter = ProductSearch::new(Language::En, Currency::Eur)
        .with_country_query(HashSet::from_iter([expected_country]).into());

    let response = repository
        .search_product_documents(
            &search_filter,
            &Sort {
                sort: SortProductField::Score,
                order: SortOrder::Desc,
            },
            &None,
        )
        .await
        .unwrap();

    let hits = response
        .hits
        .hits
        .into_iter()
        .map(|hit| hit.source.product_id)
        .collect::<HashSet<_>>();
    assert_eq!(HashSet::from_iter([expected.product_id]), hits);
}

#[localstack_test(services = [OpenSearch()])]
async fn should_search_product_documents_when_continent_query_is_given() {
    let repository = ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let mut expected = Faker.fake::<ProductDocument>();
    expected.structured_address_country = Some(isocountry::CountryCode::DEU);
    expected.structured_address_continent = Some(ContinentDocument::Europe);
    let mut other = Faker.fake::<ProductDocument>();
    other.structured_address_country = Some(isocountry::CountryCode::JPN);
    other.structured_address_continent = Some(ContinentDocument::Asia);
    let create_res = repository
        .create_product_documents(vec![expected.clone(), other])
        .await
        .unwrap();
    assert!(!create_res.errors);
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let search_filter = ProductSearch::new(Language::En, Currency::Eur)
        .with_continent_query(HashSet::from_iter([Continent::Europe]).into());

    let response = repository
        .search_product_documents(
            &search_filter,
            &Sort {
                sort: SortProductField::Score,
                order: SortOrder::Desc,
            },
            &None,
        )
        .await
        .unwrap();

    let hits = response
        .hits
        .hits
        .into_iter()
        .map(|hit| hit.source.product_id)
        .collect::<HashSet<_>>();
    assert_eq!(HashSet::from_iter([expected.product_id]), hits);
}

#[localstack_test(services = [OpenSearch()])]
async fn should_search_product_documents_when_geo_address_distance_query_is_given() {
    let repository = ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let mut expected = Faker.fake::<ProductDocument>();
    expected.geo_address = Some("52.5200,13.4050".to_string());
    let mut other = Faker.fake::<ProductDocument>();
    other.geo_address = Some("40.7128,-74.0060".to_string());
    let create_res = repository
        .create_product_documents(vec![expected.clone(), other])
        .await
        .unwrap();
    assert!(!create_res.errors);
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let search_filter = ProductSearch::new(Language::En, Currency::Eur)
        .with_geo_address_distance_query(GeoDistanceQuery {
            lat: 52.5200,
            lon: 13.4050,
            distance: Distance {
                amount: 50.0,
                unit: DistanceUnit::Kilometers,
            },
        });

    let response = repository
        .search_product_documents(
            &search_filter,
            &Sort {
                sort: SortProductField::Score,
                order: SortOrder::Desc,
            },
            &None,
        )
        .await
        .unwrap();

    let hits = response
        .hits
        .hits
        .into_iter()
        .map(|hit| hit.source.product_id)
        .collect::<HashSet<_>>();
    assert_eq!(HashSet::from_iter([expected.product_id]), hits);
}

#[localstack_test(services = [OpenSearch()])]
async fn should_delete_product_document() {
    let product_id = ProductId::new();
    let mut document = Faker.fake::<ProductDocument>();
    document.product_id = product_id;

    let client = get_opensearch_client().await;
    let repository = ProductOpenSearchRepositoryImpl::new(client);
    let response = repository
        .create_product_documents(vec![document])
        .await
        .unwrap();
    assert!(!response.errors);
    refresh_index("products").await;

    let actual = repository.get_product_document_by_id(&product_id).await;
    assert!(actual.is_ok());

    let response = repository
        .delete_product_documents(vec![product_id])
        .await
        .unwrap();
    assert!(!response.errors);
    refresh_index("products").await;

    let actual = repository.get_product_document_by_id(&product_id).await;
    assert!(actual.is_err());
}

#[localstack_test(services = [OpenSearch()])]
async fn should_get_product_document() {
    let product_id = ProductId::new();
    let expected = ProductDocument {
        product_id,
        product_slug_id: Faker.fake(),
        shop_slug_id: Faker.fake(),
        seller_slug_id: Faker.fake(),
        event_id: Default::default(),
        shop_id: Default::default(),
        seller_id: Default::default(),
        shops_product_id: ShopsProductId::from("abcdefgh"),
        shop_name: "Foo".to_string(),
        seller_name: "Bar".to_string(),
        shop_type: ShopTypeDocument::CommercialDealer,
        structured_address_addressline: None,
        structured_address_addressline_extra: None,
        structured_address_locality: None,
        structured_address_region: None,
        structured_address_postal_code: None,
        structured_address_country: None,
        structured_address_continent: None,
        geo_address: None,
        title_native: TextDocument {
            text: "Foo".to_string(),
            language: LanguageDocument::Fr,
        },
        title_de: Some("Bar".to_string()),
        title_en: Some("Baz".to_string()),
        title_fr: Some("Bat".to_string()),
        title_es: Some("Bao".to_string()),
        title_it: Some("Bao".to_string()),
        price_eur: Some(99),
        price_usd: None,
        price_gbp: None,
        price_aud: None,
        price_cad: None,
        price_nzd: None,
        price_cny: None,
        price_brl: None,
        price_pln: None,
        price_try: None,
        price_jpy: None,
        price_czk: None,
        price_rub: None,
        price_aed: None,
        price_sar: None,
        price_hkd: None,
        price_sgd: None,
        price_chf: None,
        price_estimate_min_eur: None,
        price_estimate_min_usd: None,
        price_estimate_min_gbp: None,
        price_estimate_min_aud: None,
        price_estimate_min_cad: None,
        price_estimate_min_nzd: None,
        price_estimate_min_cny: None,
        price_estimate_min_brl: None,
        price_estimate_min_pln: None,
        price_estimate_min_try: None,
        price_estimate_min_jpy: None,
        price_estimate_min_czk: None,
        price_estimate_min_rub: None,
        price_estimate_min_aed: None,
        price_estimate_min_sar: None,
        price_estimate_min_hkd: None,
        price_estimate_min_sgd: None,
        price_estimate_min_chf: None,
        price_estimate_max_eur: None,
        price_estimate_max_usd: None,
        price_estimate_max_gbp: None,
        price_estimate_max_aud: None,
        price_estimate_max_cad: None,
        price_estimate_max_nzd: None,
        price_estimate_max_cny: None,
        price_estimate_max_brl: None,
        price_estimate_max_pln: None,
        price_estimate_max_try: None,
        price_estimate_max_jpy: None,
        price_estimate_max_czk: None,
        price_estimate_max_rub: None,
        price_estimate_max_aed: None,
        price_estimate_max_sar: None,
        price_estimate_max_hkd: None,
        price_estimate_max_sgd: None,
        price_estimate_max_chf: None,
        state: ProductStateDocument::Listed,
        lifecycle: ProductLifecycleDocument::Active,
        url: Url::parse("https://foo.com/bar").unwrap(),
        view_url: Url::parse("https://foo.com/bar?utm_source=aura_historia&utm_medium=referral")
            .unwrap(),
        images: Default::default(),
        embedding: None,
        created_by: common::actor::document::ActorDocument::System,
        updated_by: common::actor::document::ActorDocument::System,
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
        auction_start: None,
        auction_end: None,
    };
    let client = get_opensearch_client().await;
    let repository = ProductOpenSearchRepositoryImpl::new(client);
    let response = repository
        .create_product_documents(vec![expected.clone()])
        .await
        .unwrap();
    assert!(!response.errors);
    refresh_index("products").await;
    let actual = repository
        .get_product_document_by_id(&product_id)
        .await
        .unwrap();

    assert_eq!(expected, actual);
}

const EXAMPLE_EMBEDDING: [f32; 768] = [
    -0.036270842,
    0.02361682,
    -0.0029220004,
    -0.016072785,
    0.02316376,
    0.008332699,
    -0.02891746,
    0.015677461,
    -0.01463142,
    -0.10077077,
    0.029492525,
    0.02435133,
    0.04219972,
    -0.014070857,
    0.0025885715,
    0.015626293,
    -0.02128292,
    -0.016839612,
    -0.033849,
    -0.005133642,
    -0.015667764,
    -0.022695456,
    -0.0026581238,
    0.004976106,
    -0.06931419,
    -0.0021109623,
    -0.021948576,
    0.014820006,
    -0.013131463,
    0.15988831,
    0.0064275274,
    -0.0076653278,
    -0.038857676,
    0.015254312,
    -0.006424452,
    0.023108613,
    0.07357906,
    0.02665727,
    0.00575866,
    -0.0020714481,
    -0.025986703,
    0.027917072,
    -0.05469967,
    -0.021670582,
    -0.013154979,
    0.03821949,
    -0.012864586,
    0.0041407137,
    0.028950866,
    -0.0063043595,
    -0.008261838,
    0.020844104,
    0.00023263764,
    0.019758994,
    -0.019021928,
    0.03960655,
    -0.033878434,
    0.013370168,
    0.014440682,
    0.0015611759,
    -0.0060427976,
    -0.045798533,
    0.0028658975,
    0.0048241396,
    -0.026040733,
    0.02626537,
    0.019150974,
    -0.029956313,
    0.034417532,
    0.004912864,
    -0.010934778,
    0.0015013685,
    -0.022339396,
    0.020023942,
    0.005828301,
    -0.09966123,
    -0.06327092,
    0.024522135,
    -0.04826947,
    -0.020258049,
    -0.020873314,
    0.00036792032,
    -0.04074486,
    -0.019007195,
    0.0076569123,
    -0.0016037169,
    -0.014027866,
    0.0073729367,
    0.032381486,
    0.0052755023,
    0.0070434883,
    -0.012318134,
    -0.021978505,
    -0.0035620113,
    -0.035701845,
    -0.0062370175,
    -0.02363757,
    -0.03096813,
    0.00068176736,
    -0.012917327,
    0.0018843627,
    0.00052359427,
    -0.0044537387,
    -0.024308093,
    0.03562218,
    -0.011851221,
    0.028853856,
    -0.0012316285,
    0.02336089,
    0.0124050295,
    -0.03968709,
    -0.22498026,
    0.019794008,
    0.017281797,
    -0.003570257,
    0.25313136,
    -0.01618679,
    -0.014901762,
    -0.005371125,
    0.028242508,
    0.01495046,
    -0.002102732,
    -0.009359438,
    0.00038446576,
    0.038829945,
    0.03757913,
    0.061200988,
    0.039118737,
    -0.004323444,
    -0.027902763,
    0.021966223,
    0.036142662,
    0.0083741965,
    -0.014607301,
    0.013467545,
    0.015450331,
    -0.01713689,
    0.015013144,
    0.031145055,
    -0.03161453,
    -0.022872536,
    0.022965059,
    0.01465307,
    -0.040879726,
    -0.0070571224,
    0.0005238096,
    0.006517733,
    -0.05945249,
    -0.00067222246,
    -0.017303798,
    -0.02743768,
    0.051286776,
    0.010820717,
    -0.008597286,
    0.008311842,
    0.031794846,
    0.03725525,
    -0.007881769,
    0.034670442,
    -0.008120512,
    -0.0017984086,
    -0.008127016,
    -0.015096135,
    0.031332,
    0.013066103,
    -0.015996825,
    0.036567163,
    0.0023044932,
    -0.015515072,
    0.035640754,
    -0.025439778,
    0.019737234,
    -0.00048255606,
    0.027483864,
    -0.0062847566,
    0.035673726,
    0.02689843,
    -0.024476523,
    0.036291257,
    0.07619501,
    0.044448603,
    -0.02978229,
    0.0003071704,
    -0.066682085,
    -0.016464977,
    0.027141921,
    0.0015256412,
    -0.040789746,
    0.00044568328,
    -0.0073254695,
    0.020374568,
    0.009659304,
    0.021580324,
    0.00032814275,
    -0.033917915,
    -0.029009834,
    0.044985965,
    0.008687944,
    -0.040525082,
    0.01396069,
    -0.05742075,
    0.019486612,
    0.01334306,
    0.031041175,
    0.027065355,
    -0.012784972,
    0.0044180467,
    0.034939438,
    -0.013596606,
    0.020558216,
    0.011244942,
    -0.02307572,
    -0.019498749,
    -0.013778815,
    -0.0036768846,
    0.018824909,
    0.037605233,
    0.039746355,
    -0.0054461425,
    -0.01871201,
    -0.008835689,
    0.020823514,
    0.032042388,
    0.01331485,
    0.02537492,
    -0.0078030215,
    0.039240696,
    -0.021729227,
    -0.005688172,
    0.021090481,
    0.039646916,
    -0.034255978,
    -0.008763929,
    0.022813259,
    0.04913263,
    -0.008697633,
    -0.047809932,
    -0.0049542347,
    -0.000523725,
    0.00044063161,
    0.0046917875,
    0.0051231035,
    -0.04871753,
    0.010481537,
    0.001975782,
    -0.029364169,
    0.0010357029,
    0.030492049,
    -0.039915103,
    -0.008770563,
    0.027659342,
    -0.029857345,
    0.0154229775,
    0.0052343365,
    0.005864664,
    0.03145457,
    -0.041445766,
    0.014016001,
    -0.03302228,
    -0.013902694,
    -0.01625225,
    0.00993095,
    -0.01161224,
    -0.03400416,
    0.009857927,
    0.0104377465,
    0.060225435,
    -0.0093719335,
    0.0018534202,
    0.018284181,
    -0.01361248,
    0.017421937,
    -0.0038058027,
    0.042009708,
    -0.015804857,
    0.021955919,
    -0.0012992409,
    0.038149707,
    0.018156793,
    -0.062405195,
    0.013066391,
    -0.056466848,
    -0.017757474,
    -0.0028650656,
    0.0058570434,
    -0.010280581,
    0.021009846,
    0.016863098,
    -0.015731147,
    0.016432023,
    0.041244943,
    0.031222174,
    -0.0053466456,
    0.016777335,
    0.004303855,
    -0.0051430822,
    -0.01962097,
    0.00046041392,
    0.009175838,
    -0.008946787,
    -0.041479073,
    0.0012780037,
    0.01963695,
    -0.026783299,
    -0.01092655,
    0.03702143,
    0.012992049,
    0.008260065,
    -0.018874738,
    -0.01286012,
    0.016152298,
    -0.024768036,
    -0.024065694,
    0.0008564311,
    -0.003723401,
    -0.0047782045,
    0.012646516,
    0.011130584,
    0.007987915,
    -0.13179192,
    -0.018177606,
    0.02961083,
    0.010106819,
    0.008113584,
    -0.030036584,
    0.012636336,
    0.029913815,
    0.03315664,
    -0.008453596,
    -0.03339465,
    0.0021889387,
    0.013170344,
    -0.01902177,
    0.005910975,
    0.022003956,
    -0.0063015297,
    -0.0185965,
    0.0033527578,
    -0.022245914,
    -0.042567033,
    0.002801951,
    -0.17528647,
    0.0005035894,
    -0.017844167,
    -0.04551095,
    0.011306323,
    -0.030462844,
    0.0017954145,
    0.0061569316,
    0.019132044,
    0.029423045,
    0.023821782,
    0.018651243,
    0.062674895,
    0.008055076,
    0.027926216,
    0.0040267725,
    -0.0015232497,
    -0.010748787,
    -0.013262485,
    0.008980097,
    -0.033223867,
    0.0146368295,
    0.022167355,
    0.009057029,
    -0.023929827,
    -0.02951758,
    -0.0056341076,
    0.06293271,
    -0.017162772,
    0.026563834,
    0.055115834,
    0.03297112,
    0.044023864,
    0.03940343,
    0.030845787,
    -0.009692795,
    -0.00940617,
    -0.017781934,
    -0.0047045476,
    -0.017536366,
    -0.029622015,
    -0.026149537,
    0.014223205,
    0.042495977,
    -0.0290101,
    0.044529866,
    -0.0454436,
    -0.017035026,
    -0.043106273,
    0.004973654,
    0.29866093,
    -0.002671509,
    -0.035108592,
    -0.004368086,
    -0.037166778,
    -0.05845625,
    -0.0010122175,
    0.011301448,
    -0.035917412,
    -0.0042722896,
    0.0069688833,
    0.04308006,
    0.014895897,
    -0.00661524,
    -0.036040846,
    0.022869103,
    -0.004199664,
    -0.010235386,
    0.0077593494,
    -0.0121860765,
    -0.046512168,
    -0.0064643933,
    -0.0047807526,
    -0.018116102,
    0.023745356,
    -0.040249992,
    -0.031160146,
    -0.05771907,
    -0.02815563,
    -0.0068371277,
    -0.01035654,
    0.024611121,
    -0.007522822,
    0.017330028,
    0.022064786,
    0.011030672,
    -0.011998312,
    -0.0041401656,
    -0.0062133586,
    -0.04972406,
    -0.011494944,
    -0.0047495724,
    0.018067274,
    0.039112672,
    -0.019449852,
    0.0065324428,
    -0.02769223,
    -0.039807513,
    0.006461706,
    0.035815254,
    0.0017134275,
    -0.005184694,
    -0.022443162,
    -0.0072568725,
    -0.002618277,
    0.015006618,
    -0.007317327,
    0.037664324,
    -0.023994833,
    0.0054134326,
    -0.003410414,
    -0.0237863,
    0.01482158,
    -0.014767443,
    -0.015756682,
    -0.0022374734,
    0.026522176,
    0.0030798607,
    -0.012200735,
    -0.0686059,
    -0.01256213,
    0.01759631,
    0.0014242876,
    0.044622954,
    0.028350726,
    -0.008226041,
    0.015207355,
    0.0146250725,
    0.015122039,
    -0.03984472,
    -0.02007866,
    0.0028963448,
    0.039672844,
    -0.057417013,
    0.048817653,
    -0.02627826,
    0.0134779485,
    -0.008799786,
    -0.0030325444,
    -0.012617669,
    -0.00087181904,
    0.019178504,
    0.011707547,
    -0.0065853586,
    -0.008898021,
    0.015297573,
    -0.04113959,
    0.01135404,
    -0.018460345,
    0.005675249,
    -0.02876876,
    -0.0065206215,
    0.006008467,
    0.04377509,
    -0.016163269,
    -0.009146873,
    -0.0015525562,
    0.0007020318,
    -0.02461698,
    -0.0344008,
    0.012333875,
    -0.011139719,
    0.011816653,
    -0.014555361,
    -0.0003070767,
    -0.00907902,
    -0.19088055,
    0.015713643,
    0.037807066,
    -0.019069457,
    -0.008042357,
    0.049934104,
    -0.021369996,
    0.0140267825,
    0.00420878,
    0.007308135,
    -0.028600363,
    0.016940795,
    -0.05842496,
    0.006888315,
    0.065117255,
    0.020332089,
    0.014443868,
    -0.065477155,
    0.0010837859,
    -0.005974733,
    0.007969608,
    -0.07594507,
    0.0029710634,
    0.010829651,
    -0.0012731664,
    0.0017792372,
    -0.014663885,
    -0.0203348,
    0.016117094,
    -0.03351677,
    -0.031653583,
    0.0020854105,
    -0.036179002,
    0.0034623882,
    0.010883555,
    0.029086262,
    -0.037473448,
    0.02590499,
    -0.008166385,
    0.009189521,
    0.020489529,
    0.038782965,
    0.029644571,
    -0.0018531352,
    0.047954768,
    -0.014560271,
    0.03497629,
    -0.2864895,
    0.030249074,
    -0.008526756,
    -0.03771894,
    -0.03704407,
    -0.056556262,
    -0.030370766,
    -0.015169972,
    0.03480281,
    0.006294808,
    -0.0067806765,
    0.011883565,
    -0.026535155,
    0.026770437,
    -0.040663313,
    0.005396514,
    -0.0063958433,
    -0.0102125285,
    0.040829312,
    0.02465255,
    0.050618887,
    -0.02336513,
    -0.01293364,
    -0.004051999,
    0.021325089,
    -0.056525428,
    0.008540481,
    0.017834686,
    -0.022880128,
    0.005065879,
    -0.023469102,
    0.024000825,
    0.028049674,
    0.01294549,
    0.026906919,
    0.0038596892,
    -0.018538222,
    -0.01048302,
    0.068679444,
    0.020244043,
    0.018993724,
    0.019902077,
    0.017294884,
    0.010387368,
    -0.013022128,
    -0.007912021,
    0.016969386,
    -0.0005016011,
    0.014465001,
    -0.0020284972,
    0.0066795074,
    0.0014131917,
    -0.039595082,
    -0.037901003,
    -0.0056755184,
    -0.0134587595,
    -0.019023392,
    0.02653589,
    -0.010009279,
    0.012755573,
    0.021138454,
    0.024111101,
    -0.0049227146,
    -0.021821024,
    -0.0038105084,
    0.00024833335,
    0.0015837409,
    0.011216936,
    -0.0011316041,
    0.040861413,
    0.030079305,
    0.020069895,
    0.018964952,
    0.025762206,
    -0.027975056,
    0.006083612,
    0.041216183,
    -0.0198914,
    -0.037045345,
    0.009628558,
    -0.004648141,
    -0.023070302,
    -0.025827674,
    0.032872,
    0.05590265,
    -0.0074252035,
    -0.02661827,
    -0.018210603,
    0.0076413676,
    0.026913233,
    0.014321531,
    0.0049917623,
    0.029138755,
    -0.00072933227,
    0.012737821,
    -0.011692181,
    0.01370206,
    -0.019096408,
    -0.017844934,
    -0.036554847,
    0.046650723,
    -0.01349189,
    -0.02140371,
    -0.016438346,
    -0.013416906,
    0.0006781695,
    0.060341988,
    -0.020184021,
    0.006736895,
    -0.005342232,
    -0.0012715309,
    -0.023459038,
    0.021250091,
    -0.01638936,
    0.009222685,
    0.017368332,
    0.005119245,
    -0.014245158,
    0.070186354,
    -0.0136648305,
    0.015072559,
    0.011200258,
    0.0020309482,
    -0.011483067,
    0.032985736,
    0.040143743,
    -0.02111509,
    0.009001113,
    -0.016965754,
    -0.0035368428,
    -0.03147873,
    0.038750287,
    -0.025312606,
    -0.010575393,
    -0.0041843024,
    -0.025241813,
    -0.02481768,
    0.0054268795,
    -0.013957707,
    -0.005630476,
    0.016301252,
    -0.009564899,
    0.040901873,
    0.0077344235,
    -0.034312457,
    0.0070438446,
    0.06272498,
    0.04281858,
    -0.012747501,
    0.057406064,
    0.03071193,
    -0.00536834,
    -0.03017276,
    -0.019532941,
    -0.0067724953,
    -0.0092885615,
    0.042543396,
    -0.056247883,
    -0.026811935,
    0.020648511,
    -0.04053965,
    0.009476278,
    0.0073615,
    0.009893717,
    0.012109694,
    0.022592265,
    -0.060574617,
    0.010043578,
    0.016987907,
    0.03377897,
    -0.030498004,
    -0.01750739,
    0.067946605,
    0.007580749,
    0.0012963444,
    0.019019201,
    -0.0069742682,
    -0.011390442,
    0.29936674,
    0.03025758,
    -0.033098254,
    -0.020426897,
    0.019291064,
    -0.022534057,
    0.016344719,
    -0.023486739,
    0.018488541,
    -0.048147343,
    -0.010020716,
    -0.040037777,
    -0.03153086,
    -0.054456603,
    0.0065903524,
    0.024521971,
    -0.060160343,
    0.0045516137,
    0.013521374,
    -0.010743019,
    0.008624498,
    0.04089224,
    -0.021292338,
    0.002317146,
    0.02656671,
    0.024010226,
    0.020137409,
    -0.030693509,
    -0.060116753,
    0.024448955,
    0.015258921,
    -0.04637649,
    0.013759733,
    0.0059404382,
    -0.006709233,
    0.019880014,
];
#[localstack_test(services = [OpenSearch()])]
async fn should_return_k_nearest_neighbors() {
    let client = get_opensearch_client().await;
    let repository = ProductOpenSearchRepositoryImpl::new(client);
    let mut documents = fake::vec![ProductDocument; 20];
    for document in &mut documents {
        document.embedding = Some(EXAMPLE_EMBEDDING.into());
    }

    for document in documents.clone() {
        let response = repository
            .create_product_documents(vec![document])
            .await
            .unwrap();
        assert!(!response.errors);
    }
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_secs(10)).await;

    let actual = repository.k_nn_text(&EXAMPLE_EMBEDDING, 20).await.unwrap();

    assert!(actual.hits.hits.len() > 1);
}

#[rstest::rstest]
#[trace]
#[test_attr(apply(test))]
#[case(&[ShopType::AuctionHouse])]
#[case(&[ShopType::AuctionHouse, ShopType::CommercialDealer])]
#[localstack_test(services = [OpenSearch()])]
async fn should_search_product_documents_when_shop_types_are_given(
    #[case] shop_types: &[ShopType],
) {
    let products = fake::vec![ProductDocument; 3000]
        .into_iter()
        .map(|mut item| {
            item.title_de = Some("Test product for shop type filter".into());
            item
        })
        .collect::<Vec<_>>();
    let client = get_opensearch_client().await;
    let repository = ProductOpenSearchRepositoryImpl::new(client);
    let response = repository
        .create_product_documents(products.clone())
        .await
        .unwrap();
    assert!(!response.errors);
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_millis(3000)).await;

    let search_filter = ProductSearch {
        language: Language::De,
        currency: Currency::Eur,
        product_query: vec!["Test product for shop type filter".try_into().unwrap()],
        enhanced_search_description: None,
        exclude_product_id_query: Default::default(),
        shop_name_query: Default::default(),
        exclude_shop_name_query: Default::default(),
        seller_name_query: Default::default(),
        exclude_seller_name_query: Default::default(),
        shop_type_query: AnyOfQuery::from(HashSet::from_iter(shop_types.iter().copied())),
        country_query: Default::default(),
        continent_query: Default::default(),
        geo_address_distance_query: None,
        price_query: None,
        state_query: Default::default(),
        lifecycle_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
        shop_slug_id_query: Default::default(),
        exclude_shop_slug_id_query: Default::default(),
        seller_slug_id_query: Default::default(),
        exclude_seller_slug_id_query: Default::default(),
    };
    let response = repository
        .search_product_documents(
            &search_filter,
            &Sort {
                sort: SortProductField::Score,
                order: SortOrder::Desc,
            },
            &None,
        )
        .await
        .unwrap();

    assert!(response.hits.total.value > 0);
    assert!(
        response
            .hits
            .hits
            .iter()
            .all(|hit| { shop_types.contains(&ShopType::from(hit.source.shop_type)) })
    );
}

#[rstest::rstest]
#[test_attr(apply(test))]
#[case(&["Sotheby's"])]
#[case(&["Sotheby's", "Christie's", "Heritage Auctions"])]
#[trace]
#[localstack_test(services = [OpenSearch()])]
async fn should_search_product_documents_when_shop_names_are_given_for_keyword_filter(
    #[case] shop_names: &[&str],
) {
    let products_with_target_shops = fake::vec![ProductDocument; 1500]
        .into_iter()
        .enumerate()
        .map(|(idx, mut item)| {
            item.title_de = Some("Test product for shop name filter".into());
            item.shop_name = shop_names[idx % shop_names.len()].to_string();
            item
        })
        .collect::<Vec<_>>();

    let products_with_other_shops = fake::vec![ProductDocument; 1500]
        .into_iter()
        .map(|mut item| {
            item.title_de = Some("Test product for shop name filter".into());
            item.shop_name = "Other Auction House".to_string();
            item
        })
        .collect::<Vec<_>>();

    let all_products = [products_with_target_shops, products_with_other_shops].concat();

    let client = get_opensearch_client().await;
    let repository = ProductOpenSearchRepositoryImpl::new(client);
    let response = repository
        .create_product_documents(all_products)
        .await
        .unwrap();
    assert!(!response.errors);
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_millis(3000)).await;

    let search_filter = ProductSearch {
        language: Language::De,
        currency: Currency::Eur,
        product_query: vec!["Test product for shop name filter".try_into().unwrap()],
        enhanced_search_description: None,
        exclude_product_id_query: Default::default(),
        shop_name_query: AnyOfQuery::from(HashSet::from_iter(
            shop_names.iter().map(|name| name.to_string().into()),
        )),
        exclude_shop_name_query: Default::default(),
        seller_name_query: Default::default(),
        exclude_seller_name_query: Default::default(),
        shop_type_query: Default::default(),
        country_query: Default::default(),
        continent_query: Default::default(),
        geo_address_distance_query: None,
        price_query: None,
        state_query: Default::default(),
        lifecycle_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
        shop_slug_id_query: Default::default(),
        exclude_shop_slug_id_query: Default::default(),
        seller_slug_id_query: Default::default(),
        exclude_seller_slug_id_query: Default::default(),
    };
    let response = repository
        .search_product_documents(
            &search_filter,
            &Sort {
                sort: SortProductField::Score,
                order: SortOrder::Desc,
            },
            &None,
        )
        .await
        .unwrap();

    assert!(response.hits.total.value > 0);
    assert_eq!(1500, response.hits.total.value);
    assert!(
        response
            .hits
            .hits
            .iter()
            .all(|hit| { shop_names.contains(&hit.source.shop_name.as_str()) })
    );
}

#[rstest::rstest]
#[test_attr(apply(test))]
#[case(&["Sotheby's"])]
#[case(&["Sotheby's", "Christie's", "Heritage Auctions"])]
#[trace]
#[localstack_test(services = [OpenSearch()])]
async fn should_search_product_documents_when_excluded_shop_names_are_given(
    #[case] exclude_shop_names: &[&str],
) {
    let products_with_target_shops = fake::vec![ProductDocument; 1500]
        .into_iter()
        .enumerate()
        .map(|(idx, mut item)| {
            item.title_de = Some("Test product for shop name filter".into());
            item.shop_name = exclude_shop_names[idx % exclude_shop_names.len()].to_string();
            item
        })
        .collect::<Vec<_>>();

    let products_with_other_shops = fake::vec![ProductDocument; 1500]
        .into_iter()
        .map(|mut item| {
            item.title_de = Some("Test product for shop name filter".into());
            item.shop_name = "Other Auction House".to_string();
            item
        })
        .collect::<Vec<_>>();

    let all_products = [products_with_target_shops, products_with_other_shops].concat();

    let client = get_opensearch_client().await;
    let repository = ProductOpenSearchRepositoryImpl::new(client);
    let response = repository
        .create_product_documents(all_products)
        .await
        .unwrap();
    assert!(!response.errors);
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_millis(3000)).await;

    let search_filter = ProductSearch {
        language: Language::De,
        currency: Currency::Eur,
        product_query: vec!["Test product for shop name filter".try_into().unwrap()],
        enhanced_search_description: None,
        exclude_product_id_query: Default::default(),
        shop_name_query: Default::default(),
        exclude_shop_name_query: AnyOfQuery::from(HashSet::from_iter(
            exclude_shop_names
                .iter()
                .map(|name| name.to_string().into()),
        )),
        seller_name_query: Default::default(),
        exclude_seller_name_query: Default::default(),
        shop_type_query: Default::default(),
        country_query: Default::default(),
        continent_query: Default::default(),
        geo_address_distance_query: None,
        price_query: None,
        state_query: Default::default(),
        lifecycle_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
        shop_slug_id_query: Default::default(),
        exclude_shop_slug_id_query: Default::default(),
        seller_slug_id_query: Default::default(),
        exclude_seller_slug_id_query: Default::default(),
    };
    let response = repository
        .search_product_documents(
            &search_filter,
            &Sort {
                sort: SortProductField::Score,
                order: SortOrder::Desc,
            },
            &None,
        )
        .await
        .unwrap();

    assert!(response.hits.total.value > 0);
    assert_eq!(1500, response.hits.total.value);
    assert!(
        response
            .hits
            .hits
            .iter()
            .all(|hit| !exclude_shop_names.contains(&hit.source.shop_name.as_str()))
    );
}

#[rstest::rstest]
#[test_attr(apply(test))]
#[case(&["Sotheby's"])]
#[case(&["Sotheby's", "Christie's", "Heritage Auctions"])]
#[trace]
#[localstack_test(services = [OpenSearch()])]
async fn should_search_product_documents_when_seller_names_are_given_for_keyword_filter(
    #[case] seller_names: &[&str],
) {
    let products_with_target_sellers = fake::vec![ProductDocument; 1500]
        .into_iter()
        .enumerate()
        .map(|(idx, mut item)| {
            item.title_de = Some("Test product for seller name filter".into());
            item.seller_name = seller_names[idx % seller_names.len()].to_string();
            item
        })
        .collect::<Vec<_>>();

    let products_with_other_sellers = fake::vec![ProductDocument; 1500]
        .into_iter()
        .map(|mut item| {
            item.title_de = Some("Test product for seller name filter".into());
            item.seller_name = "Other Seller House".to_string();
            item
        })
        .collect::<Vec<_>>();

    let all_products = [products_with_target_sellers, products_with_other_sellers].concat();

    let client = get_opensearch_client().await;
    let repository = ProductOpenSearchRepositoryImpl::new(client);
    let response = repository
        .create_product_documents(all_products)
        .await
        .unwrap();
    assert!(!response.errors);
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_millis(3000)).await;

    let search_filter = ProductSearch {
        language: Language::De,
        currency: Currency::Eur,
        product_query: vec!["Test product for seller name filter".try_into().unwrap()],
        enhanced_search_description: None,
        exclude_product_id_query: Default::default(),
        shop_name_query: Default::default(),
        exclude_shop_name_query: Default::default(),
        seller_name_query: AnyOfQuery::from(HashSet::from_iter(
            seller_names.iter().map(|name| name.to_string().into()),
        )),
        exclude_seller_name_query: Default::default(),
        shop_type_query: Default::default(),
        country_query: Default::default(),
        continent_query: Default::default(),
        geo_address_distance_query: None,
        price_query: None,
        state_query: Default::default(),
        lifecycle_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
        shop_slug_id_query: Default::default(),
        exclude_shop_slug_id_query: Default::default(),
        seller_slug_id_query: Default::default(),
        exclude_seller_slug_id_query: Default::default(),
    };
    let response = repository
        .search_product_documents(
            &search_filter,
            &Sort {
                sort: SortProductField::Score,
                order: SortOrder::Desc,
            },
            &None,
        )
        .await
        .unwrap();

    assert!(response.hits.total.value > 0);
    assert_eq!(1500, response.hits.total.value);
    assert!(
        response
            .hits
            .hits
            .iter()
            .all(|hit| { seller_names.contains(&hit.source.seller_name.as_str()) })
    );
}

#[rstest::rstest]
#[test_attr(apply(test))]
#[case(&["Sotheby's"])]
#[case(&["Sotheby's", "Christie's", "Heritage Auctions"])]
#[trace]
#[localstack_test(services = [OpenSearch()])]
async fn should_search_product_documents_when_excluded_seller_names_are_given(
    #[case] exclude_seller_names: &[&str],
) {
    let products_with_target_sellers = fake::vec![ProductDocument; 1500]
        .into_iter()
        .enumerate()
        .map(|(idx, mut item)| {
            item.title_de = Some("Test product for exclude seller name filter".into());
            item.seller_name = exclude_seller_names[idx % exclude_seller_names.len()].to_string();
            item
        })
        .collect::<Vec<_>>();

    let products_with_other_sellers = fake::vec![ProductDocument; 1500]
        .into_iter()
        .map(|mut item| {
            item.title_de = Some("Test product for exclude seller name filter".into());
            item.seller_name = "Other Seller House".to_string();
            item
        })
        .collect::<Vec<_>>();

    let all_products = [products_with_target_sellers, products_with_other_sellers].concat();

    let client = get_opensearch_client().await;
    let repository = ProductOpenSearchRepositoryImpl::new(client);
    let response = repository
        .create_product_documents(all_products)
        .await
        .unwrap();
    assert!(!response.errors);
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_millis(3000)).await;

    let search_filter = ProductSearch {
        language: Language::De,
        currency: Currency::Eur,
        product_query: vec![
            "Test product for exclude seller name filter"
                .try_into()
                .unwrap(),
        ],
        enhanced_search_description: None,
        exclude_product_id_query: Default::default(),
        shop_name_query: Default::default(),
        exclude_shop_name_query: Default::default(),
        seller_name_query: Default::default(),
        exclude_seller_name_query: AnyOfQuery::from(HashSet::from_iter(
            exclude_seller_names
                .iter()
                .map(|name| name.to_string().into()),
        )),
        shop_type_query: Default::default(),
        country_query: Default::default(),
        continent_query: Default::default(),
        geo_address_distance_query: None,
        price_query: None,
        state_query: Default::default(),
        lifecycle_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
        shop_slug_id_query: Default::default(),
        exclude_shop_slug_id_query: Default::default(),
        seller_slug_id_query: Default::default(),
        exclude_seller_slug_id_query: Default::default(),
    };
    let response = repository
        .search_product_documents(
            &search_filter,
            &Sort {
                sort: SortProductField::Score,
                order: SortOrder::Desc,
            },
            &None,
        )
        .await
        .unwrap();

    assert!(response.hits.total.value > 0);
    assert_eq!(1500, response.hits.total.value);
    assert!(
        response
            .hits
            .hits
            .iter()
            .all(|hit| !exclude_seller_names.contains(&hit.source.seller_name.as_str()))
    );
}

#[rstest::rstest]
#[test_attr(apply(test))]
#[case(&["imperial-antiques"])]
#[case(&["imperial-antiques", "vintage-collectibles", "heritage-gallery"])]
#[trace]
#[localstack_test(services = [OpenSearch()])]
async fn should_search_product_documents_when_shop_slug_ids_are_given(
    #[case] shop_slug_ids: &[&str],
) {
    let products_with_target_shops = fake::vec![ProductDocument; 1500]
        .into_iter()
        .enumerate()
        .map(|(idx, mut item)| {
            item.title_de = Some("Test product for shop slug id filter".into());
            item.shop_slug_id = ShopSlugId::from(
                shop_slug_ids[idx % shop_slug_ids.len()]
                    .to_string()
                    .as_str(),
            );
            item
        })
        .collect::<Vec<_>>();

    let products_with_other_shops = fake::vec![ProductDocument; 1500]
        .into_iter()
        .map(|mut item| {
            item.title_de = Some("Test product for shop slug id filter".into());
            item.shop_slug_id = ShopSlugId::from("other-antique-shop");
            item
        })
        .collect::<Vec<_>>();

    let all_products = [products_with_target_shops, products_with_other_shops].concat();

    let client = get_opensearch_client().await;
    let repository = ProductOpenSearchRepositoryImpl::new(client);
    let response = repository
        .create_product_documents(all_products)
        .await
        .unwrap();
    assert!(!response.errors);
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_millis(3000)).await;

    let search_filter = ProductSearch {
        language: Language::De,
        currency: Currency::Eur,
        product_query: vec!["Test product for shop slug id filter".try_into().unwrap()],
        enhanced_search_description: None,
        exclude_product_id_query: Default::default(),
        shop_name_query: Default::default(),
        exclude_shop_name_query: Default::default(),
        seller_name_query: Default::default(),
        exclude_seller_name_query: Default::default(),
        shop_slug_id_query: AnyOfQuery::from(HashSet::from_iter(
            shop_slug_ids.iter().map(|slug| ShopSlugId::from(*slug)),
        )),
        exclude_shop_slug_id_query: Default::default(),
        seller_slug_id_query: Default::default(),
        exclude_seller_slug_id_query: Default::default(),
        shop_type_query: Default::default(),
        country_query: Default::default(),
        continent_query: Default::default(),
        geo_address_distance_query: None,
        price_query: None,
        state_query: Default::default(),
        lifecycle_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
    };
    let response = repository
        .search_product_documents(
            &search_filter,
            &Sort {
                sort: SortProductField::Score,
                order: SortOrder::Desc,
            },
            &None,
        )
        .await
        .unwrap();

    assert!(response.hits.total.value > 0);
    assert_eq!(1500, response.hits.total.value);
    assert!(
        response
            .hits
            .hits
            .iter()
            .all(|hit| shop_slug_ids.contains(&hit.source.shop_slug_id.to_string().as_str()))
    );
}

#[rstest::rstest]
#[test_attr(apply(test))]
#[case(&["imperial-antiques"])]
#[case(&["imperial-antiques", "vintage-collectibles", "heritage-gallery"])]
#[trace]
#[localstack_test(services = [OpenSearch()])]
async fn should_search_product_documents_when_excluded_shop_slug_ids_are_given(
    #[case] exclude_shop_slug_ids: &[&str],
) {
    let products_with_target_shops = fake::vec![ProductDocument; 1500]
        .into_iter()
        .enumerate()
        .map(|(idx, mut item)| {
            item.title_de = Some("Test product for exclude shop slug id filter".into());
            item.shop_slug_id = ShopSlugId::from(
                exclude_shop_slug_ids[idx % exclude_shop_slug_ids.len()]
                    .to_string()
                    .as_str(),
            );
            item
        })
        .collect::<Vec<_>>();

    let products_with_other_shops = fake::vec![ProductDocument; 1500]
        .into_iter()
        .map(|mut item| {
            item.title_de = Some("Test product for exclude shop slug id filter".into());
            item.shop_slug_id = ShopSlugId::from("other-antique-shop");
            item
        })
        .collect::<Vec<_>>();

    let all_products = [products_with_target_shops, products_with_other_shops].concat();

    let client = get_opensearch_client().await;
    let repository = ProductOpenSearchRepositoryImpl::new(client);
    let response = repository
        .create_product_documents(all_products)
        .await
        .unwrap();
    assert!(!response.errors);
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_millis(3000)).await;

    let search_filter = ProductSearch {
        language: Language::De,
        currency: Currency::Eur,
        product_query: vec![
            "Test product for exclude shop slug id filter"
                .try_into()
                .unwrap(),
        ],
        enhanced_search_description: None,
        exclude_product_id_query: Default::default(),
        shop_name_query: Default::default(),
        exclude_shop_name_query: Default::default(),
        seller_name_query: Default::default(),
        exclude_seller_name_query: Default::default(),
        shop_slug_id_query: Default::default(),
        exclude_shop_slug_id_query: AnyOfQuery::from(HashSet::from_iter(
            exclude_shop_slug_ids
                .iter()
                .map(|slug| ShopSlugId::from(*slug)),
        )),
        seller_slug_id_query: Default::default(),
        exclude_seller_slug_id_query: Default::default(),
        shop_type_query: Default::default(),
        country_query: Default::default(),
        continent_query: Default::default(),
        geo_address_distance_query: None,
        price_query: None,
        state_query: Default::default(),
        lifecycle_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
    };
    let response = repository
        .search_product_documents(
            &search_filter,
            &Sort {
                sort: SortProductField::Score,
                order: SortOrder::Desc,
            },
            &None,
        )
        .await
        .unwrap();

    assert!(response.hits.total.value > 0);
    assert_eq!(1500, response.hits.total.value);
    assert!(
        response
            .hits
            .hits
            .iter()
            .all(|hit| !exclude_shop_slug_ids
                .contains(&hit.source.shop_slug_id.to_string().as_str()))
    );
}

#[rstest::rstest]
#[test_attr(apply(test))]
#[case(&["imperial-antiques"])]
#[case(&["imperial-antiques", "vintage-seller", "heritage-auctions"])]
#[trace]
#[localstack_test(services = [OpenSearch()])]
async fn should_search_product_documents_when_seller_slug_ids_are_given(
    #[case] seller_slug_ids: &[&str],
) {
    let products_with_target_sellers = fake::vec![ProductDocument; 1500]
        .into_iter()
        .enumerate()
        .map(|(idx, mut item)| {
            item.title_de = Some("Test product for seller slug id filter".into());
            item.seller_slug_id = SellerSlugId::from(
                seller_slug_ids[idx % seller_slug_ids.len()]
                    .to_string()
                    .as_str(),
            );
            item
        })
        .collect::<Vec<_>>();

    let products_with_other_sellers = fake::vec![ProductDocument; 1500]
        .into_iter()
        .map(|mut item| {
            item.title_de = Some("Test product for seller slug id filter".into());
            item.seller_slug_id = SellerSlugId::from("other-seller-shop");
            item
        })
        .collect::<Vec<_>>();

    let all_products = [products_with_target_sellers, products_with_other_sellers].concat();

    let client = get_opensearch_client().await;
    let repository = ProductOpenSearchRepositoryImpl::new(client);
    let response = repository
        .create_product_documents(all_products)
        .await
        .unwrap();
    assert!(!response.errors);
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_millis(3000)).await;

    let search_filter = ProductSearch {
        language: Language::De,
        currency: Currency::Eur,
        product_query: vec!["Test product for seller slug id filter".try_into().unwrap()],
        enhanced_search_description: None,
        exclude_product_id_query: Default::default(),
        shop_name_query: Default::default(),
        exclude_shop_name_query: Default::default(),
        seller_name_query: Default::default(),
        exclude_seller_name_query: Default::default(),
        shop_slug_id_query: Default::default(),
        exclude_shop_slug_id_query: Default::default(),
        seller_slug_id_query: AnyOfQuery::from(HashSet::from_iter(
            seller_slug_ids.iter().map(|slug| SellerSlugId::from(*slug)),
        )),
        exclude_seller_slug_id_query: Default::default(),
        shop_type_query: Default::default(),
        country_query: Default::default(),
        continent_query: Default::default(),
        geo_address_distance_query: None,
        price_query: None,
        state_query: Default::default(),
        lifecycle_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
    };
    let response = repository
        .search_product_documents(
            &search_filter,
            &Sort {
                sort: SortProductField::Score,
                order: SortOrder::Desc,
            },
            &None,
        )
        .await
        .unwrap();

    assert!(response.hits.total.value > 0);
    assert_eq!(1500, response.hits.total.value);
    assert!(
        response
            .hits
            .hits
            .iter()
            .all(|hit| seller_slug_ids.contains(&hit.source.seller_slug_id.to_string().as_str()))
    );
}

#[rstest::rstest]
#[test_attr(apply(test))]
#[case(&["imperial-antiques"])]
#[case(&["imperial-antiques", "vintage-seller", "heritage-auctions"])]
#[trace]
#[localstack_test(services = [OpenSearch()])]
async fn should_search_product_documents_when_excluded_seller_slug_ids_are_given(
    #[case] exclude_seller_slug_ids: &[&str],
) {
    let products_with_target_sellers = fake::vec![ProductDocument; 1500]
        .into_iter()
        .enumerate()
        .map(|(idx, mut item)| {
            item.title_de = Some("Test product for exclude seller slug id filter".into());
            item.seller_slug_id = SellerSlugId::from(
                exclude_seller_slug_ids[idx % exclude_seller_slug_ids.len()]
                    .to_string()
                    .as_str(),
            );
            item
        })
        .collect::<Vec<_>>();

    let products_with_other_sellers = fake::vec![ProductDocument; 1500]
        .into_iter()
        .map(|mut item| {
            item.title_de = Some("Test product for exclude seller slug id filter".into());
            item.seller_slug_id = SellerSlugId::from("other-seller-shop");
            item
        })
        .collect::<Vec<_>>();

    let all_products = [products_with_target_sellers, products_with_other_sellers].concat();

    let client = get_opensearch_client().await;
    let repository = ProductOpenSearchRepositoryImpl::new(client);
    let response = repository
        .create_product_documents(all_products)
        .await
        .unwrap();
    assert!(!response.errors);
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_millis(3000)).await;

    let search_filter = ProductSearch {
        language: Language::De,
        currency: Currency::Eur,
        product_query: vec![
            "Test product for exclude seller slug id filter"
                .try_into()
                .unwrap(),
        ],
        enhanced_search_description: None,
        exclude_product_id_query: Default::default(),
        shop_name_query: Default::default(),
        exclude_shop_name_query: Default::default(),
        seller_name_query: Default::default(),
        exclude_seller_name_query: Default::default(),
        shop_slug_id_query: Default::default(),
        exclude_shop_slug_id_query: Default::default(),
        seller_slug_id_query: Default::default(),
        exclude_seller_slug_id_query: AnyOfQuery::from(HashSet::from_iter(
            exclude_seller_slug_ids
                .iter()
                .map(|slug| SellerSlugId::from(*slug)),
        )),
        shop_type_query: Default::default(),
        country_query: Default::default(),
        continent_query: Default::default(),
        geo_address_distance_query: None,
        price_query: None,
        state_query: Default::default(),
        lifecycle_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
    };
    let response = repository
        .search_product_documents(
            &search_filter,
            &Sort {
                sort: SortProductField::Score,
                order: SortOrder::Desc,
            },
            &None,
        )
        .await
        .unwrap();

    assert!(response.hits.total.value > 0);
    assert_eq!(1500, response.hits.total.value);
    assert!(response.hits.hits.iter().all(|hit| {
        !exclude_seller_slug_ids.contains(&hit.source.seller_slug_id.to_string().as_str())
    }));
}

#[rstest::rstest]
#[trace]
#[test_attr(apply(test))]
#[case(RangeQuery { min: Some(datetime!(2026-01-01 0:00 UTC)), max: Some(datetime!(2026-01-31 23:59 UTC)) })]
#[case(RangeQuery { min: Some(datetime!(2026-02-01 0:00 UTC)), max: None })]
#[case(RangeQuery { min: None, max: Some(datetime!(2026-12-31 23:59 UTC)) })]
#[localstack_test(services = [OpenSearch()])]
async fn should_search_product_documents_when_auction_start_range_is_given(
    #[case] auction_start_query: RangeQuery<OffsetDateTime>,
) {
    let early_auction_products = fake::vec![ProductDocument; 50]
        .into_iter()
        .map(|mut product| {
            product.title_de = Some("Auction product".into());
            product.auction_start = Some(datetime!(2026-01-15 10:00 UTC));
            product.auction_end = Some(datetime!(2026-01-15 14:00 UTC));
            product
        })
        .collect::<Vec<_>>();
    let late_auction_products = fake::vec![ProductDocument; 50]
        .into_iter()
        .map(|mut product| {
            product.title_de = Some("Auction product".into());
            product.auction_start = Some(datetime!(2026-06-20 10:00 UTC));
            product.auction_end = Some(datetime!(2026-06-20 14:00 UTC));
            product
        })
        .collect::<Vec<_>>();
    let no_auction_products = fake::vec![ProductDocument; 50]
        .into_iter()
        .map(|mut product| {
            product.title_de = Some("Auction product".into());
            product.auction_start = None;
            product.auction_end = None;
            product
        })
        .collect::<Vec<_>>();
    let products = [
        early_auction_products,
        late_auction_products,
        no_auction_products,
    ]
    .concat();
    let client = get_opensearch_client().await;
    let repository = ProductOpenSearchRepositoryImpl::new(client);
    let response = repository
        .create_product_documents(products.clone())
        .await
        .unwrap();
    assert!(!response.errors);
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_millis(3000)).await;

    let search_filter = ProductSearch {
        language: Language::De,
        currency: Currency::Eur,
        product_query: vec!["Auction product".try_into().unwrap()],
        enhanced_search_description: None,
        exclude_product_id_query: Default::default(),
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
        lifecycle_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: Some(auction_start_query),
        auction_end_query: None,
        shop_slug_id_query: Default::default(),
        exclude_shop_slug_id_query: Default::default(),
        seller_slug_id_query: Default::default(),
        exclude_seller_slug_id_query: Default::default(),
    };
    let response = repository
        .search_product_documents(
            &search_filter,
            &Sort {
                sort: SortProductField::Score,
                order: SortOrder::Desc,
            },
            &Some(Cursor {
                size: 200,
                search_after: None,
            }),
        )
        .await
        .unwrap();
    let actual_items = response
        .hits
        .hits
        .into_iter()
        .map(|hit| hit.source)
        .collect::<Vec<_>>();
    let expected_products = products
        .into_iter()
        .filter(|product| {
            if let Some(auction_start) = product.auction_start {
                let mut filter = true;
                if let Some(min) = auction_start_query.min {
                    filter = filter && auction_start >= min;
                }
                if let Some(max) = auction_start_query.max {
                    filter = filter && auction_start <= max;
                }
                filter
            } else {
                false
            }
        })
        .collect::<Vec<_>>();

    assert_eq!(expected_products.len(), actual_items.len());
    assert!(
        expected_products
            .iter()
            .all(|product| actual_items.contains(product))
    );
}

#[rstest::rstest]
#[trace]
#[test_attr(apply(test))]
#[case(RangeQuery { min: Some(datetime!(2026-01-01 0:00 UTC)), max: Some(datetime!(2026-01-31 23:59 UTC)) })]
#[case(RangeQuery { min: Some(datetime!(2026-06-01 0:00 UTC)), max: None })]
#[case(RangeQuery { min: None, max: Some(datetime!(2026-12-31 23:59 UTC)) })]
#[localstack_test(services = [OpenSearch()])]
async fn should_search_product_documents_when_auction_end_range_is_given(
    #[case] auction_end_query: RangeQuery<OffsetDateTime>,
) {
    let early_auction_products = fake::vec![ProductDocument; 50]
        .into_iter()
        .map(|mut product| {
            product.title_de = Some("Auction item".into());
            product.auction_start = Some(datetime!(2026-01-15 10:00 UTC));
            product.auction_end = Some(datetime!(2026-01-15 14:00 UTC));
            product
        })
        .collect::<Vec<_>>();
    let late_auction_products = fake::vec![ProductDocument; 50]
        .into_iter()
        .map(|mut product| {
            product.title_de = Some("Auction item".into());
            product.auction_start = Some(datetime!(2026-06-20 10:00 UTC));
            product.auction_end = Some(datetime!(2026-06-20 14:00 UTC));
            product
        })
        .collect::<Vec<_>>();
    let no_auction_products = fake::vec![ProductDocument; 50]
        .into_iter()
        .map(|mut product| {
            product.title_de = Some("Auction item".into());
            product.auction_start = None;
            product.auction_end = None;
            product
        })
        .collect::<Vec<_>>();
    let products = [
        early_auction_products,
        late_auction_products,
        no_auction_products,
    ]
    .concat();
    let client = get_opensearch_client().await;
    let repository = ProductOpenSearchRepositoryImpl::new(client);
    let response = repository
        .create_product_documents(products.clone())
        .await
        .unwrap();
    assert!(!response.errors);
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_millis(3000)).await;

    let search_filter = ProductSearch {
        language: Language::De,
        currency: Currency::Eur,
        product_query: vec!["Auction item".try_into().unwrap()],
        enhanced_search_description: None,
        exclude_product_id_query: Default::default(),
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
        lifecycle_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: Some(auction_end_query),
        shop_slug_id_query: Default::default(),
        exclude_shop_slug_id_query: Default::default(),
        seller_slug_id_query: Default::default(),
        exclude_seller_slug_id_query: Default::default(),
    };
    let response = repository
        .search_product_documents(
            &search_filter,
            &Sort {
                sort: SortProductField::Score,
                order: SortOrder::Desc,
            },
            &Some(Cursor {
                size: 200,
                search_after: None,
            }),
        )
        .await
        .unwrap();
    let actual_items = response
        .hits
        .hits
        .into_iter()
        .map(|hit| hit.source)
        .collect::<Vec<_>>();
    let expected_products = products
        .into_iter()
        .filter(|product| {
            if let Some(auction_end) = product.auction_end {
                let mut filter = true;
                if let Some(min) = auction_end_query.min {
                    filter = filter && auction_end >= min;
                }
                if let Some(max) = auction_end_query.max {
                    filter = filter && auction_end <= max;
                }
                filter
            } else {
                false
            }
        })
        .collect::<Vec<_>>();

    assert_eq!(expected_products.len(), actual_items.len());
    assert!(
        expected_products
            .iter()
            .all(|product| actual_items.contains(product))
    );
}

#[rstest::rstest]
#[trace]
#[test_attr(apply(test))]
#[case(RangeQuery { min: Some(datetime!(2026-01-01 0:00 UTC)), max: Some(datetime!(2026-01-31 23:59 UTC)) })]
#[case(RangeQuery { min: Some(datetime!(2026-06-01 0:00 UTC)), max: None })]
#[case(RangeQuery { min: None, max: Some(datetime!(2026-12-31 23:59 UTC)) })]
#[localstack_test(services = [OpenSearch()])]
async fn should_search_product_documents_when_query_is_empty(
    #[case] auction_end_query: RangeQuery<OffsetDateTime>,
) {
    let early_auction_products = fake::vec![ProductDocument; 50]
        .into_iter()
        .map(|mut product| {
            product.title_de = Some("Auction item".into());
            product.auction_start = Some(datetime!(2026-01-15 10:00 UTC));
            product.auction_end = Some(datetime!(2026-01-15 14:00 UTC));
            product
        })
        .collect::<Vec<_>>();
    let late_auction_products = fake::vec![ProductDocument; 50]
        .into_iter()
        .map(|mut product| {
            product.title_de = Some("Auction item".into());
            product.auction_start = Some(datetime!(2026-06-20 10:00 UTC));
            product.auction_end = Some(datetime!(2026-06-20 14:00 UTC));
            product
        })
        .collect::<Vec<_>>();
    let no_auction_products = fake::vec![ProductDocument; 50]
        .into_iter()
        .map(|mut product| {
            product.title_de = Some("Auction item".into());
            product.auction_start = None;
            product.auction_end = None;
            product
        })
        .collect::<Vec<_>>();
    let products = [
        early_auction_products,
        late_auction_products,
        no_auction_products,
    ]
    .concat();
    let client = get_opensearch_client().await;
    let repository = ProductOpenSearchRepositoryImpl::new(client);
    let response = repository
        .create_product_documents(products.clone())
        .await
        .unwrap();
    assert!(!response.errors);
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_millis(3000)).await;

    let search_filter = ProductSearch {
        language: Language::De,
        currency: Currency::Eur,
        product_query: Vec::new(),
        enhanced_search_description: None,
        exclude_product_id_query: Default::default(),
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
        lifecycle_query: Default::default(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: Some(auction_end_query),
        shop_slug_id_query: Default::default(),
        exclude_shop_slug_id_query: Default::default(),
        seller_slug_id_query: Default::default(),
        exclude_seller_slug_id_query: Default::default(),
    };
    let response = repository
        .search_product_documents(
            &search_filter,
            &Sort {
                sort: SortProductField::Score,
                order: SortOrder::Desc,
            },
            &Some(Cursor {
                size: 200,
                search_after: None,
            }),
        )
        .await
        .unwrap();
    let actual_items = response
        .hits
        .hits
        .into_iter()
        .map(|hit| hit.source)
        .collect::<Vec<_>>();
    let expected_products = products
        .into_iter()
        .filter(|product| {
            if let Some(auction_end) = product.auction_end {
                let mut filter = true;
                if let Some(min) = auction_end_query.min {
                    filter = filter && auction_end >= min;
                }
                if let Some(max) = auction_end_query.max {
                    filter = filter && auction_end <= max;
                }
                filter
            } else {
                false
            }
        })
        .collect::<Vec<_>>();

    assert_eq!(expected_products.len(), actual_items.len());
    assert!(
        expected_products
            .iter()
            .all(|product| actual_items.contains(product))
    );
}

// =====================================================================================
// OpenSearch-native hybrid (BM25 + kNN) search integration tests
// =====================================================================================
//
// These tests prove that hybrid_search_product_documents produces a request OpenSearch
// can execute against the configured products index, and that the relevant native-hybrid
// behaviours (RRF fusion, filter pass-through, paging, and response shaping) work end-to-end.

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
        product_query: vec![query.try_into().unwrap()],
        enhanced_search_description: None,
        exclude_product_id_query: Default::default(),
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
        lifecycle_query: Default::default(),
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
async fn should_return_bm25_and_knn_hits_when_both_branches_match_for_product_search() {
    let bm25_only = make_product_doc(|doc| {
        set_titles(doc, "Rolex Submariner Vintage 1965");
        doc.embedding = Some(one_hot_embedding(0, 1.0).into());
    });
    let knn_only = make_product_doc(|doc| {
        set_titles(doc, "lorem ipsum dolor");
        doc.embedding = Some(one_hot_embedding(7, 1.0).into());
    });
    let unrelated = make_product_doc(|doc| {
        set_titles(doc, "lorem ipsum dolor");
        doc.embedding = Some(one_hot_embedding(500, 1.0).into());
    });

    let repository = ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    repository
        .create_product_documents(vec![bm25_only.clone(), knn_only.clone(), unrelated])
        .await
        .unwrap();
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_secs(2)).await;

    let response = repository
        .hybrid_search_product_documents(
            &search_with_query("Rolex Submariner"),
            &one_hot_embedding(7, 1.0),
            &None,
        )
        .await
        .unwrap();

    let returned_ids: HashSet<_> = response
        .hits
        .hits
        .iter()
        .map(|hit| hit.source.product_id)
        .collect();
    assert!(returned_ids.contains(&bm25_only.product_id));
    assert!(returned_ids.contains(&knn_only.product_id));
}

#[localstack_test(services = [OpenSearch()])]
async fn should_rank_dual_match_first_when_both_branches_contribute_for_product_search() {
    let query = "Meissen porcelain figurine 1750";
    let dual_match = make_product_doc(|doc| {
        set_titles(doc, query);
        doc.embedding = Some(one_hot_embedding(42, 1.0).into());
    });
    let bm25_only = make_product_doc(|doc| {
        set_titles(doc, query);
        doc.embedding = Some(one_hot_embedding(43, 1.0).into());
    });
    let semantic_only = make_product_doc(|doc| {
        set_titles(doc, "decorative porcelain figure");
        doc.embedding = Some(one_hot_embedding(42, 1.0).into());
    });

    let repository = ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    repository
        .create_product_documents(vec![dual_match.clone(), bm25_only, semantic_only])
        .await
        .unwrap();
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_secs(2)).await;

    let response = repository
        .hybrid_search_product_documents(
            &search_with_query(query),
            &one_hot_embedding(42, 1.0),
            &None,
        )
        .await
        .unwrap();

    assert!(!response.hits.hits.is_empty());
    assert_eq!(
        response.hits.hits[0].source.product_id,
        dual_match.product_id
    );
}

#[localstack_test(services = [OpenSearch()])]
async fn should_exclude_embedding_from_source_when_returning_hybrid_hits_for_product_search() {
    let doc = make_product_doc(|product| {
        set_titles(product, "Tea Cup Set");
        product.embedding = Some(one_hot_embedding(11, 1.0).into());
    });

    let repository = ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    repository
        .create_product_documents(vec![doc.clone()])
        .await
        .unwrap();
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_secs(2)).await;

    let response = repository
        .hybrid_search_product_documents(
            &search_with_query("Tea Cup Set"),
            &one_hot_embedding(11, 1.0),
            &None,
        )
        .await
        .unwrap();

    let hit = response
        .hits
        .hits
        .into_iter()
        .find(|hit| hit.source.product_id == doc.product_id)
        .expect("freshly-indexed document must be returned");
    assert!(hit.source.embedding.is_none());
}

#[localstack_test(services = [OpenSearch()])]
async fn should_honor_page_size_when_loading_first_hybrid_page_for_product_search() {
    let docs: Vec<ProductDocument> = (0..6)
        .map(|idx| {
            make_product_doc(|doc| {
                set_titles(doc, "Porcelain Vase");
                doc.embedding = Some(one_hot_embedding(15 + idx, 1.0).into());
            })
        })
        .collect();

    let repository = ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    repository.create_product_documents(docs).await.unwrap();
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_secs(2)).await;

    let response = repository
        .hybrid_search_product_documents(
            &search_with_query("Porcelain Vase"),
            &one_hot_embedding(15, 1.0),
            &Some(Cursor {
                size: 3,
                search_after: None,
            }),
        )
        .await
        .unwrap();

    assert_eq!(3, response.hits.hits.len());
}

#[localstack_test(services = [OpenSearch()])]
async fn should_apply_state_filter_when_running_hybrid_search_for_product_search() {
    let available = make_product_doc(|doc| {
        set_titles(doc, "Bronze Statue");
        doc.state = ProductStateDocument::Available;
        doc.embedding = Some(one_hot_embedding(20, 1.0).into());
    });
    let sold = make_product_doc(|doc| {
        set_titles(doc, "Bronze Statue");
        doc.state = ProductStateDocument::Sold;
        doc.embedding = Some(one_hot_embedding(20, 1.0).into());
    });

    let repository = ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    repository
        .create_product_documents(vec![available.clone(), sold.clone()])
        .await
        .unwrap();
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_secs(2)).await;

    let mut search = search_with_query("Bronze Statue");
    search.state_query = AnyOfQuery::from(HashSet::from([ProductState::Available]));

    let response = repository
        .hybrid_search_product_documents(&search, &one_hot_embedding(20, 1.0), &None)
        .await
        .unwrap();

    let returned_ids: HashSet<_> = response
        .hits
        .hits
        .iter()
        .map(|hit| hit.source.product_id)
        .collect();
    assert!(returned_ids.contains(&available.product_id));
    assert!(!returned_ids.contains(&sold.product_id));
}

#[localstack_test(services = [OpenSearch()])]
async fn should_apply_price_filter_when_running_hybrid_search_for_product_search() {
    let emb = one_hot_embedding(250, 1.0);
    let in_range = make_product_doc(|doc| {
        set_titles(doc, "Silver Candlestick");
        doc.embedding = Some(emb.into());
        doc.price_eur = Some(50);
    });
    let out_of_range = make_product_doc(|doc| {
        set_titles(doc, "Silver Candlestick");
        doc.embedding = Some(emb.into());
        doc.price_eur = Some(500);
    });

    let repository = ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    repository
        .create_product_documents(vec![in_range.clone(), out_of_range.clone()])
        .await
        .unwrap();
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_secs(2)).await;

    let mut search = search_with_query("Silver Candlestick");
    search.price_query = Some(RangeQuery {
        min: Some(1u64.into()),
        max: Some(100u64.into()),
    });
    search.currency = Currency::Eur;

    let response = repository
        .hybrid_search_product_documents(&search, &emb, &None)
        .await
        .unwrap();

    let returned_ids: HashSet<_> = response
        .hits
        .hits
        .iter()
        .map(|hit| hit.source.product_id)
        .collect();
    assert!(returned_ids.contains(&in_range.product_id));
    assert!(!returned_ids.contains(&out_of_range.product_id));
}
