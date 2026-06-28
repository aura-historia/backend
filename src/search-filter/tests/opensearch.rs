use common::currency::record::CurrencyRecord;
use common::event_id::EventId;
use common::language::document::{LanguageDocument, TextDocument};
use common::language::record::LanguageRecord;
use common::product_id::ProductId;
use common::product_slug_id::ProductSlugId;
use common::query::range_query::RangeQuery;
use common::resource_state::record::ResourceStateRecord;
use common::seller_slug_id::SellerSlugId;
use common::shop_id::ShopId;
use common::shop_slug_id::ShopSlugId;
use common::shops_product_id::ShopsProductId;
use common::user_id::UserId;
use common::user_search_filter_id::UserSearchFilterId;
use opensearch::http::Url;
use product::opensearch::product_document::ProductDocument;
use product::opensearch::product_state_document::ProductStateDocument;
use search_filter::dynamodb::user_search_filter_record::UserSearchFilterRecord;
use search_filter::opensearch::repository::{
    UserSearchFilterOpenSearchRepository, UserSearchFilterOpenSearchRepositoryImpl,
};
use search_filter::opensearch::user_search_filter_document::UserSearchFilterDocument;
use shop::dynamodb::shop_type_record::ShopTypeRecord;
use shop::opensearch::shop_type_document::ShopTypeDocument;
use std::collections::HashSet;
use test_api::*;
use time::macros::datetime;

#[localstack_test(services = [OpenSearch()])]
async fn should_index_user_search_filter_document() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let expected = exact_document();

    let response = repository.index_document(expected.clone()).await.unwrap();

    assert_eq!(response.id, expected.user_search_filter_id.to_string());

    refresh_index("user_search_filters").await;

    let actual: UserSearchFilterDocument =
        read_by_id("user_search_filters", expected.user_search_filter_id).await;

    assert_eq!(expected, actual);
}

#[localstack_test(services = [OpenSearch()])]
async fn should_update_user_search_filter_document_when_indexing_same_id_again() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let first = exact_document();
    let mut second = first.clone();
    second.name = "updated search filter".into();
    second.updated = datetime!(2024-02-02 12:00:00 UTC);

    repository.index_document(first).await.unwrap();
    repository.index_document(second.clone()).await.unwrap();

    refresh_index("user_search_filters").await;

    let actual: UserSearchFilterDocument =
        read_by_id("user_search_filters", second.user_search_filter_id).await;

    assert_eq!(second, actual);
}

#[localstack_test(services = [OpenSearch()])]
async fn should_index_user_search_filter_document_with_query_embedding() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut expected = unique_document();
    expected.embedding = Some(embedding(25));

    repository.index_document(expected.clone()).await.unwrap();
    refresh_index("user_search_filters").await;

    let actual: UserSearchFilterDocument =
        read_by_id("user_search_filters", expected.user_search_filter_id).await;

    assert_eq!(actual.embedding, Some(embedding(25)));
}

#[localstack_test(services = [OpenSearch()])]
async fn should_update_query_embedding_when_indexing_same_filter_again() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut first = unique_document();
    first.embedding = Some(embedding(11));
    let mut second = first.clone();
    second.embedding = Some(embedding(77));
    second.updated = datetime!(2024-02-03 12:00:00 UTC);

    repository.index_document(first).await.unwrap();
    repository.index_document(second.clone()).await.unwrap();
    refresh_index("user_search_filters").await;

    let actual: UserSearchFilterDocument =
        read_by_id("user_search_filters", second.user_search_filter_id).await;

    assert_eq!(actual.embedding, Some(embedding(77)));
}

#[localstack_test(services = [OpenSearch()])]
async fn should_get_user_search_filter_document_with_query_embedding() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut expected = unique_document();
    expected.embedding = Some(embedding(33));
    repository.index_document(expected.clone()).await.unwrap();
    refresh_index("user_search_filters").await;

    let actual = repository
        .get_document(&expected.user_search_filter_id)
        .await
        .unwrap();

    assert_eq!(actual, Some(expected));
}

#[localstack_test(services = [OpenSearch()])]
async fn should_delete_user_search_filter_document() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let document = exact_document();
    repository.index_document(document.clone()).await.unwrap();
    refresh_index("user_search_filters").await;

    let response = repository
        .delete_document(&document.user_search_filter_id)
        .await
        .unwrap();

    assert_eq!(response.id, document.user_search_filter_id.to_string());

    refresh_index("user_search_filters").await;

    let search_response = repository
        .percolate(&base_product_document())
        .await
        .unwrap();

    assert!(search_response.is_empty());
}

#[localstack_test(services = [OpenSearch()])]
async fn should_return_empty_percolate_result_when_index_has_no_documents() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let actual = repository
        .percolate(&base_product_document())
        .await
        .unwrap();

    assert!(actual.is_empty());
}

#[localstack_test(services = [OpenSearch()])]
async fn should_percolate_document_when_query_is_empty() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_record();
    record.shop_name_query.clear();
    record.seller_name_query.clear();
    record.exclude_shop_name_query.clear();
    record.exclude_seller_name_query.clear();
    record.shop_type_query.clear();
    record.price_query = None;
    record.state_query.clear();
    record.created_query = None;
    record.updated_query = None;
    record.auction_start_query = None;
    record.auction_end_query = None;

    let expected = index_document(&repository, record).await;
    let actual = repository
        .percolate(&base_product_document())
        .await
        .unwrap();

    assert_eq!(vec![expected], actual);
}

#[localstack_test(services = [OpenSearch()])]
async fn should_percolate_document_when_product_query_matches_title_in_selected_language() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_record();
    record.product_query = vec!["renaissance cabinet".try_into().unwrap()];
    record.language = common::language::record::LanguageRecord::En;

    let expected = index_document(&repository, record).await;
    let actual = repository
        .percolate(&base_product_document())
        .await
        .unwrap();

    assert_eq!(vec![expected], actual);
}

#[localstack_test(services = [OpenSearch()])]
async fn should_percolate_document_when_any_product_query_matches_title_in_selected_language() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_record();
    record.product_query = vec![
        "completely unrelated phrase".try_into().unwrap(),
        "renaissance cabinet".try_into().unwrap(),
    ];
    record.language = common::language::record::LanguageRecord::En;

    let expected = index_document(&repository, record).await;
    let actual = repository
        .percolate(&base_product_document())
        .await
        .unwrap();

    assert_eq!(vec![expected], actual);
}

#[localstack_test(services = [OpenSearch()])]
async fn should_not_percolate_document_when_product_query_does_not_match() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_record();
    record.product_query = vec!["completely unrelated phrase".try_into().unwrap()];

    index_document(&repository, record).await;

    let actual = repository
        .percolate(&base_product_document())
        .await
        .unwrap();

    assert!(actual.is_empty());
}

#[localstack_test(services = [OpenSearch()])]
async fn should_percolate_document_when_shop_name_matches() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_record();
    record.shop_name_query = HashSet::from_iter(["Imperial Antiques".into()]);

    let expected = index_document(&repository, record).await;
    let actual = repository
        .percolate(&base_product_document())
        .await
        .unwrap();

    assert_eq!(vec![expected], actual);
}

#[localstack_test(services = [OpenSearch()])]
async fn should_not_percolate_document_when_shop_name_does_not_match() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_record();
    record.shop_name_query = HashSet::from_iter(["Other Shop".into()]);

    index_document(&repository, record).await;

    let actual = repository
        .percolate(&base_product_document())
        .await
        .unwrap();

    assert!(actual.is_empty());
}

#[localstack_test(services = [OpenSearch()])]
async fn should_not_percolate_document_when_excluded_shop_name_matches_product() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_record();
    record.exclude_shop_name_query = HashSet::from_iter(["Imperial Antiques".into()]);

    index_document(&repository, record).await;

    let actual = repository
        .percolate(&base_product_document())
        .await
        .unwrap();

    assert!(actual.is_empty());
}

#[localstack_test(services = [OpenSearch()])]
async fn should_percolate_document_when_excluded_shop_name_does_not_match_product() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_record();
    record.exclude_shop_name_query = HashSet::from_iter(["Different Shop".into()]);

    let expected = index_document(&repository, record).await;
    let actual = repository
        .percolate(&base_product_document())
        .await
        .unwrap();

    assert_eq!(vec![expected], actual);
}

#[localstack_test(services = [OpenSearch()])]
async fn should_percolate_document_when_shop_type_matches() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_record();
    record.shop_type_query = HashSet::from_iter([ShopTypeRecord::CommercialDealer]);

    let expected = index_document(&repository, record).await;
    let actual = repository
        .percolate(&base_product_document())
        .await
        .unwrap();

    assert_eq!(vec![expected], actual);
}

#[localstack_test(services = [OpenSearch()])]
async fn should_not_percolate_document_when_shop_type_does_not_match() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_record();
    record.shop_type_query = HashSet::from_iter([ShopTypeRecord::Marketplace]);

    index_document(&repository, record).await;

    let actual = repository
        .percolate(&base_product_document())
        .await
        .unwrap();

    assert!(actual.is_empty());
}

#[localstack_test(services = [OpenSearch()])]
async fn should_percolate_document_when_state_matches() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_record();
    record.state_query =
        HashSet::from_iter([product::dynamodb::product_state_record::ProductStateRecord::Listed]);

    let expected = index_document(&repository, record).await;
    let actual = repository
        .percolate(&base_product_document())
        .await
        .unwrap();

    assert_eq!(vec![expected], actual);
}

#[localstack_test(services = [OpenSearch()])]
async fn should_not_percolate_document_when_state_does_not_match() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_record();
    record.state_query =
        HashSet::from_iter([product::dynamodb::product_state_record::ProductStateRecord::Sold]);

    index_document(&repository, record).await;

    let actual = repository
        .percolate(&base_product_document())
        .await
        .unwrap();

    assert!(actual.is_empty());
}

#[localstack_test(services = [OpenSearch()])]
async fn should_percolate_document_when_price_is_greater_than_or_equal_to_minimum_for_currency() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_record();
    record.currency = common::currency::record::CurrencyRecord::Eur;
    record.price_query = Some(RangeQuery {
        min: Some(100),
        max: None,
    });

    let expected = index_document(&repository, record).await;
    let actual = repository
        .percolate(&base_product_document())
        .await
        .unwrap();

    assert_eq!(vec![expected], actual);
}

#[localstack_test(services = [OpenSearch()])]
async fn should_not_percolate_document_when_price_is_below_minimum_for_currency() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_record();
    record.currency = common::currency::record::CurrencyRecord::Eur;
    record.price_query = Some(RangeQuery {
        min: Some(151),
        max: None,
    });

    index_document(&repository, record).await;

    let actual = repository
        .percolate(&base_product_document())
        .await
        .unwrap();

    assert!(actual.is_empty());
}

#[localstack_test(services = [OpenSearch()])]
async fn should_percolate_document_when_price_is_less_than_or_equal_to_maximum_for_currency() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_record();
    record.currency = common::currency::record::CurrencyRecord::Eur;
    record.price_query = Some(RangeQuery {
        min: None,
        max: Some(150),
    });

    let expected = index_document(&repository, record).await;
    let actual = repository
        .percolate(&base_product_document())
        .await
        .unwrap();

    assert_eq!(vec![expected], actual);
}

#[localstack_test(services = [OpenSearch()])]
async fn should_not_percolate_document_when_price_is_above_maximum_for_currency() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_record();
    record.currency = common::currency::record::CurrencyRecord::Eur;
    record.price_query = Some(RangeQuery {
        min: None,
        max: Some(149),
    });

    index_document(&repository, record).await;

    let actual = repository
        .percolate(&base_product_document())
        .await
        .unwrap();

    assert!(actual.is_empty());
}

#[localstack_test(services = [OpenSearch()])]
async fn should_percolate_document_when_price_is_within_min_and_max_range() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_record();
    record.currency = common::currency::record::CurrencyRecord::Eur;
    record.price_query = Some(RangeQuery {
        min: Some(120),
        max: Some(180),
    });

    let expected = index_document(&repository, record).await;
    let actual = repository
        .percolate(&base_product_document())
        .await
        .unwrap();

    assert_eq!(vec![expected], actual);
}

#[localstack_test(services = [OpenSearch()])]
async fn should_not_percolate_document_when_price_is_outside_min_and_max_range() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_record();
    record.currency = common::currency::record::CurrencyRecord::Eur;
    record.price_query = Some(RangeQuery {
        min: Some(10),
        max: Some(149),
    });

    index_document(&repository, record).await;

    let actual = repository
        .percolate(&base_product_document())
        .await
        .unwrap();

    assert!(actual.is_empty());
}

#[localstack_test(services = [OpenSearch()])]
async fn should_percolate_document_when_usd_price_range_matches_usd_price() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut product = base_product_document();
    product.price_eur = None;
    product.price_usd = Some(220);

    let mut record = base_record();
    record.currency = common::currency::record::CurrencyRecord::Usd;
    record.price_query = Some(RangeQuery {
        min: Some(200),
        max: Some(250),
    });

    let expected = index_document(&repository, record).await;
    let actual = repository.percolate(&product).await.unwrap();

    assert_eq!(vec![expected], actual);
}

#[localstack_test(services = [OpenSearch()])]
async fn should_not_percolate_document_when_selected_currency_price_field_is_missing() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let product = base_product_document();

    let mut record = base_record();
    record.currency = common::currency::record::CurrencyRecord::Usd;
    record.price_query = Some(RangeQuery {
        min: Some(1),
        max: Some(500),
    });

    index_document(&repository, record).await;

    let actual = repository.percolate(&product).await.unwrap();

    assert!(actual.is_empty());
}

#[localstack_test(services = [OpenSearch()])]
async fn should_percolate_document_when_created_is_within_min_and_max_range() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_record();
    record.created_query = Some(RangeQuery {
        min: Some(datetime!(2024-01-09 00:00:00 UTC)),
        max: Some(datetime!(2024-01-11 00:00:00 UTC)),
    });

    let expected = index_document(&repository, record).await;
    let actual = repository
        .percolate(&base_product_document())
        .await
        .unwrap();

    assert_eq!(vec![expected], actual);
}

#[localstack_test(services = [OpenSearch()])]
async fn should_not_percolate_document_when_created_is_outside_range() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_record();
    record.created_query = Some(RangeQuery {
        min: Some(datetime!(2024-01-11 00:00:01 UTC)),
        max: Some(datetime!(2024-01-12 00:00:00 UTC)),
    });

    index_document(&repository, record).await;

    let actual = repository
        .percolate(&base_product_document())
        .await
        .unwrap();

    assert!(actual.is_empty());
}

#[localstack_test(services = [OpenSearch()])]
async fn should_percolate_document_when_updated_is_within_min_and_max_range() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_record();
    record.updated_query = Some(RangeQuery {
        min: Some(datetime!(2024-01-14 00:00:00 UTC)),
        max: Some(datetime!(2024-01-16 00:00:00 UTC)),
    });

    let expected = index_document(&repository, record).await;
    let actual = repository
        .percolate(&base_product_document())
        .await
        .unwrap();

    assert_eq!(vec![expected], actual);
}

#[localstack_test(services = [OpenSearch()])]
async fn should_not_percolate_document_when_updated_is_outside_range() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_record();
    record.updated_query = Some(RangeQuery {
        min: Some(datetime!(2024-01-16 00:00:01 UTC)),
        max: Some(datetime!(2024-01-17 00:00:00 UTC)),
    });

    index_document(&repository, record).await;

    let actual = repository
        .percolate(&base_product_document())
        .await
        .unwrap();

    assert!(actual.is_empty());
}

#[localstack_test(services = [OpenSearch()])]
async fn should_percolate_document_when_auction_start_is_within_min_and_max_range() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_record();
    record.auction_start_query = Some(RangeQuery {
        min: Some(datetime!(2024-01-20 00:00:00 UTC)),
        max: Some(datetime!(2024-01-21 00:00:00 UTC)),
    });

    let expected = index_document(&repository, record).await;
    let actual = repository
        .percolate(&base_product_document())
        .await
        .unwrap();

    assert_eq!(vec![expected], actual);
}

#[localstack_test(services = [OpenSearch()])]
async fn should_not_percolate_document_when_auction_start_is_outside_range() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_record();
    record.auction_start_query = Some(RangeQuery {
        min: Some(datetime!(2024-01-21 00:00:01 UTC)),
        max: Some(datetime!(2024-01-22 00:00:00 UTC)),
    });

    index_document(&repository, record).await;

    let actual = repository
        .percolate(&base_product_document())
        .await
        .unwrap();

    assert!(actual.is_empty());
}

#[localstack_test(services = [OpenSearch()])]
async fn should_percolate_document_when_auction_end_is_within_min_and_max_range() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_record();
    record.auction_end_query = Some(RangeQuery {
        min: Some(datetime!(2024-01-25 00:00:00 UTC)),
        max: Some(datetime!(2024-01-26 00:00:00 UTC)),
    });

    let expected = index_document(&repository, record).await;
    let actual = repository
        .percolate(&base_product_document())
        .await
        .unwrap();

    assert_eq!(vec![expected], actual);
}

#[localstack_test(services = [OpenSearch()])]
async fn should_not_percolate_document_when_auction_end_is_outside_range() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_record();
    record.auction_end_query = Some(RangeQuery {
        min: Some(datetime!(2024-01-26 00:00:01 UTC)),
        max: Some(datetime!(2024-01-27 00:00:00 UTC)),
    });

    index_document(&repository, record).await;

    let actual = repository
        .percolate(&base_product_document())
        .await
        .unwrap();

    assert!(actual.is_empty());
}

#[localstack_test(services = [OpenSearch()])]
async fn should_not_percolate_document_when_auction_start_query_is_given_but_product_has_no_auction_start()
 {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut product = base_product_document();
    product.auction_start = None;

    let mut record = base_record();
    record.auction_start_query = Some(RangeQuery {
        min: Some(datetime!(2024-01-20 00:00:00 UTC)),
        max: Some(datetime!(2024-01-21 00:00:00 UTC)),
    });

    index_document(&repository, record).await;

    let actual = repository.percolate(&product).await.unwrap();

    assert!(actual.is_empty());
}

#[localstack_test(services = [OpenSearch()])]
async fn should_not_percolate_document_when_auction_end_query_is_given_but_product_has_no_auction_end()
 {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut product = base_product_document();
    product.auction_end = None;

    let mut record = base_record();
    record.auction_end_query = Some(RangeQuery {
        min: Some(datetime!(2024-01-25 00:00:00 UTC)),
        max: Some(datetime!(2024-01-26 00:00:00 UTC)),
    });

    index_document(&repository, record).await;

    let actual = repository.percolate(&product).await.unwrap();

    assert!(actual.is_empty());
}

#[localstack_test(services = [OpenSearch()])]
async fn should_percolate_only_documents_that_match_when_multiple_filters_are_indexed() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut matching_record = base_record();
    matching_record.user_search_filter_id = UserSearchFilterId::new();
    matching_record.name = "matching".into();
    matching_record.product_query = vec!["renaissance cabinet".try_into().unwrap()];
    matching_record.shop_name_query = HashSet::from_iter(["Imperial Antiques".into()]);
    matching_record.price_query = Some(RangeQuery {
        min: Some(100),
        max: Some(200),
    });

    let mut non_matching_record = base_record();
    non_matching_record.user_search_filter_id = UserSearchFilterId::new();
    non_matching_record.name = "non matching".into();
    non_matching_record.product_query = vec!["renaissance cabinet".try_into().unwrap()];
    non_matching_record.shop_name_query = HashSet::from_iter(["A Different Shop".into()]);

    let expected = index_document(&repository, matching_record).await;
    index_document(&repository, non_matching_record).await;

    let actual = repository
        .percolate(&base_product_document())
        .await
        .unwrap();

    assert_eq!(vec![expected], actual);
}

#[localstack_test(services = [OpenSearch()])]
async fn should_percolate_multiple_documents_when_all_match() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut first_record = base_record();
    first_record.user_search_filter_id = UserSearchFilterId::new();
    first_record.name = "first".into();
    first_record.product_query = vec!["renaissance cabinet".try_into().unwrap()];

    let mut second_record = base_record();
    second_record.user_search_filter_id = UserSearchFilterId::new();
    second_record.name = "second".into();
    second_record.shop_name_query = HashSet::from_iter(["Imperial Antiques".into()]);
    second_record.state_query =
        HashSet::from_iter([product::dynamodb::product_state_record::ProductStateRecord::Listed]);

    let first = index_document(&repository, first_record).await;
    let second = index_document(&repository, second_record).await;

    let actual = repository
        .percolate(&base_product_document())
        .await
        .unwrap();

    assert_eq!(2, actual.len());
    assert!(actual.contains(&first));
    assert!(actual.contains(&second));
}

fn base_record() -> UserSearchFilterRecord {
    let user_id = UserId::new();
    let user_search_filter_id = UserSearchFilterId::new();

    UserSearchFilterRecord {
        pk: format!("user#{user_id}"),
        sk: format!("search_filter#{user_search_filter_id}"),
        user_id,
        user_search_filter_id,
        name: "imperial filter".into(),
        enhanced_search_description: None,
        notifications: true,
        state: ResourceStateRecord::Active,
        product_query: vec!["renaissance cabinet".try_into().unwrap()],
        shop_name_query: HashSet::new(),
        exclude_shop_name_query: HashSet::new(),
        seller_name_query: HashSet::new(),
        exclude_seller_name_query: HashSet::new(),
        shop_slug_id_query: HashSet::new(),
        exclude_shop_slug_id_query: HashSet::new(),
        seller_slug_id_query: HashSet::new(),
        exclude_seller_slug_id_query: HashSet::new(),
        shop_type_query: HashSet::new(),
        country_query: HashSet::new(),
        continent_query: HashSet::new(),
        geo_address_distance_query: None,
        price_query: None,
        state_query: HashSet::new(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
        language: common::language::record::LanguageRecord::En,
        currency: common::currency::record::CurrencyRecord::Eur,
        created_by: common::actor::record::ActorRecord::System,
        updated_by: common::actor::record::ActorRecord::System,
        created: datetime!(2024-01-01 00:00:00 UTC),
        updated: datetime!(2024-01-02 00:00:00 UTC),
        last_hybrid_search_matched: datetime!(2024-01-02 00:00:00 UTC),
    }
}

fn unique_document() -> UserSearchFilterDocument {
    let mut document = exact_document();
    document.user_search_filter_id = UserSearchFilterId::new();
    document.user_id = UserId::new();
    document
}

fn embedding(slot: usize) -> Vec<f32> {
    let mut embedding = vec![0.0_f32; 768];
    embedding[slot] = 1.0;
    embedding
}

fn exact_document() -> UserSearchFilterDocument {
    let mut record = base_record();
    record.user_search_filter_id = UserSearchFilterId::new();
    record.pk = format!("user#{}", record.user_id);
    record.sk = format!("search_filter#{}", record.user_search_filter_id);
    record.created = datetime!(2024-01-03 00:00:00 UTC);
    record.updated = datetime!(2024-01-04 00:00:00 UTC);
    record.try_into().unwrap()
}

fn base_product_document() -> ProductDocument {
    let shop_id = ShopId::new();
    ProductDocument {
        product_id: ProductId::new(),
        product_slug_id: ProductSlugId::from("product"),
        shop_slug_id: ShopSlugId::from("imperial-antiques"),
        seller_slug_id: SellerSlugId::from("imperial-antiques"),
        event_id: EventId::new(),
        shop_id,
        seller_id: shop_id,
        shops_product_id: ShopsProductId::from("shop-item-1"),
        shop_name: "Imperial Antiques".to_string(),
        seller_name: "Imperial Antiques".to_string(),
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
            text: "Cabinet renaissance impérial".to_string(),
            language: LanguageDocument::Fr,
        },
        title_de: Some("Renaissance Schrank".to_string()),
        title_en: Some("Renaissance cabinet".to_string()),
        title_fr: Some("Cabinet renaissance".to_string()),
        title_es: Some("Armario renacentista".to_string()),
        title_it: Some("Cabinet rinascimentale".to_string()),
        price_eur: Some(150),
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
        url: Url::parse("https://example.com/products/renaissance-cabinet").unwrap(),
        view_url: Url::parse("https://example.com/products/renaissance-cabinet?utm_source=aura_historia&utm_medium=referral").unwrap(),
        images: Default::default(),
        embedding: None,
        auction_start: Some(datetime!(2024-01-20 10:00:00 UTC)),
        auction_end: Some(datetime!(2024-01-25 18:00:00 UTC)),
        created_by: common::actor::document::ActorDocument::System,
        updated_by: common::actor::document::ActorDocument::System,
        created: datetime!(2024-01-10 08:00:00 UTC),
        updated: datetime!(2024-01-15 09:00:00 UTC),
    }
}

async fn index_document(
    repository: &UserSearchFilterOpenSearchRepositoryImpl<'_>,
    record: UserSearchFilterRecord,
) -> UserSearchFilterDocument {
    let document: UserSearchFilterDocument = record.try_into().unwrap();
    repository.index_document(document.clone()).await.unwrap();
    refresh_index("user_search_filters").await;
    document
}

// =============================================================================
// Helpers for text-query-only percolation tests
// =============================================================================

/// Returns a filter record with no price, shop, or other structural filters.
/// Matching is driven purely by `product_query` text.
/// Set `product_query` and `language` in each individual test case.
fn base_query_record() -> UserSearchFilterRecord {
    let user_id = UserId::new();
    let user_search_filter_id = UserSearchFilterId::new();
    UserSearchFilterRecord {
        pk: format!("user#{user_id}"),
        sk: format!("search_filter#{user_search_filter_id}"),
        user_id,
        user_search_filter_id,
        name: "text-only filter".into(),
        enhanced_search_description: None,
        notifications: true,
        state: ResourceStateRecord::Active,
        product_query: Vec::new(),
        shop_name_query: HashSet::new(),
        exclude_shop_name_query: HashSet::new(),
        seller_name_query: HashSet::new(),
        exclude_seller_name_query: HashSet::new(),
        shop_slug_id_query: HashSet::new(),
        exclude_shop_slug_id_query: HashSet::new(),
        seller_slug_id_query: HashSet::new(),
        exclude_seller_slug_id_query: HashSet::new(),
        shop_type_query: HashSet::new(),
        country_query: HashSet::new(),
        continent_query: HashSet::new(),
        geo_address_distance_query: None,
        price_query: None,
        state_query: HashSet::new(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
        language: LanguageRecord::En,
        currency: CurrencyRecord::Eur,
        created_by: common::actor::record::ActorRecord::System,
        updated_by: common::actor::record::ActorRecord::System,
        created: datetime!(2024-01-01 00:00:00 UTC),
        updated: datetime!(2024-01-02 00:00:00 UTC),
        last_hybrid_search_matched: datetime!(2024-01-02 00:00:00 UTC),
    }
}

// =============================================================================
// Product 1: Victorian Sterling Silver Tea Service  (London, 1872)
// =============================================================================

fn silver_tea_set_product_document() -> ProductDocument {
    let shop_id = ShopId::new();
    ProductDocument {
        product_id: ProductId::new(),
        product_slug_id: ProductSlugId::from("victorian-silver-tea-service"),
        shop_slug_id: ShopSlugId::from("silver-heirlooms-gallery"),
        seller_slug_id: SellerSlugId::from("silver-heirlooms-gallery"),
        event_id: EventId::new(),
        shop_id,
        seller_id: shop_id,
        shops_product_id: ShopsProductId::from("sts-001"),
        shop_name: "Silver Heirlooms Gallery".to_string(),
        seller_name: "Silver Heirlooms Gallery".to_string(),
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
            text: "Victorian sterling silver tea service London 1872".to_string(),
            language: LanguageDocument::En,
        },
        // Each title is kept concise so the relevant tokens are easy to reason
        // about across language-specific analyzers.
        title_de: Some("Viktorianisches Sterling-Silber-Teeservice London 1872".to_string()),
        title_en: Some("Victorian sterling silver tea service London 1872".to_string()),
        title_fr: Some("Service argent sterling victorien Londres 1872".to_string()),
        title_es: Some("Servicio de té plata esterlina victoriano Londres 1872".to_string()),
        title_it: Some("Servizio da tè argento sterling vittoriano Londra 1872".to_string()),
        price_eur: Some(3_200),
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
        url: Url::parse("https://example.com/products/victorian-silver-tea-service").unwrap(),
        view_url: Url::parse("https://example.com/products/victorian-silver-tea-service?utm_source=aura_historia&utm_medium=referral").unwrap(),
        images: Default::default(),
        embedding: None,
        auction_start: None,
        auction_end: None,
        created_by: common::actor::document::ActorDocument::System,
        updated_by: common::actor::document::ActorDocument::System,
        created: datetime!(2024-02-01 09:30:00 UTC),
        updated: datetime!(2024-02-03 11:15:00 UTC),
    }
}

/// Every query uses words that appear literally in the language-specific title of
/// the silver tea service; because both the percolator-stored query and the
/// percolated document run through the same language analyzer, the text relevance
/// clause can satisfy its `minimum_should_match` requirement.
#[rstest::rstest]
#[case::en_full_phrase("Victorian sterling silver tea service", LanguageRecord::En)]
#[case::de_hyphen_split_compound(
    // Standard tokenizer splits "Sterling-Silber-Teeservice" on hyphens, so
    // querying the three parts individually matches the German title.
    "Sterling Silber Teeservice",
    LanguageRecord::De
)]
#[case::fr_key_nouns(
    // French stop words (à, en, de…) are absent; the remaining nouns match.
    "argent sterling victorien",
    LanguageRecord::Fr
)]
#[localstack_test(services = [OpenSearch()])]
#[tokio::test]
async fn should_percolate_victorian_silver_tea_set_when_query_matches(
    #[case] query: &'static str,
    #[case] language: LanguageRecord,
) {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_query_record();
    record.product_query = vec![query.try_into().unwrap()];
    record.language = language;

    let expected = index_document(&repository, record).await;
    let actual = repository
        .percolate(&silver_tea_set_product_document())
        .await
        .unwrap();

    assert_eq!(vec![expected], actual);
}

/// Queries about entirely different antiques — Chinese porcelain, Renaissance
/// woodwork, Rococo seating — share no title tokens with the silver tea service,
/// so the text relevance clause cannot satisfy its `minimum_should_match` requirement.
#[rstest::rstest]
#[case::en_chinese_ceramic("Ming dynasty blue white porcelain vase", LanguageRecord::En)]
#[case::de_chinese_ceramic("Blau-Weiß-Porzellan Ming Drachen", LanguageRecord::De)]
#[case::fr_chinese_ceramic("porcelaine Ming bleu dragon", LanguageRecord::Fr)]
#[localstack_test(services = [OpenSearch()])]
#[tokio::test]
async fn should_not_percolate_victorian_silver_tea_set_when_query_does_not_match(
    #[case] query: &'static str,
    #[case] language: LanguageRecord,
) {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_query_record();
    record.product_query = vec![query.try_into().unwrap()];
    record.language = language;

    index_document(&repository, record).await;

    let actual = repository
        .percolate(&silver_tea_set_product_document())
        .await
        .unwrap();

    assert!(actual.is_empty());
}

// =============================================================================
// Product 2: Ming Dynasty Blue-and-White Porcelain Vase, Jiajing Period
// =============================================================================

fn ming_vase_product_document() -> ProductDocument {
    let shop_id = ShopId::new();
    ProductDocument {
        product_id: ProductId::new(),
        product_slug_id: ProductSlugId::from("ming-dynasty-blue-white-vase"),
        shop_slug_id: ShopSlugId::from("oriental-antiquities"),
        seller_slug_id: SellerSlugId::from("oriental-antiquities"),
        event_id: EventId::new(),
        shop_id,
        seller_id: shop_id,
        shops_product_id: ShopsProductId::from("mvp-001"),
        shop_name: "Oriental Antiquities Ltd.".to_string(),
        seller_name: "Oriental Antiquities Ltd.".to_string(),
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
            text: "Ming dynasty blue white porcelain vase Jiajing period dragon cloud".to_string(),
            language: LanguageDocument::En,
        },
        title_de: Some("Ming-Dynastie Blau-Weiß Porzellan Vase Jiajing Drachen Wolken".to_string()),
        title_en: Some(
            "Ming dynasty blue white porcelain vase Jiajing period dragon cloud".to_string(),
        ),
        title_fr: Some("Vase Ming porcelaine bleu blanc Jiajing dragon nuages".to_string()),
        title_es: Some("Vaso Ming porcelana azul blanco Jiajing dragón nubes".to_string()),
        title_it: Some("Vaso Ming porcellana bianco blu Jiajing drago nuvole".to_string()),
        price_eur: Some(12_000),
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
        url: Url::parse("https://example.com/products/ming-dynasty-blue-white-vase").unwrap(),
        view_url: Url::parse("https://example.com/products/ming-dynasty-blue-white-vase?utm_source=aura_historia&utm_medium=referral").unwrap(),
        images: Default::default(),
        embedding: None,
        auction_start: None,
        auction_end: None,
        created_by: common::actor::document::ActorDocument::System,
        updated_by: common::actor::document::ActorDocument::System,
        created: datetime!(2024-03-10 10:00:00 UTC),
        updated: datetime!(2024-03-12 14:00:00 UTC),
    }
}

/// All query terms appear verbatim in the language-specific title of the Ming vase.
/// German note: standard tokenizer splits "Blau-Weiß" on the hyphen and
/// `german_normalization` converts ß→ss, so "Weiß" in both query and title
/// normalises to "weiss" before light-German stemming.
#[rstest::rstest]
#[case::en_full_phrase("Ming dynasty blue white porcelain vase", LanguageRecord::En)]
#[case::de_colour_material_period(
    // "Weiß" normalises to "weiss"; all resulting tokens are in the German title.
    "Blau Weiß Porzellan Jiajing",
    LanguageRecord::De
)]
#[case::fr_origin_period_and_motif("Ming porcelaine Jiajing dragon", LanguageRecord::Fr)]
#[localstack_test(services = [OpenSearch()])]
#[tokio::test]
async fn should_percolate_ming_dynasty_vase_when_query_matches(
    #[case] query: &'static str,
    #[case] language: LanguageRecord,
) {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_query_record();
    record.product_query = vec![query.try_into().unwrap()];
    record.language = language;

    let expected = index_document(&repository, record).await;
    let actual = repository
        .percolate(&ming_vase_product_document())
        .await
        .unwrap();

    assert_eq!(vec![expected], actual);
}

#[localstack_test(services = [OpenSearch()])]
async fn should_percolate_ming_dynasty_vase_when_long_query_misses_one_detail() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_query_record();
    record.product_query = vec![
        "Ming dynasty blue white porcelain vase dragon mark"
            .try_into()
            .unwrap(),
    ];
    record.language = LanguageRecord::En;

    let expected = index_document(&repository, record).await;
    let actual = repository
        .percolate(&ming_vase_product_document())
        .await
        .unwrap();

    assert_eq!(vec![expected], actual);
}

/// Queries about Victorian silverware, Rococo seating, and Baroque metalwork
/// contain no terms present in the Ming vase title, so the text relevance clause fails.
#[rstest::rstest]
#[case::en_silver_tea("Victorian sterling silver tea service", LanguageRecord::En)]
#[case::de_silver_tea("Sterling Silber Teeservice viktorianisch", LanguageRecord::De)]
#[case::fr_silver_tea("argent sterling victorien service", LanguageRecord::Fr)]
#[localstack_test(services = [OpenSearch()])]
#[tokio::test]
async fn should_not_percolate_ming_dynasty_vase_when_query_does_not_match(
    #[case] query: &'static str,
    #[case] language: LanguageRecord,
) {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_query_record();
    record.product_query = vec![query.try_into().unwrap()];
    record.language = language;

    index_document(&repository, record).await;

    let actual = repository
        .percolate(&ming_vase_product_document())
        .await
        .unwrap();

    assert!(actual.is_empty());
}

// =============================================================================
// Product 3: Louis XV Carved Walnut Fauteuil with Aubusson Tapestry, c. 1750
// =============================================================================

fn louis_xv_fauteuil_product_document() -> ProductDocument {
    let shop_id = ShopId::new();
    ProductDocument {
        product_id: ProductId::new(),
        product_slug_id: ProductSlugId::from("louis-xv-walnut-fauteuil-aubusson"),
        shop_slug_id: ShopSlugId::from("maison-des-antiquites"),
        seller_slug_id: SellerSlugId::from("maison-des-antiquites"),
        event_id: EventId::new(),
        shop_id,
        seller_id: shop_id,
        shops_product_id: ShopsProductId::from("lxv-001"),
        shop_name: "Maison des Antiquités".to_string(),
        seller_name: "Maison des Antiquités".to_string(),
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
            text: "Louis XV carved walnut fauteuil Aubusson tapestry Rococo 1750".to_string(),
            language: LanguageDocument::En,
        },
        title_de: Some(
            "Ludwig XV Nussbaum Fauteuil geschnitzt Aubusson Tapisserie Rokoko 1750".to_string(),
        ),
        title_en: Some("Louis XV carved walnut fauteuil Aubusson tapestry Rococo 1750".to_string()),
        title_fr: Some("Louis XV fauteuil noyer Aubusson tapisserie Rococo 1750".to_string()),
        title_es: Some("Luis XV fauteuil nogal Aubusson tapicería Rococo 1750".to_string()),
        title_it: Some("Luigi XV fauteuil noce Aubusson tappezzeria Rococo 1750".to_string()),
        price_eur: Some(4_500),
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
        url: Url::parse("https://example.com/products/louis-xv-walnut-fauteuil").unwrap(),
        view_url: Url::parse("https://example.com/products/louis-xv-walnut-fauteuil?utm_source=aura_historia&utm_medium=referral").unwrap(),
        images: Default::default(),
        embedding: None,
        auction_start: None,
        auction_end: None,
        created_by: common::actor::document::ActorDocument::System,
        updated_by: common::actor::document::ActorDocument::System,
        created: datetime!(2024-03-15 10:00:00 UTC),
        updated: datetime!(2024-03-18 14:00:00 UTC),
    }
}

/// All query terms exist verbatim in the language-specific title so they produce
/// the same stems during both percolator indexing and document percolation.
/// French note: "Louis", "Aubusson", and "Rococo" are proper nouns / loanwords
/// that survive the French light-stemmer unchanged.
#[rstest::rstest]
#[case::en_full_phrase("Louis XV carved walnut fauteuil", LanguageRecord::En)]
#[case::de_material_and_upholstery("Nussbaum Fauteuil Aubusson", LanguageRecord::De)]
#[case::fr_period_and_upholstery(
    // French stop words (de, la, en…) absent; proper nouns and loanwords match.
    "Louis fauteuil Aubusson Rococo",
    LanguageRecord::Fr
)]
#[localstack_test(services = [OpenSearch()])]
#[tokio::test]
async fn should_percolate_louis_xv_fauteuil_when_query_matches(
    #[case] query: &'static str,
    #[case] language: LanguageRecord,
) {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_query_record();
    record.product_query = vec![query.try_into().unwrap()];
    record.language = language;

    let expected = index_document(&repository, record).await;
    let actual = repository
        .percolate(&louis_xv_fauteuil_product_document())
        .await
        .unwrap();

    assert_eq!(vec![expected], actual);
}

/// Queries about Chinese ceramics, Victorian silverware, and Baroque metalwork
/// have no overlap with the Louis XV fauteuil title in any language.
#[rstest::rstest]
#[case::en_chinese_ceramic("Ming dynasty blue white porcelain vase", LanguageRecord::En)]
#[case::de_chinese_ceramic("Blau Weiß Porzellan Ming Drachen", LanguageRecord::De)]
#[case::fr_chinese_ceramic("Ming porcelaine bleu dragon Jiajing", LanguageRecord::Fr)]
#[localstack_test(services = [OpenSearch()])]
#[tokio::test]
async fn should_not_percolate_louis_xv_fauteuil_when_query_does_not_match(
    #[case] query: &'static str,
    #[case] language: LanguageRecord,
) {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_query_record();
    record.product_query = vec![query.try_into().unwrap()];
    record.language = language;

    index_document(&repository, record).await;

    let actual = repository
        .percolate(&louis_xv_fauteuil_product_document())
        .await
        .unwrap();

    assert!(actual.is_empty());
}
