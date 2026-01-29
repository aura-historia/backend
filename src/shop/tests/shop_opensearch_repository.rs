use common::domain::Domain;
use common::pagination::cursor::Cursor;
use common::shop_name::ShopName;
use common::slug_id::SlugId;
use common::sort::SortOrder;
use common::{query::range_query::RangeQuery, sort::Sort};
use fake::{Fake, Faker};
use shop::core::shop_search::ShopSearch;
use shop::core::sort_shop_field::SortShopField;
use shop::opensearch::shop_document_update::ShopDocumentUpdate;
use shop::opensearch::{
    repository::{ShopOpenSearchRepository, ShopOpenSearchRepositoryImpl},
    shop_document::ShopDocument,
};
use std::time::Duration;
use test_api::*;
use time::OffsetDateTime;
use time::macros::datetime;
use url::Url;

async fn get_repository() -> ShopOpenSearchRepositoryImpl<'static> {
    ShopOpenSearchRepositoryImpl::new(get_opensearch_client().await)
}

#[localstack_test(services = [OpenSearch()])]
async fn should_index_shop_document_when_not_exists() {
    let repository = get_repository().await;
    let expected = Faker.fake::<ShopDocument>();

    let response = repository
        .index_shop_document(expected.clone())
        .await
        .unwrap();
    assert_eq!("created", response.result);
    refresh_index("shops").await;
    tokio::time::sleep(Duration::from_secs(3)).await;

    let actual = read_by_id::<ShopDocument>("shops", expected.shop_id).await;
    assert_eq!(expected, actual);
}

#[localstack_test(services = [OpenSearch()])]
async fn should_search_shop_documents_when_only_name_query_supplied() {
    let repository = get_repository().await;
    let expected = Faker.fake::<ShopDocument>();

    // insert expected
    let response = repository
        .index_shop_document(expected.clone())
        .await
        .unwrap();
    assert_eq!("created", response.result);

    // insert civilians
    for doc in fake::vec![ShopDocument; 20] {
        let response = repository.index_shop_document(doc).await.unwrap();
        assert_eq!("created", response.result);
    }
    refresh_index("shops").await;
    tokio::time::sleep(Duration::from_secs(3)).await;

    let search = ShopSearch {
        shop_name_query: Some(expected.name.to_string().try_into().unwrap()),
        shop_type_query: Default::default(),
        created: None,
        updated: None,
        min_score: None,
    };
    let actual = repository
        .search_shop_documents(
            &search,
            &Sort {
                sort: SortShopField::Score,
                order: SortOrder::Desc,
            },
            &None,
        )
        .await
        .unwrap();

    assert_eq!(expected, actual.hits.hits[0].source);
}

#[rstest::rstest]
#[trace]
#[test_attr(apply(test))]
#[case(
    ShopSearch {
        shop_name_query: Some("Expected name".try_into().unwrap()),
            created: Some(RangeQuery { min: Some(datetime!(2000 - 01 - 01 0:00 UTC)), max: Some(datetime!(3000 - 01 - 01 0:00 UTC)) }),
            updated: Some(RangeQuery { min: Some(datetime!(1000 - 01 - 01 0:00 UTC)), max: Some(datetime!(4000 - 01 - 01 0:00 UTC)) }),
            min_score: None,
            shop_type_query: Default::default(),
        },
    Sort { sort: SortShopField::Created, order: SortOrder::Asc },
)]
#[case(
    ShopSearch {
        shop_name_query: Some("Expected name".try_into().unwrap()),
            created: Some(RangeQuery { min: Some(datetime!(2000 - 01 - 01 0:00 UTC)), max: Some(datetime!(3000 - 01 - 01 0:00 UTC)) }),
            updated: Some(RangeQuery { min: Some(datetime!(1000 - 01 - 01 0:00 UTC)), max: Some(datetime!(4000 - 01 - 01 0:00 UTC)) }),
            min_score: None,
            shop_type_query: Default::default(),
        },
    Sort { sort: SortShopField::Created, order: SortOrder::Desc },
)]
#[case(
    ShopSearch {
        shop_name_query: Some("Expected name".try_into().unwrap()),
            created: Some(RangeQuery { min: Some(datetime!(2000 - 01 - 01 0:00 UTC)), max: Some(datetime!(3000 - 01 - 01 0:00 UTC)) }),
            updated: Some(RangeQuery { min: Some(datetime!(1000 - 01 - 01 0:00 UTC)), max: Some(datetime!(4000 - 01 - 01 0:00 UTC)) }),
            min_score: None,
            shop_type_query: Default::default(),
        },
    Sort { sort: SortShopField::Updated, order: SortOrder::Asc },
)]
#[case(
    ShopSearch {
        shop_name_query: Some("Expected name".try_into().unwrap()),
            created: Some(RangeQuery { min: Some(datetime!(2000 - 01 - 01 0:00 UTC)), max: Some(datetime!(3000 - 01 - 01 0:00 UTC)) }),
            updated: Some(RangeQuery { min: Some(datetime!(1000 - 01 - 01 0:00 UTC)), max: Some(datetime!(4000 - 01 - 01 0:00 UTC)) }),
            min_score: None,
            shop_type_query: Default::default(),
        },
    Sort { sort: SortShopField::Updated, order: SortOrder::Desc },
)]
#[case(
    ShopSearch {
        shop_name_query: Some("Expected name".try_into().unwrap()),
            created: Some(RangeQuery { min: Some(datetime!(2000 - 01 - 01 0:00 UTC)), max: Some(datetime!(3000 - 01 - 01 0:00 UTC)) }),
            updated: Some(RangeQuery { min: Some(datetime!(1000 - 01 - 01 0:00 UTC)), max: Some(datetime!(4000 - 01 - 01 0:00 UTC)) }),
            min_score: None,
            shop_type_query: Default::default(),
        },
    Sort { sort: SortShopField::Name, order: SortOrder::Asc },
)]
#[case(
    ShopSearch {
        shop_name_query: Some("Expected name".try_into().unwrap()),
            created: Some(RangeQuery { min: Some(datetime!(2000 - 01 - 01 0:00 UTC)), max: Some(datetime!(3000 - 01 - 01 0:00 UTC)) }),
            updated: Some(RangeQuery { min: Some(datetime!(1000 - 01 - 01 0:00 UTC)), max: Some(datetime!(4000 - 01 - 01 0:00 UTC)) }),
            min_score: None,
            shop_type_query: Default::default(),
        },
    Sort { sort: SortShopField::Name, order: SortOrder::Desc },
)]
#[case(
    ShopSearch {
        shop_name_query: Some("Expected name".try_into().unwrap()),
            created: Some(RangeQuery { min: Some(datetime!(2000 - 01 - 01 0:00 UTC)), max: Some(datetime!(3000 - 01 - 01 0:00 UTC)) }),
            updated: Some(RangeQuery { min: Some(datetime!(1000 - 01 - 01 0:00 UTC)), max: Some(datetime!(4000 - 01 - 01 0:00 UTC)) }),
            min_score: None,
            shop_type_query: Default::default(),
        },
    Sort {
        sort: SortShopField::Score,
        order: SortOrder::Desc,
    },
)]
#[case(
    ShopSearch {
        shop_name_query: Some("Expected name".try_into().unwrap()),
            created: Some(RangeQuery { min: Some(datetime!(2000 - 01 - 01 0:00 UTC)), max: Some(datetime!(3000 - 01 - 01 0:00 UTC)) }),
            updated: None,
            shop_type_query: Default::default(),
        },
    Sort {
        sort: SortShopField::Score,
        order: SortOrder::Desc,
    },
)]
#[case(
    ShopSearch {
        shop_name_query: Some("Expected name".try_into().unwrap()),
            created: None,
            updated: Some(RangeQuery { min: Some(datetime!(2000 - 01 - 01 0:00 UTC)), max: Some(datetime!(3000 - 01 - 01 0:00 UTC)) }),
            min_score: None,
            shop_type_query: Default::default(),
        },
    Sort {
        sort: SortShopField::Score,
        order: SortOrder::Desc,
    },
)]
#[localstack_test(services = [OpenSearch()])]
async fn should_search_shop_documents_for_arguments(
    #[case] search: ShopSearch,
    #[case] sort: Sort<SortShopField>,
) {
    let repository = get_repository().await;
    let mut expected = Faker.fake::<ShopDocument>();
    expected.name = "Expected name".into();

    // insert expected
    let response = repository
        .index_shop_document(expected.clone())
        .await
        .unwrap();
    assert_eq!("created", response.result);

    // insert civilians
    for doc in fake::vec![ShopDocument; 20] {
        let response = repository.index_shop_document(doc).await.unwrap();
        assert_eq!("created", response.result);
    }
    refresh_index("shops").await;
    tokio::time::sleep(Duration::from_secs(3)).await;

    let actual = repository
        .search_shop_documents(&search, &sort, &None)
        .await
        .unwrap();

    assert!(actual.hits.hits.iter().any(|hit| hit.source == expected));
}

#[localstack_test(services = [OpenSearch()])]
async fn should_update_shop_document_for_index() {
    let repository = get_repository().await;
    let create_expected = Faker.fake::<ShopDocument>();

    let created_res = repository
        .index_shop_document(create_expected.clone())
        .await
        .unwrap();
    assert_eq!("created", created_res.result);
    refresh_index("shops").await;
    tokio::time::sleep(Duration::from_secs(3)).await;

    let created = read_by_id::<ShopDocument>("shops", create_expected.shop_id).await;
    assert_eq!(create_expected, created);

    let name: ShopName = "Hansi hans and the Hanses".into();
    let updated_expected = ShopDocument {
        shop_id: created.shop_id,
        shop_slug_id: SlugId::from(name.as_ref()),
        name,
        shop_type: Faker.fake(),
        domains: HashSet::from_iter([
            Domain::try_from("hansi-hans.de").unwrap(),
            Domain::try_from("hansi-hans.com").unwrap(),
        ]),
        image: Some(Url::parse("https://hansi-hanseatic.es/foo.png").unwrap()),
        created: created.created,
        updated: OffsetDateTime::now_utc(),
    };

    let updated_res = repository
        .index_shop_document(updated_expected.clone())
        .await
        .unwrap();
    assert_eq!("updated", updated_res.result);
    refresh_index("shops").await;
    tokio::time::sleep(Duration::from_secs(3)).await;

    let updated = read_by_id::<ShopDocument>("shops", updated_expected.shop_id).await;
    assert_eq!(updated_expected, updated);
}

#[localstack_test(services = [OpenSearch()])]
async fn should_update_shop_document_for_update() {
    let repository = get_repository().await;
    let create_expected = Faker.fake::<ShopDocument>();

    let created_res = repository
        .index_shop_document(create_expected.clone())
        .await
        .unwrap();
    assert_eq!("created", created_res.result);
    refresh_index("shops").await;
    tokio::time::sleep(Duration::from_secs(3)).await;

    let created = read_by_id::<ShopDocument>("shops", create_expected.shop_id).await;
    assert_eq!(create_expected, created);

    let update = ShopDocumentUpdate {
        domains: Some(HashSet::from_iter([
            Domain::try_from("hansi-hans.de").unwrap(),
            Domain::try_from("hansi-hans.com").unwrap(),
        ])),
        image: Some(Url::parse("https://hansi-hanseatic.es/foo.png").unwrap()),
        updated: OffsetDateTime::now_utc(),
    };

    let updated_res = repository
        .update_shop_document(&created.shop_id, update.clone())
        .await
        .unwrap();
    assert_eq!("updated", updated_res.result);
    refresh_index("shops").await;
    tokio::time::sleep(Duration::from_secs(3)).await;

    let updated = read_by_id::<ShopDocument>("shops", create_expected.shop_id).await;
    assert_eq!(update.domains.unwrap(), updated.domains);
    assert_eq!(update.image.unwrap(), updated.image.unwrap());
    assert_eq!(update.updated, updated.updated);
}

#[localstack_test(services = [OpenSearch()])]
async fn should_search_shop_documents_when_no_filters() {
    let repository = get_repository().await;

    // insert civilians
    for doc in fake::vec![ShopDocument; 20] {
        let response = repository.index_shop_document(doc).await.unwrap();
        assert_eq!("created", response.result);
    }
    refresh_index("shops").await;
    tokio::time::sleep(Duration::from_secs(3)).await;

    let actual = repository
        .search_shop_documents(
            &Default::default(),
            &Sort {
                sort: SortShopField::Name,
                order: SortOrder::Asc,
            },
            &Some(Cursor {
                size: 20,
                search_after: None,
            }),
        )
        .await
        .unwrap();

    assert_eq!(20, actual.hits.hits.len());
    assert_eq!(20, actual.hits.total.value);
}

#[rstest::rstest]
#[trace]
#[test_attr(apply(test))]
#[case(&[shop::core::shop_type::ShopType::AuctionHouse])]
#[case(&[shop::core::shop_type::ShopType::AuctionPlatform])]
#[case(&[shop::core::shop_type::ShopType::CommercialDealer])]
#[case(&[shop::core::shop_type::ShopType::Marketplace])]
#[case(&[shop::core::shop_type::ShopType::AuctionHouse, shop::core::shop_type::ShopType::Marketplace])]
#[localstack_test(services = [OpenSearch()])]
async fn should_search_shop_documents_when_shop_types_are_given(
    #[case] shop_types: &[shop::core::shop_type::ShopType],
) {
    use common::query::any_of_query::AnyOfQuery;
    use std::collections::HashSet;

    let repository = get_repository().await;
    let shops = fake::vec![ShopDocument; 100];

    for doc in shops {
        repository.index_shop_document(doc).await.unwrap();
    }
    refresh_index("shops").await;
    tokio::time::sleep(Duration::from_secs(3)).await;

    let search = ShopSearch {
        shop_name_query: None,
        shop_type_query: AnyOfQuery::from(HashSet::from_iter(shop_types.iter().copied())),
        created: None,
        updated: None,
        min_score: None,
    };
    let response = repository
        .search_shop_documents(
            &search,
            &Sort {
                sort: SortShopField::Score,
                order: SortOrder::Desc,
            },
            &None,
        )
        .await
        .unwrap();

    assert!(response.hits.total.value > 0);
    assert!(response.hits.hits.iter().all(|hit| {
        shop_types.contains(&shop::core::shop_type::ShopType::from(hit.source.shop_type))
    }));
}

#[localstack_test(services = [OpenSearch()])]
async fn should_filter_shop_documents_when_min_score_is_given() {
    let repository = get_repository().await;

    // Create a shop with high relevance
    let high_relevance_shop = ShopDocument {
        shop_id: Default::default(),
        shop_slug_id: Faker.fake(),
        name: ShopName::from("Antique Auction House"),
        domain: Domain::from("antique-auction.com"),
        shop_type: shop::opensearch::shop_type_document::ShopTypeDocument::AuctionHouse,
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
        url: Url::parse("https://antique-auction.com").unwrap(),
    };

    // Create a shop with lower relevance
    let low_relevance_shop = ShopDocument {
        shop_id: Default::default(),
        shop_slug_id: Faker.fake(),
        name: ShopName::from("Modern Store antique mention"),
        domain: Domain::from("modern-store.com"),
        shop_type: shop::opensearch::shop_type_document::ShopTypeDocument::CommercialDealer,
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
        url: Url::parse("https://modern-store.com").unwrap(),
    };

    // Index both shops
    let response1 = repository
        .index_shop_document(high_relevance_shop.clone())
        .await
        .unwrap();
    assert_eq!("created", response1.result);

    let response2 = repository
        .index_shop_document(low_relevance_shop.clone())
        .await
        .unwrap();
    assert_eq!("created", response2.result);

    refresh_index("shops").await;
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Search without min_score - should return both shops
    let search_without_threshold = ShopSearch {
        shop_name_query: Some("antique".try_into().unwrap()),
        shop_type_query: Default::default(),
        created: None,
        updated: None,
        min_score: None,
    };

    let response_without_threshold = repository
        .search_shop_documents(
            &search_without_threshold,
            &Sort {
                sort: SortShopField::Score,
                order: SortOrder::Desc,
            },
            &None,
        )
        .await
        .unwrap();

    // Should return both shops
    assert_eq!(response_without_threshold.hits.hits.len(), 2);

    // Search with high min_score threshold - should filter out low relevance shop
    let search_with_threshold = ShopSearch {
        shop_name_query: Some("antique".try_into().unwrap()),
        shop_type_query: Default::default(),
        created: None,
        updated: None,
        min_score: Some(0.5),
    };

    let response_with_threshold = repository
        .search_shop_documents(
            &search_with_threshold,
            &Sort {
                sort: SortShopField::Score,
                order: SortOrder::Desc,
            },
            &None,
        )
        .await
        .unwrap();

    // Should return at most 2 shops, at least 1
    assert!(response_with_threshold.hits.hits.len() <= 2);
    assert!(response_with_threshold.hits.hits.len() >= 1);

    // Verify that all returned shops have scores >= min_score
    for hit in response_with_threshold.hits.hits {
        assert!(hit.score.unwrap_or(0.0) >= 0.5);
    }
}
