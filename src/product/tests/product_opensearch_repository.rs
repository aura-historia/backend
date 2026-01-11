use common::currency::domain::Currency;
use common::event_id::EventId;
use common::language::document::{LanguageDocument, TextDocument};
use common::language::domain::Language;
use common::pagination::cursor::Cursor;
use common::price::domain::MonetaryAmount;
use common::product_id::ProductId;
use common::product_state::domain::ProductState;
use common::query::any_of_query::AnyOfQuery;
use common::query::range_query::RangeQuery;
use common::shops_product_id::ShopsProductId;
use common::sort::{Sort, SortOrder};
use common::year::Year;
use fake::{Fake, Faker, rand};
use opensearch::http::Url;
use product::core::authenticity::Authenticity;
use product::core::condition::Condition;
use product::core::product_search::ProductSearch;
use product::core::provenance::Provenance;
use product::core::restoration::Restoration;
use product::core::sort_product_field::SortProductField;
use product::opensearch::product_document::ProductDocument;
use product::opensearch::product_state_document::ProductStateDocument;
use product::opensearch::product_update_document::ProductUpdateDocument;
use product::opensearch::repository::{
    ProductOpenSearchRepository, ProductOpenSearchRepositoryImpl,
};
use serde_json::json;
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
        event_id: Default::default(),
        shop_id: Default::default(),
        shops_product_id: ShopsProductId::from("abcdefgh"),
        shop_name: "Foo".to_string(),
        title_native: TextDocument {
            text: "Foo".to_string(),
            language: LanguageDocument::Fr,
        },
        title_de: Some("Bar".to_string()),
        title_en: Some("Baz".to_string()),
        title_fr: Some("Bat".to_string()),
        title_es: Some("Bao".to_string()),
        description_de: Some("Lorem ipsum dolor sit amet".to_string()),
        description_en: Some("Lorem ipsum dolor sit amet".to_string()),
        description_fr: Some("Lorem ipsum dolor sit amet".to_string()),
        description_es: Some("Lorem ipsum dolor sit amet".to_string()),
        price_eur: Some(99),
        price_usd: None,
        price_gbp: None,
        price_aud: None,
        price_cad: None,
        price_nzd: None,
        state: ProductStateDocument::Listed,
        url: Url::parse("https://foo.com/bar").unwrap(),
        images: Faker.fake(),
        text_embedding: None,
        origin_year_min: None,
        origin_year: None,
        origin_year_max: None,
        authenticity: None,
        condition: None,
        provenance: None,
        restoration: None,
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
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
        event_id: Default::default(),
        shop_id: Default::default(),
        shops_product_id: ShopsProductId::from("abcdefgh"),
        shop_name: "Foo".to_string(),
        title_native: TextDocument {
            text: "Foo".to_string(),
            language: LanguageDocument::Fr,
        },
        title_de: Some("Bar".to_string()),
        title_en: Some("Baz".to_string()),
        title_fr: Some("Bat".to_string()),
        title_es: Some("Bao".to_string()),
        description_de: Some("Lorem ipsum dolor sit amet".to_string()),
        description_en: Some("Lorem ipsum dolor sit amet".to_string()),
        description_fr: Some("Lorem ipsum dolor sit amet".to_string()),
        description_es: Some("Lorem ipsum dolor sit amet".to_string()),
        price_eur: Some(99),
        price_usd: None,
        price_gbp: None,
        price_aud: None,
        price_cad: None,
        price_nzd: None,
        state: ProductStateDocument::Listed,
        url: Url::parse("https://foo.com/bar").unwrap(),
        images: Faker.fake(),
        text_embedding: None,
        origin_year_min: None,
        origin_year: None,
        origin_year_max: None,
        authenticity: None,
        condition: None,
        provenance: None,
        restoration: None,
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
    };
    let product_id2 = ProductId::new();
    let expected2 = ProductDocument {
        product_id: product_id2,
        event_id: Default::default(),
        shop_id: Default::default(),
        shops_product_id: ShopsProductId::from("abcdefgh"),
        shop_name: "Foo".to_string(),
        title_native: TextDocument {
            text: "Foo".to_string(),
            language: LanguageDocument::Fr,
        },
        title_de: Some("Bar".to_string()),
        title_en: Some("Baz".to_string()),
        title_fr: Some("Bat".to_string()),
        title_es: Some("Bao".to_string()),
        description_de: Some("Lorem ipsum dolor sit amet".to_string()),
        description_en: Some("Lorem ipsum dolor sit amet".to_string()),
        description_fr: Some("Lorem ipsum dolor sit amet".to_string()),
        description_es: Some("Lorem ipsum dolor sit amet".to_string()),
        price_eur: Some(99),
        price_usd: None,
        price_gbp: None,
        price_aud: None,
        price_cad: None,
        price_nzd: None,
        state: ProductStateDocument::Listed,
        url: Url::parse("https://foo.com/bar").unwrap(),
        images: Faker.fake(),
        text_embedding: None,
        origin_year_min: None,
        origin_year: None,
        origin_year_max: None,
        authenticity: None,
        condition: None,
        provenance: None,
        restoration: None,
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
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
        event_id: Default::default(),
        shop_id: Default::default(),
        shops_product_id: ShopsProductId::from("abcdefgh"),
        shop_name: "Foo".to_string(),
        title_native: TextDocument {
            text: "Foo".to_string(),
            language: LanguageDocument::Fr,
        },
        title_de: Some("Bar".to_string()),
        title_en: Some("Baz".to_string()),
        title_fr: Some("Bat".to_string()),
        title_es: Some("Bao".to_string()),
        description_de: Some("Lorem ipsum dolor sit amet".to_string()),
        description_en: Some("Lorem ipsum dolor sit amet".to_string()),
        description_fr: Some("Lorem ipsum dolor sit amet".to_string()),
        description_es: Some("Lorem ipsum dolor sit amet".to_string()),
        price_eur: Some(99),
        price_usd: None,
        price_gbp: None,
        price_aud: None,
        price_cad: None,
        price_nzd: None,
        state: ProductStateDocument::Listed,
        url: Url::parse("https://foo.com/bar").unwrap(),
        images: Faker.fake(),
        text_embedding: None,
        origin_year_min: None,
        origin_year: None,
        origin_year_max: None,
        authenticity: None,
        condition: None,
        provenance: None,
        restoration: None,
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
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
        state: Some(ProductStateDocument::Sold),
        title_de: None,
        title_en: None,
        title_fr: None,
        title_es: None,
        description_de: None,
        description_en: None,
        description_fr: None,
        description_es: None,
        images: None,
        text_embedding: None,
        origin_year_min: None,
        origin_year: None,
        origin_year_max: None,
        authenticity: None,
        condition: None,
        provenance: None,
        restoration: None,
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
        event_id: Default::default(),
        shop_id: Default::default(),
        shops_product_id: ShopsProductId::from("abcdefgh"),
        shop_name: "Foo".to_string(),
        title_native: TextDocument {
            text: "Foo".to_string(),
            language: LanguageDocument::Fr,
        },
        title_de: Some("Hallo Welt".to_string()),
        title_en: Some("Baz".to_string()),
        title_fr: Some("Bat".to_string()),
        title_es: None,
        description_de: None,
        description_en: None,
        description_fr: None,
        description_es: None,
        price_eur: Some(99),
        price_usd: None,
        price_gbp: None,
        price_aud: None,
        price_cad: None,
        price_nzd: None,
        state: ProductStateDocument::Available,
        url: Url::parse("https://foo.com/bar").unwrap(),
        images: Faker.fake(),
        text_embedding: None,
        origin_year_min: None,
        origin_year: None,
        origin_year_max: None,
        authenticity: None,
        condition: None,
        provenance: None,
        restoration: None,
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
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
        product_query: "Hallo Welt".try_into().unwrap(),
        shop_name_query: None,
        price_query: None,
        state_query: Default::default(),
        origin_year_query: None,
        authenticity_query: Default::default(),
        condition_query: Default::default(),
        provenance_query: Default::default(),
        restoration_query: Default::default(),
        created_query: None,
        updated_query: None,
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
async fn should_omit_descriptions_in_response_for_search_product_documents() {
    let mut expected = ProductDocument {
        product_id: Default::default(),
        event_id: Default::default(),
        shop_id: Default::default(),
        shops_product_id: ShopsProductId::from("abcdefgh"),
        shop_name: "Foo".to_string(),
        title_native: TextDocument {
            text: "Foo".to_string(),
            language: LanguageDocument::Fr,
        },
        title_de: Some("Hallo Welt".to_string()),
        title_en: Some("Baz".to_string()),
        title_fr: Some("Bat".to_string()),
        title_es: None,
        description_de: Some("Hallo Welt".to_string()),
        description_en: Some("Baz".to_string()),
        description_fr: Some("Bat".to_string()),
        description_es: None,
        price_eur: Some(99),
        price_usd: None,
        price_gbp: None,
        price_aud: None,
        price_cad: None,
        price_nzd: None,
        state: ProductStateDocument::Available,
        url: Url::parse("https://foo.com/bar").unwrap(),
        images: Faker.fake(),
        text_embedding: None,
        origin_year_min: None,
        origin_year: None,
        origin_year_max: None,
        authenticity: None,
        condition: None,
        provenance: None,
        restoration: None,
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
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
        product_query: "Hallo Welt".try_into().unwrap(),
        shop_name_query: None,
        price_query: None,
        state_query: Default::default(),
        origin_year_query: None,
        authenticity_query: Default::default(),
        condition_query: Default::default(),
        provenance_query: Default::default(),
        restoration_query: Default::default(),
        created_query: None,
        updated_query: None,
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

    expected.description_de = None;
    expected.description_en = None;
    expected.description_fr = None;
    expected.description_es = None;

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
        product_query: "Lorem".try_into().unwrap(),
        shop_name_query: Some("LLC".try_into().unwrap()),
        price_query: Some(RangeQuery {
            min: Some(100u64.into()),
            max: Some(999999u64.into()),
        }),
        state_query: AnyOfQuery::from(HashSet::from_iter([
            ProductState::Available,
            ProductState::Listed,
        ])),
        origin_year_query: Some(RangeQuery {
            min: Some((-753).into()),
            max: Some(2100.into()),
        }),
        authenticity_query: AnyOfQuery::from(HashSet::from_iter([
            Authenticity::Questionable,
            Authenticity::Reproduction,
            Authenticity::LaterCopy,
        ])),
        condition_query: AnyOfQuery::from(HashSet::from_iter([Condition::Fair, Condition::Poor])),
        provenance_query: AnyOfQuery::from(HashSet::from_iter([
            Provenance::Unknown,
            Provenance::Partial,
        ])),
        restoration_query: AnyOfQuery::from(HashSet::from_iter([Restoration::None])),
        created_query: Some(RangeQuery {
            min: Some(datetime!(1000-01-01 0:00 UTC)),
            max: Some(datetime!(3000-01-01 0:00 UTC)),
        }),
        updated_query: Some(RangeQuery {
            min: Some(datetime!(1000-01-01 0:00 UTC)),
            max: Some(datetime!(3000-01-01 0:00 UTC)),
        }),
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
#[case(&[ProductState::Listed, ProductState::Available])]
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
        product_query: "The same title".try_into().unwrap(),
        shop_name_query: None,
        price_query: None,
        state_query: AnyOfQuery::from(HashSet::from_iter(states.iter().copied())),
        origin_year_query: None,
        authenticity_query: Default::default(),
        condition_query: Default::default(),
        provenance_query: Default::default(),
        restoration_query: Default::default(),
        created_query: None,
        updated_query: None,
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
        product_query: "The same title".try_into().unwrap(),
        shop_name_query: None,
        price_query: None,
        state_query: AnyOfQuery::from(HashSet::new()),
        origin_year_query: None,
        authenticity_query: Default::default(),
        condition_query: Default::default(),
        provenance_query: Default::default(),
        restoration_query: Default::default(),
        created_query: None,
        updated_query: None,
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
#[case(RangeQuery { min: Some(0u64.into()), max: Some(999999u64.into()) })]
#[case(RangeQuery { min: Some(0u64.into()), max: Some(999999u64.into()) })]
#[case(RangeQuery { min: Some(300u64.into()), max: Some(5000u64.into()) })]
#[case(RangeQuery { min: Some(500u64.into()), max: None })]
#[case(RangeQuery { min: None, max: Some(8888u64.into()) })]
#[case(RangeQuery { min: None, max: None })]
#[case(RangeQuery { min: None, max: None })]
#[localstack_test(services = [OpenSearch()])]
async fn should_search_product_documents_when_price_range_is_given(
    #[case] price_query: RangeQuery<MonetaryAmount>,
) {
    let cheap_products = fake::vec![ProductDocument; 50]
        .into_iter()
        .map(|mut product| {
            product.title_de = Some("The same title".into());
            product.price_eur = Some(rand::random_range(150..=1000));
            product.description_de = None;
            product.description_en = None;
            product.description_fr = None;
            product.description_es = None;
            product
        })
        .collect::<Vec<_>>();
    let expensive_products = fake::vec![ProductDocument; 50]
        .into_iter()
        .map(|mut product| {
            product.title_de = Some("The same title".into());
            product.price_eur = Some(rand::random_range(1500..=20000));
            product.description_de = None;
            product.description_en = None;
            product.description_fr = None;
            product.description_es = None;
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
        product_query: "The same title".try_into().unwrap(),
        shop_name_query: None,
        price_query: Some(price_query),
        state_query: Default::default(),
        origin_year_query: None,
        authenticity_query: Default::default(),
        condition_query: Default::default(),
        provenance_query: Default::default(),
        restoration_query: Default::default(),
        created_query: None,
        updated_query: None,
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
        .map(|mut product| {
            product.title_en = Some("The same title".into());
            product.price_usd = Some(rand::random_range(1500..=20000));
            product.description_de = None;
            product.description_en = None;
            product.description_fr = None;
            product.description_es = None;
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
        product_query: "The same title".try_into().unwrap(),
        shop_name_query: None,
        price_query: None,
        state_query: Default::default(),
        origin_year_query: None,
        authenticity_query: Default::default(),
        condition_query: Default::default(),
        provenance_query: Default::default(),
        restoration_query: Default::default(),
        created_query: None,
        updated_query: None,
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
        .map(|mut product| {
            product.title_en = Some("The same title".into());
            product.price_usd = Some(rand::random_range(1500..=20000));
            product.description_de = None;
            product.description_en = None;
            product.description_fr = None;
            product.description_es = None;
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
        product_query: "The same title".try_into().unwrap(),
        shop_name_query: None,
        price_query: None,
        state_query: Default::default(),
        origin_year_query: None,
        authenticity_query: Default::default(),
        condition_query: Default::default(),
        provenance_query: Default::default(),
        restoration_query: Default::default(),
        created_query: None,
        updated_query: None,
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
async fn should_get_product_document() {
    let product_id = ProductId::new();
    let expected = ProductDocument {
        product_id,
        event_id: Default::default(),
        shop_id: Default::default(),
        shops_product_id: ShopsProductId::from("abcdefgh"),
        shop_name: "Foo".to_string(),
        title_native: TextDocument {
            text: "Foo".to_string(),
            language: LanguageDocument::Fr,
        },
        title_de: Some("Bar".to_string()),
        title_en: Some("Baz".to_string()),
        title_fr: Some("Bat".to_string()),
        title_es: Some("Bao".to_string()),
        description_de: Some("Lorem ipsum dolor sit amet".to_string()),
        description_en: Some("Lorem ipsum dolor sit amet".to_string()),
        description_fr: Some("Lorem ipsum dolor sit amet".to_string()),
        description_es: Some("Lorem ipsum dolor sit amet".to_string()),
        price_eur: Some(99),
        price_usd: None,
        price_gbp: None,
        price_aud: None,
        price_cad: None,
        price_nzd: None,
        state: ProductStateDocument::Listed,
        url: Url::parse("https://foo.com/bar").unwrap(),
        images: Faker.fake(),
        text_embedding: None,
        origin_year_min: None,
        origin_year: None,
        origin_year_max: None,
        authenticity: None,
        condition: None,
        provenance: None,
        restoration: None,
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
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

const EXAMPLE_EMBEDDING: [f32; 1024] = [
    0.0003272566,
    0.057399165,
    -0.03456967,
    -0.0106262015,
    -0.014141742,
    0.010457292,
    0.04659525,
    0.012118102,
    -0.01775892,
    -0.0030063824,
    -0.0026424518,
    0.041838173,
    0.017255038,
    -0.05011607,
    0.03330435,
    -0.043115288,
    0.024365269,
    0.008319518,
    -0.0010633086,
    -0.029032322,
    -0.03335746,
    0.003915449,
    0.0026979458,
    0.006955503,
    -0.010099175,
    0.026191471,
    -0.041106544,
    -0.0023986483,
    -0.022852676,
    -0.02046144,
    -0.002146331,
    0.01218532,
    -0.0072695855,
    -0.030294996,
    -0.045752205,
    0.032810956,
    -0.0067101414,
    -0.027104286,
    -0.064636745,
    0.004402361,
    0.043752763,
    0.085650235,
    -0.015107115,
    0.022542398,
    -0.012031321,
    -0.016969888,
    0.0076060123,
    -0.0017969686,
    0.0015017944,
    -0.031243084,
    0.021503126,
    -0.031438302,
    0.024717737,
    -0.0144598475,
    0.033105273,
    0.06400776,
    0.0064835474,
    0.020815318,
    -0.035197426,
    0.008032621,
    -0.033077635,
    -0.0075795515,
    0.0020131955,
    -0.04924523,
    -0.015111905,
    0.09664927,
    0.039005585,
    0.018401628,
    -0.008501611,
    -0.04901847,
    -0.018197248,
    0.010896852,
    0.025534352,
    0.007962412,
    -0.06537132,
    0.047222693,
    0.047002513,
    -0.032187644,
    0.061376147,
    0.01863283,
    0.08137537,
    -0.024960617,
    -0.012657353,
    0.0015068561,
    -0.041729964,
    0.048218288,
    -0.017299946,
    0.032438353,
    0.018043075,
    0.022542244,
    0.033174258,
    -0.053141434,
    -0.0022261054,
    0.0031429217,
    -0.038417198,
    0.026048293,
    -0.008870415,
    0.030812439,
    0.03319375,
    0.011411405,
    0.040851586,
    0.046423644,
    -0.043405082,
    -0.04912621,
    0.031803377,
    -0.016694633,
    0.033403166,
    0.030523121,
    0.011753323,
    0.023821648,
    0.05829599,
    0.017138349,
    0.026108429,
    0.02419801,
    -0.0020035687,
    -0.010595497,
    -0.02986105,
    -0.012447884,
    0.053139225,
    -0.0010849425,
    -0.0024957422,
    0.023570115,
    0.01298907,
    -0.04547553,
    0.0389707,
    -0.046143718,
    -0.019500313,
    0.0116330525,
    -0.00965437,
    0.038469248,
    0.048517454,
    0.02178535,
    -0.052851528,
    -0.02059403,
    0.0143086715,
    -0.045374274,
    0.047701433,
    -0.016601518,
    0.037074994,
    -0.04640927,
    -0.0002305248,
    0.033905182,
    0.039176434,
    -0.028918877,
    0.001103291,
    -0.061383422,
    0.013512757,
    -0.00791641,
    -0.029770156,
    -0.024654103,
    -0.015810521,
    0.0084724855,
    -0.033417154,
    -0.030438572,
    0.010559214,
    -0.0012923476,
    0.022190606,
    0.060173254,
    -0.0071199643,
    0.009328146,
    0.05591182,
    0.0048989854,
    -0.00032818865,
    -0.0060883774,
    -0.00023993742,
    0.036252055,
    0.015170894,
    -0.010240388,
    -0.010290683,
    -0.011864539,
    -0.009298821,
    -0.051631484,
    0.004371834,
    0.0011985721,
    0.027394142,
    0.018983513,
    0.047578435,
    0.0006170196,
    0.022756878,
    -0.047732078,
    0.0074887024,
    -0.04091164,
    0.007259588,
    -0.004065235,
    -0.031365845,
    0.013086743,
    -0.012649365,
    0.028764397,
    -0.0018775607,
    0.021444034,
    -0.018342517,
    0.012937729,
    0.015516735,
    -0.013029383,
    0.015664442,
    -0.024555063,
    0.00019338762,
    0.0017034123,
    0.0004926277,
    0.009059934,
    -0.014520103,
    0.05243122,
    0.014932085,
    -0.0039938837,
    0.019125547,
    -0.008614347,
    -0.06729333,
    -0.013556678,
    0.05028874,
    -0.0018153647,
    0.015557867,
    -0.0030649423,
    -0.0026829718,
    0.018642286,
    -0.038199183,
    -0.03294994,
    -0.014883622,
    0.014779545,
    0.02140016,
    0.009804876,
    0.004958428,
    0.019226104,
    0.025099836,
    0.046242405,
    0.0008196466,
    0.018209888,
    -0.02076707,
    0.059340294,
    -0.031871144,
    -0.037058495,
    -0.0046318094,
    -0.012318178,
    0.011814306,
    0.0041106166,
    -0.016442508,
    0.002910965,
    -0.010647634,
    -0.008500043,
    0.013334221,
    0.020931307,
    -0.014139455,
    -0.030637203,
    0.004125956,
    0.0312838,
    -0.039864857,
    0.030869365,
    -0.016873274,
    0.0056212116,
    -0.013738663,
    0.004012051,
    -0.038413186,
    -0.028748166,
    -0.024072843,
    0.057576973,
    0.017201373,
    0.028801078,
    -0.009352578,
    0.005576139,
    0.010144287,
    -0.05617081,
    0.026736649,
    -0.057129078,
    -0.037356164,
    0.04270804,
    -0.022015018,
    0.025703205,
    0.016018357,
    0.004235701,
    -0.001066849,
    -0.0133604165,
    0.0039634574,
    -0.0009934092,
    -0.04011141,
    -0.009605451,
    -0.042391464,
    0.029926252,
    -0.0022060736,
    -0.06582467,
    0.03539945,
    0.031970825,
    -0.015887093,
    -0.010586142,
    0.0025160008,
    0.027151367,
    0.015396707,
    0.020803122,
    -0.012347851,
    0.041142147,
    0.01460739,
    -0.027189141,
    -0.0084227305,
    0.03268739,
    0.03432998,
    -0.050671257,
    -0.006849337,
    0.05580775,
    -0.029546585,
    -0.19109386,
    0.008132767,
    -0.00625366,
    0.008462262,
    -0.005741844,
    0.027879208,
    -0.04825245,
    0.0048290244,
    0.0030262228,
    -0.012869358,
    -0.010487197,
    -0.033437826,
    0.00086632045,
    -0.0031849043,
    0.054632913,
    0.012125366,
    -0.0034956357,
    -0.023784228,
    0.0045979237,
    -0.06838102,
    0.0066340277,
    0.008821881,
    -0.017112399,
    -0.06651932,
    0.016837852,
    0.016893044,
    0.014203568,
    -0.010174751,
    -0.029387718,
    -0.011306487,
    -0.027990853,
    -0.0028507991,
    0.012847916,
    0.030015633,
    0.061893035,
    0.040559474,
    0.06450448,
    0.008577098,
    0.01361189,
    0.01301374,
    0.017445505,
    0.063280314,
    -0.024008118,
    -0.0410387,
    0.009988834,
    -0.004833229,
    0.0031237896,
    0.012673825,
    -0.032089576,
    -0.020773202,
    -0.018866468,
    -0.0030336387,
    -0.037033644,
    0.02092163,
    0.002071078,
    -0.015567679,
    -0.033961352,
    0.032231517,
    -0.037392493,
    -0.020856244,
    -0.030775473,
    -0.03454945,
    0.004895689,
    0.016605146,
    -0.055688687,
    0.013458171,
    -0.020007674,
    -0.028545652,
    -0.008191386,
    -0.011002774,
    0.050427735,
    -0.008550305,
    0.0118111,
    -0.005803428,
    -0.026859796,
    -0.011692541,
    0.021300903,
    -0.028170336,
    -0.017763572,
    -0.13710505,
    0.004965118,
    -0.012338429,
    -0.009626636,
    0.033704028,
    0.007601361,
    -0.044706993,
    0.063490316,
    0.015604505,
    0.031396233,
    0.24593687,
    -0.034070414,
    0.023450267,
    -0.030969962,
    0.038910042,
    -0.023677358,
    0.0071090786,
    -0.011207256,
    0.029248567,
    -0.04609916,
    -0.022783192,
    -0.014655579,
    0.0013965754,
    -0.0036873475,
    -0.019272102,
    0.011954131,
    -0.040581945,
    0.010395461,
    0.070001654,
    0.028521886,
    0.020681182,
    -0.010727249,
    0.024728553,
    -0.0018973184,
    -0.016035778,
    -0.04022159,
    0.015369633,
    0.053623963,
    -0.0032370207,
    -0.0068921903,
    -0.0074646845,
    -0.045909774,
    0.024136009,
    -0.012132545,
    -0.02143451,
    -0.009162377,
    -0.010898247,
    -0.031385545,
    0.011661473,
    -0.012991721,
    -0.010576877,
    0.011779889,
    0.006928308,
    0.025649205,
    0.0028401532,
    0.015434813,
    -0.031618256,
    -0.020008171,
    -0.035858158,
    0.0009007813,
    -0.010263004,
    -0.02045078,
    -0.060780726,
    0.02870223,
    -0.059399962,
    -0.02819086,
    -0.028941907,
    0.0014574742,
    0.018966153,
    0.059438027,
    0.00813851,
    0.041569088,
    -0.04852137,
    -0.025426703,
    -0.04566685,
    0.0013227283,
    -0.0135409115,
    -0.021306759,
    -0.016258981,
    0.01099489,
    0.011348335,
    -0.029114893,
    0.00058327557,
    0.026428098,
    -0.0037051656,
    0.012885822,
    -0.029917996,
    0.030765334,
    -0.005484935,
    0.0053331107,
    -0.025947286,
    -0.039691433,
    -0.014631929,
    -0.009714047,
    0.014868744,
    -0.013864954,
    -0.030055424,
    0.01786473,
    -0.0092636915,
    -0.0109823365,
    0.056882,
    0.009323296,
    0.0037069088,
    0.004796603,
    -0.0048888833,
    0.0054285945,
    0.043755546,
    0.024507822,
    0.025022179,
    0.027026204,
    -0.08134872,
    -0.012025706,
    0.02460811,
    -0.013556706,
    0.026682822,
    -0.011773854,
    0.016998423,
    -9.3735e-5,
    -0.032791283,
    -0.009831742,
    0.053448338,
    -0.004855143,
    0.0069636162,
    0.020332327,
    0.039362658,
    0.036531907,
    -0.006381021,
    -5.527525e-6,
    -0.01604043,
    0.06029084,
    -0.05366821,
    -0.024639117,
    -0.0060600154,
    -0.008861102,
    -0.0045871404,
    0.008669352,
    -0.06810332,
    0.0018733272,
    0.018493325,
    0.017002486,
    -0.029507855,
    -0.037704434,
    -0.01631373,
    0.08775386,
    0.04600553,
    -0.04080889,
    0.07545939,
    0.019134983,
    -0.032352936,
    0.058893166,
    -0.02953855,
    -0.03984061,
    -0.012755565,
    0.0014477421,
    -0.029224813,
    0.054907944,
    -0.0789144,
    0.002413634,
    -0.0051396578,
    0.051368546,
    -0.007456196,
    -0.0057195937,
    0.052404836,
    -0.05682206,
    -0.030991841,
    0.006827349,
    0.003521702,
    0.017826024,
    -0.020567209,
    -0.027690174,
    0.01883157,
    -0.0074440874,
    0.053265754,
    0.09342776,
    0.027881276,
    0.029499996,
    -0.015187565,
    0.05059695,
    -0.013954103,
    -0.03284258,
    -0.004100567,
    -0.036653206,
    -0.024409015,
    -0.019542146,
    -0.011304147,
    -0.004688139,
    -0.057332404,
    -0.0027535206,
    -0.02539958,
    0.025160607,
    0.038703024,
    -0.02674856,
    -0.017489722,
    -0.002494743,
    0.008934229,
    0.048612032,
    0.0049296618,
    -0.0064484146,
    0.042560503,
    -0.0066472767,
    -0.0013230841,
    0.07318776,
    0.002059235,
    -0.010504023,
    0.020186918,
    0.022652715,
    0.028194541,
    0.022320177,
    0.02590463,
    -0.007175373,
    -0.007648733,
    -0.036022216,
    -0.0031242715,
    -0.009156579,
    -0.010659548,
    0.008049303,
    0.008840813,
    0.02352207,
    0.0017198211,
    0.003525938,
    -0.017763577,
    -0.02255104,
    0.0054182066,
    0.0027917984,
    -0.030119449,
    0.015834024,
    0.015099323,
    0.0032004844,
    0.0024566595,
    -0.050682098,
    -0.0022582116,
    -0.0037904717,
    0.045005098,
    -0.011423952,
    0.0067611965,
    -0.030309727,
    0.019692667,
    0.032845058,
    -0.0090010865,
    -0.01480977,
    0.0005478675,
    0.008241499,
    -0.018594833,
    0.020048302,
    -0.003415002,
    0.022371223,
    -0.044811677,
    0.014281272,
    0.014886089,
    -0.026090553,
    0.002907364,
    0.01371469,
    0.0092705805,
    0.04732476,
    -0.012872408,
    0.05785681,
    -0.02855162,
    -0.024949966,
    -0.0375568,
    0.0020091098,
    -0.037340682,
    -0.009061861,
    0.03339302,
    -0.025103046,
    0.046012443,
    -0.020558462,
    0.028964512,
    -0.006917054,
    -0.0770982,
    0.01828087,
    -0.024794715,
    0.01697373,
    -0.025829177,
    -0.034757238,
    -0.03368985,
    -0.03379701,
    0.040056404,
    0.004607489,
    -0.0218689,
    -0.050506763,
    0.014846354,
    -0.020619864,
    -0.02638047,
    -0.010243197,
    -0.019768784,
    0.0037510414,
    -0.0075338874,
    0.01765253,
    -0.02485942,
    0.011276767,
    -0.022816496,
    0.0045660967,
    -0.018123796,
    0.04748066,
    -0.0072258823,
    0.039989892,
    -0.022220366,
    0.014220618,
    0.009477211,
    -0.039026357,
    0.0062931096,
    -0.018500213,
    -0.060098987,
    -0.011408045,
    -0.024998842,
    -0.0058286777,
    -0.0195909,
    0.020962415,
    -0.03363868,
    0.027674908,
    -0.022782326,
    0.00011843879,
    -0.004896305,
    -0.0036817114,
    0.028065119,
    0.011832172,
    0.0045515657,
    -0.033082232,
    0.07098138,
    0.0034055999,
    0.022488177,
    -0.059109736,
    -0.006545849,
    0.01567799,
    0.045923635,
    -0.040564496,
    0.020701755,
    -0.013943637,
    0.010593306,
    0.03258394,
    0.022522068,
    -0.0010788155,
    0.0076529128,
    0.05484559,
    -0.010061054,
    0.009654935,
    -0.0022583394,
    0.05352336,
    -0.011079317,
    0.0035695934,
    -0.028402412,
    -0.006438774,
    -0.06544066,
    0.0015482869,
    -0.02509151,
    0.0032823705,
    0.07090053,
    0.0056362124,
    0.039916538,
    -0.025373423,
    -0.041575126,
    0.060639337,
    0.0029663588,
    -0.012050132,
    -0.027709965,
    -0.02914558,
    0.019477332,
    0.011386469,
    0.011246975,
    -0.036803737,
    -5.930105e-5,
    0.054610252,
    -0.0067799045,
    -0.014393941,
    0.022297248,
    0.0032388605,
    -0.013143182,
    0.037334643,
    0.02809063,
    0.0024890795,
    -0.0098310355,
    0.022139102,
    -0.000509493,
    0.026036005,
    -0.01182871,
    0.0012622843,
    -0.03270492,
    0.017757278,
    -0.035536855,
    -0.072909854,
    -0.03651895,
    0.0040604817,
    -0.016163552,
    0.017194662,
    0.02697014,
    0.042724665,
    0.023468137,
    0.019033179,
    0.043340992,
    -0.016657673,
    -0.002251577,
    -0.01508343,
    -0.02167459,
    -0.051993024,
    -0.0059517934,
    -0.06468132,
    -0.057369076,
    -0.005457933,
    0.040747315,
    0.013892949,
    -0.054217882,
    -0.0067164046,
    -0.018244999,
    0.016202413,
    -0.17906684,
    0.0077831107,
    -0.016322823,
    -0.022605948,
    -0.0341306,
    -1.4258145e-5,
    -0.024862673,
    -0.026506605,
    -0.018937126,
    -0.0015299825,
    -0.02946169,
    0.015510216,
    -0.029751161,
    -0.03022711,
    -0.036712512,
    0.031198155,
    0.013417892,
    -0.008315913,
    0.008630413,
    0.05939868,
    0.004666387,
    -0.03700047,
    0.01061426,
    -0.04444638,
    0.0011062768,
    -0.01687693,
    -0.013345257,
    0.011018251,
    -0.045670312,
    -0.055196635,
    0.02702817,
    -0.0012713602,
    0.016427027,
    0.065190285,
    -0.05031679,
    0.0354162,
    0.0111456355,
    0.019916313,
    0.01477492,
    0.006546737,
    0.005281983,
    0.019113593,
    -0.030623721,
    -0.007362806,
    -0.0048263073,
    -0.00022648936,
    -0.012456244,
    -0.026633704,
    -0.059150986,
    -0.024084214,
    0.03341897,
    -0.004672881,
    -0.0032702186,
    0.02231938,
    0.026500896,
    0.0231716,
    -0.040338017,
    0.015829084,
    0.00064458227,
    0.03079693,
    -0.0073098433,
    0.004672375,
    -0.04269056,
    -0.034884535,
    -0.03460636,
    0.042699225,
    -0.070788555,
    -0.0042571486,
    -0.011707434,
    0.026174184,
    -0.040012766,
    -0.014723488,
    0.03398638,
    -0.02656347,
    0.055813666,
    0.0022373649,
    0.027270485,
    -0.008574889,
    -0.047244847,
    -0.020595375,
    -0.0024405264,
    -0.015287482,
    -0.034606777,
    0.014166236,
    0.014883017,
    -0.024907274,
    0.0036789668,
    0.04588317,
    -0.025727632,
    0.007474228,
    -0.041889444,
    -0.04239232,
    -0.031765148,
    0.016402654,
    -0.032313958,
    -0.042583466,
    0.003694188,
    -0.03789163,
    0.005762771,
    -0.014236267,
    0.026926348,
    0.0145796435,
    -0.046388373,
    0.010923083,
    0.004996239,
    0.062479153,
    0.025659053,
    -0.02194447,
    0.005160734,
    -0.025858726,
    -0.03657522,
    0.011269099,
    0.020505859,
    0.02050172,
    0.028689394,
    -0.032361012,
    0.026581394,
    -0.006538726,
    -0.02164772,
    -0.02820093,
    7.197924e-6,
    0.006236892,
    0.035982244,
    -0.029548632,
    0.059329294,
    -0.019903114,
    0.030476535,
    0.0012749213,
    -0.0067717233,
    -0.057012323,
    0.05123047,
    -0.022876687,
    0.007296464,
    -0.0058410163,
    -0.012961809,
    -0.022470405,
    0.022805417,
    0.027031465,
    -0.047690865,
    -0.0377045,
    0.033635926,
    -0.037884004,
    -0.036368813,
    -0.008691378,
    -0.011877837,
    0.027587203,
    0.03739567,
    -0.010263341,
    -0.016878022,
    -0.017726379,
    -0.0028035117,
    0.016174102,
    0.007416928,
    0.016449932,
    -0.044825092,
    0.028005298,
    0.0075571584,
    -0.0045754467,
    0.02552638,
    -0.017322907,
    -0.054073393,
    -0.0022051185,
    0.016951907,
    -0.00097456406,
    -0.0057038623,
    0.005191519,
    0.009454499,
    -0.017367927,
    -0.031111648,
    -0.017883712,
    0.0061059613,
    0.03894756,
    -0.014612449,
    0.021154026,
    0.041503686,
    -0.00025324986,
    0.041500125,
    -0.01261671,
    0.035225008,
    -0.021307211,
    0.004774336,
    0.009435292,
    -0.0037574295,
    0.027970085,
    -0.010632901,
    0.020761402,
    0.028760817,
    0.0014389881,
    0.047640633,
    0.012061617,
    0.025732249,
    0.0034775035,
    0.017368317,
    -0.0110013895,
    0.048862655,
    -0.00082114513,
    -0.021151956,
    -0.0035007775,
    -0.047433738,
    0.027765855,
    -0.035673257,
    -0.015826378,
    0.015195975,
    -0.03630748,
    0.017007241,
    0.029029569,
    -0.033839382,
    -0.00847942,
    -0.03248065,
    0.066325404,
    -0.031397443,
    0.011676608,
    -0.008554638,
    0.008910565,
    -0.028092973,
    0.006312944,
    -0.0009780206,
    0.019365432,
    0.028579503,
    0.049160477,
    0.020700263,
    0.0059294514,
    -0.0036679148,
    0.00886464,
    0.027618295,
    0.0013910793,
    -0.037796766,
    0.030503033,
    0.0006737808,
    -0.017314281,
    0.025290234,
    -0.05493075,
    -0.026802655,
    0.035179928,
    -0.0026953951,
    -0.049771644,
    0.02076164,
    -0.007511784,
    -0.0048586307,
    -0.051234838,
    0.036292616,
    0.028930582,
    -0.017055722,
    0.016079217,
];
#[localstack_test(services = [OpenSearch()])]
async fn should_return_k_nearest_neighbors() {
    let client = get_opensearch_client().await;
    let repository = ProductOpenSearchRepositoryImpl::new(client);
    let mut documents = fake::vec![ProductDocument; 20];
    for document in &mut documents {
        document.text_embedding = Some(EXAMPLE_EMBEDDING.into());
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

#[localstack_test(services = [OpenSearch()])]
async fn should_search_product_documents_when_exact_year_is_given_for_stored_exact_year() {
    let civilians = fake::vec![ProductDocument; 50]
        .into_iter()
        .map(|mut product| {
            let year = rand::random_range(1830..=1835).into();
            product.title_de = Some("The same title".into());
            product.origin_year = Some(year);
            product.origin_year_min = Some(year);
            product.origin_year_max = Some(year);
            product.description_de = None;
            product.description_en = None;
            product.description_fr = None;
            product.description_es = None;
            product
        })
        .collect::<Vec<_>>();
    let expected = fake::vec![ProductDocument; 42]
        .into_iter()
        .map(|mut product| {
            let year = 1836.into();
            product.title_de = Some("The same title".into());
            product.origin_year = Some(year);
            product.origin_year_min = Some(year);
            product.origin_year_max = Some(year);
            product.description_de = None;
            product.description_en = None;
            product.description_fr = None;
            product.description_es = None;
            product
        })
        .collect::<Vec<_>>();
    let products = [civilians, expected.clone()].concat();
    let client = get_opensearch_client().await;
    let repository = ProductOpenSearchRepositoryImpl::new(client);
    let response = repository
        .create_product_documents(products.clone())
        .await
        .unwrap();
    assert!(!response.errors);
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_millis(5000)).await;

    let search_filter = ProductSearch {
        language: Language::De,
        currency: Currency::Eur,
        product_query: "The same title".try_into().unwrap(),
        shop_name_query: None,
        price_query: None,
        state_query: Default::default(),
        origin_year_query: Some(RangeQuery {
            min: Some(1836.into()),
            max: Some(1836.into()),
        }),
        authenticity_query: Default::default(),
        condition_query: Default::default(),
        provenance_query: Default::default(),
        restoration_query: Default::default(),
        created_query: None,
        updated_query: None,
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
    let actual_products = response
        .hits
        .hits
        .into_iter()
        .map(|hit| hit.source)
        .collect::<Vec<_>>();

    assert_eq!(expected.len(), actual_products.len());
    assert!(
        actual_products
            .iter()
            .all(|product| product.origin_year.unwrap() == Year::from(1836))
    );
}

#[localstack_test(services = [OpenSearch()])]
async fn should_search_product_documents_when_only_min_year_is_given_for_stored_exact_year() {
    let civilians = fake::vec![ProductDocument; 50]
        .into_iter()
        .map(|mut product| {
            let year = rand::random_range(1830..=1835).into();
            product.title_de = Some("The same title".into());
            product.origin_year = Some(year);
            product.origin_year_min = Some(year);
            product.origin_year_max = Some(year);
            product.description_de = None;
            product.description_en = None;
            product.description_fr = None;
            product.description_es = None;
            product
        })
        .collect::<Vec<_>>();
    let expected = fake::vec![ProductDocument; 42]
        .into_iter()
        .map(|mut product| {
            let year = 1836.into();
            product.title_de = Some("The same title".into());
            product.origin_year = Some(year);
            product.origin_year_min = Some(year);
            product.origin_year_max = Some(year);
            product.description_de = None;
            product.description_en = None;
            product.description_fr = None;
            product.description_es = None;
            product
        })
        .collect::<Vec<_>>();
    let products = [civilians, expected.clone()].concat();
    let client = get_opensearch_client().await;
    let repository = ProductOpenSearchRepositoryImpl::new(client);
    let response = repository
        .create_product_documents(products.clone())
        .await
        .unwrap();
    assert!(!response.errors);
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_millis(5000)).await;

    let search_filter = ProductSearch {
        language: Language::De,
        currency: Currency::Eur,
        product_query: "The same title".try_into().unwrap(),
        shop_name_query: None,
        price_query: None,
        state_query: Default::default(),
        origin_year_query: Some(RangeQuery {
            min: Some(1836.into()),
            max: None,
        }),
        authenticity_query: Default::default(),
        condition_query: Default::default(),
        provenance_query: Default::default(),
        restoration_query: Default::default(),
        created_query: None,
        updated_query: None,
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
    let actual_products = response
        .hits
        .hits
        .into_iter()
        .map(|hit| hit.source)
        .collect::<Vec<_>>();

    assert_eq!(expected.len(), actual_products.len());
    assert!(
        actual_products
            .iter()
            .all(|product| product.origin_year.unwrap() >= Year::from(1836))
    );
}

#[localstack_test(services = [OpenSearch()])]
async fn should_search_product_documents_when_only_max_year_is_given_for_stored_exact_year() {
    let civilians = fake::vec![ProductDocument; 50]
        .into_iter()
        .map(|mut product| {
            let year = rand::random_range(1836..=1840).into();
            product.title_de = Some("The same title".into());
            product.origin_year = Some(year);
            product.origin_year_min = Some(year);
            product.origin_year_max = Some(year);
            product.description_de = None;
            product.description_en = None;
            product.description_fr = None;
            product.description_es = None;
            product
        })
        .collect::<Vec<_>>();
    let expected = fake::vec![ProductDocument; 42]
        .into_iter()
        .map(|mut product| {
            let year = rand::random_range(1830..=1835).into();
            product.title_de = Some("The same title".into());
            product.origin_year = Some(year);
            product.origin_year_min = Some(year);
            product.origin_year_max = Some(year);
            product.description_de = None;
            product.description_en = None;
            product.description_fr = None;
            product.description_es = None;
            product
        })
        .collect::<Vec<_>>();
    let products = [civilians, expected.clone()].concat();
    let client = get_opensearch_client().await;
    let repository = ProductOpenSearchRepositoryImpl::new(client);
    let response = repository
        .create_product_documents(products.clone())
        .await
        .unwrap();
    assert!(!response.errors);
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_millis(5000)).await;

    let search_filter = ProductSearch {
        language: Language::De,
        currency: Currency::Eur,
        product_query: "The same title".try_into().unwrap(),
        shop_name_query: None,
        price_query: None,
        state_query: Default::default(),
        origin_year_query: Some(RangeQuery {
            min: None,
            max: Some(1835.into()),
        }),
        authenticity_query: Default::default(),
        condition_query: Default::default(),
        provenance_query: Default::default(),
        restoration_query: Default::default(),
        created_query: None,
        updated_query: None,
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
    let actual_products = response
        .hits
        .hits
        .into_iter()
        .map(|hit| hit.source)
        .collect::<Vec<_>>();

    assert_eq!(expected.len(), actual_products.len());
    assert!(
        actual_products
            .iter()
            .all(|product| product.origin_year.unwrap() <= Year::from(1835))
    );
}

#[localstack_test(services = [OpenSearch()])]
async fn should_search_product_documents_when_min_and_max_year_is_given_for_stored_exact_year() {
    let civilians = fake::vec![ProductDocument; 50]
        .into_iter()
        .map(|mut product| {
            let year = rand::random_range(1830..=1833).into();
            product.title_de = Some("The same title".into());
            product.origin_year = Some(year);
            product.origin_year_min = Some(year);
            product.origin_year_max = Some(year);
            product.description_de = None;
            product.description_en = None;
            product.description_fr = None;
            product.description_es = None;
            product
        })
        .collect::<Vec<_>>();
    let expected = fake::vec![ProductDocument; 42]
        .into_iter()
        .map(|mut product| {
            let year = rand::random_range(1836..=1840).into();
            product.title_de = Some("The same title".into());
            product.origin_year = Some(year);
            product.origin_year_min = Some(year);
            product.origin_year_max = Some(year);
            product.description_de = None;
            product.description_en = None;
            product.description_fr = None;
            product.description_es = None;
            product
        })
        .collect::<Vec<_>>();
    let products = [civilians, expected.clone()].concat();
    let client = get_opensearch_client().await;
    let repository = ProductOpenSearchRepositoryImpl::new(client);
    let response = repository
        .create_product_documents(products.clone())
        .await
        .unwrap();
    assert!(!response.errors);
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_millis(5000)).await;

    let search_filter = ProductSearch {
        language: Language::De,
        currency: Currency::Eur,
        product_query: "The same title".try_into().unwrap(),
        shop_name_query: None,
        price_query: None,
        state_query: Default::default(),
        origin_year_query: Some(RangeQuery {
            min: Some(1834.into()),
            max: Some(1843.into()),
        }),
        authenticity_query: Default::default(),
        condition_query: Default::default(),
        provenance_query: Default::default(),
        restoration_query: Default::default(),
        created_query: None,
        updated_query: None,
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
    let actual_products = response
        .hits
        .hits
        .into_iter()
        .map(|hit| hit.source)
        .collect::<Vec<_>>();

    assert_eq!(expected.len(), actual_products.len());
    assert!(
        actual_products
            .iter()
            .all(|product| product.origin_year.unwrap() >= Year::from(1834)
                && product.origin_year.unwrap() <= Year::from(1843))
    );
}

// ------------------------
#[localstack_test(services = [OpenSearch()])]
async fn should_search_product_documents_when_only_min_year_is_given_for_stored_year_range() {
    let civilians = fake::vec![ProductDocument; 50]
        .into_iter()
        .map(|mut product| {
            let year1 = rand::random_range(1830..=1832).into();
            let year2 = rand::random_range(1833..=1835).into();
            product.title_de = Some("The same title".into());
            product.origin_year_min = Some(year1);
            product.origin_year_max = Some(year2);
            product.description_de = None;
            product.description_en = None;
            product.description_fr = None;
            product.description_es = None;
            product
        })
        .collect::<Vec<_>>();
    let expected = fake::vec![ProductDocument; 42]
        .into_iter()
        .map(|mut product| {
            let year1 = rand::random_range(1836..=1840).into();
            let year2 = rand::random_range(1841..=1847).into();
            product.title_de = Some("The same title".into());
            product.origin_year_min = Some(year1);
            product.origin_year_max = Some(year2);
            product.description_de = None;
            product.description_en = None;
            product.description_fr = None;
            product.description_es = None;
            product
        })
        .collect::<Vec<_>>();
    let products = [civilians, expected.clone()].concat();
    let client = get_opensearch_client().await;
    let repository = ProductOpenSearchRepositoryImpl::new(client);
    let response = repository
        .create_product_documents(products.clone())
        .await
        .unwrap();
    assert!(!response.errors);
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_millis(5000)).await;

    let search_filter = ProductSearch {
        language: Language::De,
        currency: Currency::Eur,
        product_query: "The same title".try_into().unwrap(),
        shop_name_query: None,
        price_query: None,
        state_query: Default::default(),
        origin_year_query: Some(RangeQuery {
            min: Some(1836.into()),
            max: None,
        }),
        authenticity_query: Default::default(),
        condition_query: Default::default(),
        provenance_query: Default::default(),
        restoration_query: Default::default(),
        created_query: None,
        updated_query: None,
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
    let actual_products = response
        .hits
        .hits
        .into_iter()
        .map(|hit| hit.source)
        .collect::<Vec<_>>();

    assert_eq!(expected.len(), actual_products.len());
    assert!(
        actual_products
            .iter()
            .all(|product| product.origin_year_min.unwrap() >= Year::from(1836))
    );
}

#[localstack_test(services = [OpenSearch()])]
async fn should_search_product_documents_when_only_max_year_is_given_for_stored_year_range() {
    let civilians = fake::vec![ProductDocument; 50]
        .into_iter()
        .map(|mut product| {
            let year1 = rand::random_range(1836..=1837).into();
            let year2 = rand::random_range(1838..=1840).into();
            product.title_de = Some("The same title".into());
            product.origin_year_min = Some(year1);
            product.origin_year_max = Some(year2);
            product.description_de = None;
            product.description_en = None;
            product.description_fr = None;
            product.description_es = None;
            product
        })
        .collect::<Vec<_>>();
    let expected = fake::vec![ProductDocument; 42]
        .into_iter()
        .map(|mut product| {
            let year1 = rand::random_range(1830..=1833).into();
            let year2 = rand::random_range(1834..=1835).into();
            product.title_de = Some("The same title".into());
            product.origin_year_min = Some(year1);
            product.origin_year_max = Some(year2);
            product.description_de = None;
            product.description_en = None;
            product.description_fr = None;
            product.description_es = None;
            product
        })
        .collect::<Vec<_>>();
    let products = [civilians, expected.clone()].concat();
    let client = get_opensearch_client().await;
    let repository = ProductOpenSearchRepositoryImpl::new(client);
    let response = repository
        .create_product_documents(products.clone())
        .await
        .unwrap();
    assert!(!response.errors);
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_millis(5000)).await;

    let search_filter = ProductSearch {
        language: Language::De,
        currency: Currency::Eur,
        product_query: "The same title".try_into().unwrap(),
        shop_name_query: None,
        price_query: None,
        state_query: Default::default(),
        origin_year_query: Some(RangeQuery {
            min: None,
            max: Some(1835.into()),
        }),
        authenticity_query: Default::default(),
        condition_query: Default::default(),
        provenance_query: Default::default(),
        restoration_query: Default::default(),
        created_query: None,
        updated_query: None,
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
    let actual_products = response
        .hits
        .hits
        .into_iter()
        .map(|hit| hit.source)
        .collect::<Vec<_>>();

    assert_eq!(expected.len(), actual_products.len());
    assert!(
        actual_products
            .iter()
            .all(|product| product.origin_year_max.unwrap() <= Year::from(1835))
    );
}

#[localstack_test(services = [OpenSearch()])]
async fn should_search_product_documents_when_min_and_max_year_is_given_for_stored_year_range() {
    let civilians = fake::vec![ProductDocument; 50]
        .into_iter()
        .map(|mut product| {
            let year1 = rand::random_range(1830..=1831).into();
            let year2 = rand::random_range(1832..=1833).into();
            product.title_de = Some("The same title".into());
            product.origin_year_min = Some(year1);
            product.origin_year_max = Some(year2);
            product.description_de = None;
            product.description_en = None;
            product.description_fr = None;
            product.description_es = None;
            product
        })
        .collect::<Vec<_>>();
    let expected = fake::vec![ProductDocument; 42]
        .into_iter()
        .map(|mut product| {
            let year1 = rand::random_range(1836..=1838).into();
            let year2 = rand::random_range(1839..=1840).into();
            product.title_de = Some("The same title".into());
            product.origin_year_min = Some(year1);
            product.origin_year_max = Some(year2);
            product.description_de = None;
            product.description_en = None;
            product.description_fr = None;
            product.description_es = None;
            product
        })
        .collect::<Vec<_>>();
    let products = [civilians, expected.clone()].concat();
    let client = get_opensearch_client().await;
    let repository = ProductOpenSearchRepositoryImpl::new(client);
    let response = repository
        .create_product_documents(products.clone())
        .await
        .unwrap();
    assert!(!response.errors);
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_millis(5000)).await;

    let search_filter = ProductSearch {
        language: Language::De,
        currency: Currency::Eur,
        product_query: "The same title".try_into().unwrap(),
        shop_name_query: None,
        price_query: None,
        state_query: Default::default(),
        origin_year_query: Some(RangeQuery {
            min: Some(1835.into()),
            max: Some(1845.into()),
        }),
        authenticity_query: Default::default(),
        condition_query: Default::default(),
        provenance_query: Default::default(),
        restoration_query: Default::default(),
        created_query: None,
        updated_query: None,
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
    let actual_products = response
        .hits
        .hits
        .into_iter()
        .map(|hit| hit.source)
        .collect::<Vec<_>>();

    assert_eq!(expected.len(), actual_products.len());
    assert!(
        actual_products.iter().all(
            |product| product.origin_year_min.unwrap() >= Year::from(1834)
                && product.origin_year_max.unwrap() <= Year::from(1843)
        )
    );
}

#[rstest::rstest]
#[trace]
#[test_attr(apply(test))]
#[case(&[Authenticity::Original])]
#[case(&[Authenticity::LaterCopy])]
#[case(&[Authenticity::Reproduction])]
#[case(&[Authenticity::Questionable])]
#[case(&[Authenticity::Unknown])]
#[case(&[Authenticity::Original, Authenticity::LaterCopy])]
#[case(&[Authenticity::Reproduction, Authenticity::Questionable, Authenticity::Unknown])]
#[localstack_test(services = [OpenSearch()])]
async fn should_search_product_documents_when_authenticity_filter_is_given(
    #[case] authenticities: &[Authenticity],
) {
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
        product_query: "The same title".try_into().unwrap(),
        shop_name_query: None,
        price_query: None,
        state_query: Default::default(),
        origin_year_query: None,
        authenticity_query: AnyOfQuery::from(HashSet::from_iter(authenticities.iter().copied())),
        condition_query: Default::default(),
        provenance_query: Default::default(),
        restoration_query: Default::default(),
        created_query: None,
        updated_query: None,
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
    assert!(response.hits.hits.iter().all(|hit| {
        hit.source
            .authenticity
            .map(|a| authenticities.contains(&Authenticity::from(a)))
            .unwrap_or(false)
    }));
}

#[rstest::rstest]
#[trace]
#[test_attr(apply(test))]
#[case(&[Condition::Excellent])]
#[case(&[Condition::Great])]
#[case(&[Condition::Good])]
#[case(&[Condition::Fair])]
#[case(&[Condition::Poor])]
#[case(&[Condition::Unknown])]
#[case(&[Condition::Excellent, Condition::Great])]
#[case(&[Condition::Good, Condition::Fair, Condition::Poor])]
#[localstack_test(services = [OpenSearch()])]
async fn should_search_product_documents_when_condition_filter_is_given(
    #[case] conditions: &[Condition],
) {
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
        product_query: "The same title".try_into().unwrap(),
        shop_name_query: None,
        price_query: None,
        state_query: Default::default(),
        origin_year_query: None,
        authenticity_query: Default::default(),
        condition_query: AnyOfQuery::from(HashSet::from_iter(conditions.iter().copied())),
        provenance_query: Default::default(),
        restoration_query: Default::default(),
        created_query: None,
        updated_query: None,
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
    assert!(response.hits.hits.iter().all(|hit| {
        hit.source
            .condition
            .map(|c| conditions.contains(&Condition::from(c)))
            .unwrap_or(false)
    }));
}

#[rstest::rstest]
#[trace]
#[test_attr(apply(test))]
#[case(&[Provenance::Complete])]
#[case(&[Provenance::Partial])]
#[case(&[Provenance::Claimed])]
#[case(&[Provenance::None])]
#[case(&[Provenance::Unknown])]
#[case(&[Provenance::Complete, Provenance::Partial])]
#[case(&[Provenance::Claimed, Provenance::None, Provenance::Unknown])]
#[localstack_test(services = [OpenSearch()])]
async fn should_search_product_documents_when_provenance_filter_is_given(
    #[case] provenances: &[Provenance],
) {
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
        product_query: "The same title".try_into().unwrap(),
        shop_name_query: None,
        price_query: None,
        state_query: Default::default(),
        origin_year_query: None,
        authenticity_query: Default::default(),
        condition_query: Default::default(),
        provenance_query: AnyOfQuery::from(HashSet::from_iter(provenances.iter().copied())),
        restoration_query: Default::default(),
        created_query: None,
        updated_query: None,
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
    assert!(response.hits.hits.iter().all(|hit| {
        hit.source
            .provenance
            .map(|p| provenances.contains(&Provenance::from(p)))
            .unwrap_or(false)
    }));
}

#[rstest::rstest]
#[trace]
#[test_attr(apply(test))]
#[case(&[Restoration::None])]
#[case(&[Restoration::Minor])]
#[case(&[Restoration::Major])]
#[case(&[Restoration::Unknown])]
#[case(&[Restoration::None, Restoration::Minor])]
#[case(&[Restoration::Major, Restoration::Unknown])]
#[localstack_test(services = [OpenSearch()])]
async fn should_search_product_documents_when_restoration_filter_is_given(
    #[case] restorations: &[Restoration],
) {
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
        product_query: "The same title".try_into().unwrap(),
        shop_name_query: None,
        price_query: None,
        state_query: Default::default(),
        origin_year_query: None,
        authenticity_query: Default::default(),
        condition_query: Default::default(),
        provenance_query: Default::default(),
        restoration_query: AnyOfQuery::from(HashSet::from_iter(restorations.iter().copied())),
        created_query: None,
        updated_query: None,
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
    assert!(response.hits.hits.iter().all(|hit| {
        hit.source
            .restoration
            .map(|r| restorations.contains(&Restoration::from(r)))
            .unwrap_or(false)
    }));
}
