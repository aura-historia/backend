use common::pagination::cursor::Cursor;
use common::sort::SortOrder;
use common::{query::range_query::RangeQuery, sort::Sort};
use fake::{Fake, Faker};
use shop::core::sort_shop_field::SortShopField;
use shop::opensearch::{
    repository::{ShopOpenSearchRepository, ShopOpenSearchRepositoryImpl},
    shop_document::ShopDocument,
    shop_search::ShopSearch,
};
use std::time::Duration;
use test_api::*;
use time::macros::datetime;

async fn get_repository() -> ShopOpenSearchRepositoryImpl<'static> {
    ShopOpenSearchRepositoryImpl::new(get_opensearch_client().await)
}

#[localstack_test(services = [OpenSearch()])]
async fn should_create_shop_document() {
    let repository = get_repository().await;
    let expected = Faker.fake::<ShopDocument>();

    let response = repository
        .create_shop_document(expected.clone())
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
        .create_shop_document(expected.clone())
        .await
        .unwrap();
    assert_eq!("created", response.result);

    // insert civilians
    for doc in fake::vec![ShopDocument; 20] {
        let response = repository.create_shop_document(doc).await.unwrap();
        assert_eq!("created", response.result);
    }
    refresh_index("shops").await;
    tokio::time::sleep(Duration::from_secs(3)).await;

    let search = ShopSearch {
        shop_name_query: Some(expected.name.to_string().try_into().unwrap()),
        created: None,
        updated: None,
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

#[trace]
#[rstest::rstest]
#[test_attr(apply(test))]
#[case(
    ShopSearch {
        shop_name_query: Some("Expected name".try_into().unwrap()),
            created: Some(RangeQuery { min: Some(datetime!(2000 - 01 - 01 0:00 UTC)), max: Some(datetime!(3000 - 01 - 01 0:00 UTC)) }),
            updated: Some(RangeQuery { min: Some(datetime!(1000 - 01 - 01 0:00 UTC)), max: Some(datetime!(4000 - 01 - 01 0:00 UTC)) })
        },
    Sort { sort: SortShopField::Created, order: SortOrder::Asc },
)]
#[case(
    ShopSearch {
        shop_name_query: Some("Expected name".try_into().unwrap()),
            created: Some(RangeQuery { min: Some(datetime!(2000 - 01 - 01 0:00 UTC)), max: Some(datetime!(3000 - 01 - 01 0:00 UTC)) }),
            updated: Some(RangeQuery { min: Some(datetime!(1000 - 01 - 01 0:00 UTC)), max: Some(datetime!(4000 - 01 - 01 0:00 UTC)) })
        },
    Sort { sort: SortShopField::Created, order: SortOrder::Desc },
)]
#[case(
    ShopSearch {
        shop_name_query: Some("Expected name".try_into().unwrap()),
            created: Some(RangeQuery { min: Some(datetime!(2000 - 01 - 01 0:00 UTC)), max: Some(datetime!(3000 - 01 - 01 0:00 UTC)) }),
            updated: Some(RangeQuery { min: Some(datetime!(1000 - 01 - 01 0:00 UTC)), max: Some(datetime!(4000 - 01 - 01 0:00 UTC)) })
        },
    Sort { sort: SortShopField::Updated, order: SortOrder::Asc },
)]
#[case(
    ShopSearch {
        shop_name_query: Some("Expected name".try_into().unwrap()),
            created: Some(RangeQuery { min: Some(datetime!(2000 - 01 - 01 0:00 UTC)), max: Some(datetime!(3000 - 01 - 01 0:00 UTC)) }),
            updated: Some(RangeQuery { min: Some(datetime!(1000 - 01 - 01 0:00 UTC)), max: Some(datetime!(4000 - 01 - 01 0:00 UTC)) })
        },
    Sort { sort: SortShopField::Updated, order: SortOrder::Desc },
)]
#[case(
    ShopSearch {
        shop_name_query: Some("Expected name".try_into().unwrap()),
            created: Some(RangeQuery { min: Some(datetime!(2000 - 01 - 01 0:00 UTC)), max: Some(datetime!(3000 - 01 - 01 0:00 UTC)) }),
            updated: Some(RangeQuery { min: Some(datetime!(1000 - 01 - 01 0:00 UTC)), max: Some(datetime!(4000 - 01 - 01 0:00 UTC)) })
        },
    Sort { sort: SortShopField::Name, order: SortOrder::Asc },
)]
#[case(
    ShopSearch {
        shop_name_query: Some("Expected name".try_into().unwrap()),
            created: Some(RangeQuery { min: Some(datetime!(2000 - 01 - 01 0:00 UTC)), max: Some(datetime!(3000 - 01 - 01 0:00 UTC)) }),
            updated: Some(RangeQuery { min: Some(datetime!(1000 - 01 - 01 0:00 UTC)), max: Some(datetime!(4000 - 01 - 01 0:00 UTC)) })
        },
    Sort { sort: SortShopField::Name, order: SortOrder::Desc },
)]
#[case(
    ShopSearch {
        shop_name_query: Some("Expected name".try_into().unwrap()),
            created: Some(RangeQuery { min: Some(datetime!(2000 - 01 - 01 0:00 UTC)), max: Some(datetime!(3000 - 01 - 01 0:00 UTC)) }),
            updated: Some(RangeQuery { min: Some(datetime!(1000 - 01 - 01 0:00 UTC)), max: Some(datetime!(4000 - 01 - 01 0:00 UTC)) })
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
            updated: None
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
        .create_shop_document(expected.clone())
        .await
        .unwrap();
    assert_eq!("created", response.result);

    // insert civilians
    for doc in fake::vec![ShopDocument; 20] {
        let response = repository.create_shop_document(doc).await.unwrap();
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
async fn should_search_shop_documents_when_no_filters() {
    let repository = get_repository().await;

    // insert civilians
    for doc in fake::vec![ShopDocument; 20] {
        let response = repository.create_shop_document(doc).await.unwrap();
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
