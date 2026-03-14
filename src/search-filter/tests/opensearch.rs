use common::category_key::CategoryId;
use common::event_id::EventId;
use common::language::document::{LanguageDocument, TextDocument};
use common::period_key::PeriodId;
use common::product_id::ProductId;
use common::query::range_query::RangeQuery;
use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;
use common::slug_id::SlugId;
use common::user_id::UserId;
use common::year::Year;
use fake::{Fake, Faker};
use opensearch::http::Url;
use product::opensearch::product_document::ProductDocument;
use product::opensearch::product_state_document::ProductStateDocument;
use search_filter::core::user_search_filter_id::UserSearchFilterId;
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

    let response = repository
        .index_document(expected.clone())
        .await
        .unwrap();

    assert_eq!(response.id, expected.user_search_filter_id.to_string());

    refresh_index("user_search_filter").await;

    let actual: UserSearchFilterDocument =
        read_by_id("user_search_filter", expected.user_search_filter_id).await;

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

    refresh_index("user_search_filter").await;

    let actual: UserSearchFilterDocument =
        read_by_id("user_search_filter", second.user_search_filter_id).await;

    assert_eq!(second, actual);
}

#[localstack_test(services = [OpenSearch()])]
async fn should_delete_user_search_filter_document() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let document = exact_document();
    repository.index_document(document.clone()).await.unwrap();
    refresh_index("user_search_filter").await;

    let response = repository
        .delete_document(&document.user_search_filter_id)
        .await
        .unwrap();

    assert_eq!(response.id, document.user_search_filter_id.to_string());

    refresh_index("user_search_filter").await;

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
    record.product_query = None;
    record.category_id.clear();
    record.period_id.clear();
    record.shop_name_query.clear();
    record.exclude_shop_name_query.clear();
    record.shop_type_query.clear();
    record.price_query = None;
    record.state_query.clear();
    record.created_query = None;
    record.updated_query = None;
    record.auction_start_query = None;
    record.auction_end_query = None;
    record.origin_year_query = None;
    record.authenticity_query.clear();
    record.condition_query.clear();
    record.provenance_query.clear();
    record.restoration_query.clear();

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
    record.product_query = Some("renaissance cabinet".try_into().unwrap());
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
    record.product_query = Some("completely unrelated phrase".try_into().unwrap());

    index_document(&repository, record).await;

    let actual = repository
        .percolate(&base_product_document())
        .await
        .unwrap();

    assert!(actual.is_empty());
}

#[localstack_test(services = [OpenSearch()])]
async fn should_percolate_document_when_category_matches() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_record();
    record.product_query = None;
    record.category_id = HashSet::from_iter([CategoryId::from("furniture")]);

    let expected = index_document(&repository, record).await;
    let actual = repository
        .percolate(&base_product_document())
        .await
        .unwrap();

    assert_eq!(vec![expected], actual);
}

#[localstack_test(services = [OpenSearch()])]
async fn should_not_percolate_document_when_category_does_not_match() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_record();
    record.product_query = None;
    record.category_id = HashSet::from_iter([CategoryId::from("ceramics")]);

    index_document(&repository, record).await;

    let actual = repository
        .percolate(&base_product_document())
        .await
        .unwrap();

    assert!(actual.is_empty());
}

#[localstack_test(services = [OpenSearch()])]
async fn should_percolate_document_when_period_matches() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_record();
    record.product_query = None;
    record.period_id = HashSet::from_iter([PeriodId::from("baroque")]);

    let expected = index_document(&repository, record).await;
    let actual = repository
        .percolate(&base_product_document())
        .await
        .unwrap();

    assert_eq!(vec![expected], actual);
}

#[localstack_test(services = [OpenSearch()])]
async fn should_not_percolate_document_when_period_does_not_match() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_record();
    record.product_query = None;
    record.period_id = HashSet::from_iter([PeriodId::from("modernism")]);

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
    record.product_query = None;
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
    record.product_query = None;
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
    record.product_query = None;
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
    record.product_query = None;
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
    record.product_query = None;
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
    record.product_query = None;
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
    record.product_query = None;
    record.state_query = HashSet::from_iter([product::dynamodb::product_state_record::ProductStateRecord::Listed]);

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
    record.product_query = None;
    record.state_query = HashSet::from_iter([product::dynamodb::product_state_record::ProductStateRecord::Sold]);

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
    record.product_query = None;
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
    record.product_query = None;
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
    record.product_query = None;
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
    record.product_query = None;
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
    record.product_query = None;
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
    record.product_query = None;
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
    record.product_query = None;
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
    record.product_query = None;
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
async fn should_percolate_document_when_origin_year_exactly_matches_stored_exact_year() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_record();
    record.product_query = None;
    record.origin_year_query = Some(RangeQuery {
        min: Some(Year::from(1780)),
        max: Some(Year::from(1780)),
    });

    let expected = index_document(&repository, record).await;
    let actual = repository
        .percolate(&base_product_document())
        .await
        .unwrap();

    assert_eq!(vec![expected], actual);
}

#[localstack_test(services = [OpenSearch()])]
async fn should_not_percolate_document_when_origin_year_exact_does_not_match_stored_exact_year() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_record();
    record.product_query = None;
    record.origin_year_query = Some(RangeQuery {
        min: Some(Year::from(1781)),
        max: Some(Year::from(1781)),
    });

    index_document(&repository, record).await;

    let actual = repository
        .percolate(&base_product_document())
        .await
        .unwrap();

    assert!(actual.is_empty());
}

#[localstack_test(services = [OpenSearch()])]
async fn should_percolate_document_when_origin_year_min_matches_stored_year_range() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_record();
    record.product_query = None;
    record.origin_year_query = Some(RangeQuery {
        min: Some(Year::from(1770)),
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
async fn should_not_percolate_document_when_origin_year_min_exceeds_stored_year_range() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_record();
    record.product_query = None;
    record.origin_year_query = Some(RangeQuery {
        min: Some(Year::from(1810)),
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
async fn should_percolate_document_when_origin_year_max_matches_stored_year_range() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_record();
    record.product_query = None;
    record.origin_year_query = Some(RangeQuery {
        min: None,
        max: Some(Year::from(1790)),
    });

    let expected = index_document(&repository, record).await;
    let actual = repository
        .percolate(&base_product_document())
        .await
        .unwrap();

    assert_eq!(vec![expected], actual);
}

#[localstack_test(services = [OpenSearch()])]
async fn should_not_percolate_document_when_origin_year_max_is_before_stored_year_range() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_record();
    record.product_query = None;
    record.origin_year_query = Some(RangeQuery {
        min: None,
        max: Some(Year::from(1700)),
    });

    index_document(&repository, record).await;

    let actual = repository
        .percolate(&base_product_document())
        .await
        .unwrap();

    assert!(actual.is_empty());
}

#[localstack_test(services = [OpenSearch()])]
async fn should_percolate_document_when_origin_year_range_overlaps_stored_year_range() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_record();
    record.product_query = None;
    record.origin_year_query = Some(RangeQuery {
        min: Some(Year::from(1775)),
        max: Some(Year::from(1785)),
    });

    let expected = index_document(&repository, record).await;
    let actual = repository
        .percolate(&base_product_document())
        .await
        .unwrap();

    assert_eq!(vec![expected], actual);
}

#[localstack_test(services = [OpenSearch()])]
async fn should_not_percolate_document_when_origin_year_range_does_not_overlap_stored_year_range() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_record();
    record.product_query = None;
    record.origin_year_query = Some(RangeQuery {
        min: Some(Year::from(1600)),
        max: Some(Year::from(1700)),
    });

    index_document(&repository, record).await;

    let actual = repository
        .percolate(&base_product_document())
        .await
        .unwrap();

    assert!(actual.is_empty());
}

#[localstack_test(services = [OpenSearch()])]
async fn should_percolate_document_when_authenticity_matches() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_record();
    record.product_query = None;
    record.authenticity_query =
        HashSet::from_iter([product::dynamodb::authenticity_record::AuthenticityRecord::Original]);

    let expected = index_document(&repository, record).await;
    let actual = repository
        .percolate(&base_product_document())
        .await
        .unwrap();

    assert_eq!(vec![expected], actual);
}

#[localstack_test(services = [OpenSearch()])]
async fn should_not_percolate_document_when_authenticity_does_not_match() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_record();
    record.product_query = None;
    record.authenticity_query =
        HashSet::from_iter([product::dynamodb::authenticity_record::AuthenticityRecord::Questionable]);

    index_document(&repository, record).await;

    let actual = repository
        .percolate(&base_product_document())
        .await
        .unwrap();

    assert!(actual.is_empty());
}

#[localstack_test(services = [OpenSearch()])]
async fn should_percolate_document_when_condition_matches() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_record();
    record.product_query = None;
    record.condition_query =
        HashSet::from_iter([product::dynamodb::condition_record::ConditionRecord::Good]);

    let expected = index_document(&repository, record).await;
    let actual = repository
        .percolate(&base_product_document())
        .await
        .unwrap();

    assert_eq!(vec![expected], actual);
}

#[localstack_test(services = [OpenSearch()])]
async fn should_not_percolate_document_when_condition_does_not_match() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_record();
    record.product_query = None;
    record.condition_query =
        HashSet::from_iter([product::dynamodb::condition_record::ConditionRecord::Poor]);

    index_document(&repository, record).await;

    let actual = repository
        .percolate(&base_product_document())
        .await
        .unwrap();

    assert!(actual.is_empty());
}

#[localstack_test(services = [OpenSearch()])]
async fn should_percolate_document_when_provenance_matches() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_record();
    record.product_query = None;
    record.provenance_query =
        HashSet::from_iter([product::dynamodb::provenance_record::ProvenanceRecord::Complete]);

    let expected = index_document(&repository, record).await;
    let actual = repository
        .percolate(&base_product_document())
        .await
        .unwrap();

    assert_eq!(vec![expected], actual);
}

#[localstack_test(services = [OpenSearch()])]
async fn should_not_percolate_document_when_provenance_does_not_match() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_record();
    record.product_query = None;
    record.provenance_query =
        HashSet::from_iter([product::dynamodb::provenance_record::ProvenanceRecord::None]);

    index_document(&repository, record).await;

    let actual = repository
        .percolate(&base_product_document())
        .await
        .unwrap();

    assert!(actual.is_empty());
}

#[localstack_test(services = [OpenSearch()])]
async fn should_percolate_document_when_restoration_matches() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_record();
    record.product_query = None;
    record.restoration_query =
        HashSet::from_iter([product::dynamodb::restoration_record::RestorationRecord::Major]);

    let expected = index_document(&repository, record).await;
    let actual = repository
        .percolate(&base_product_document())
        .await
        .unwrap();

    assert_eq!(vec![expected], actual);
}

#[localstack_test(services = [OpenSearch()])]
async fn should_not_percolate_document_when_restoration_does_not_match() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_record();
    record.product_query = None;
    record.restoration_query =
        HashSet::from_iter([product::dynamodb::restoration_record::RestorationRecord::None]);

    index_document(&repository, record).await;

    let actual = repository
        .percolate(&base_product_document())
        .await
        .unwrap();

    assert!(actual.is_empty());
}

#[localstack_test(services = [OpenSearch()])]
async fn should_percolate_document_when_created_is_within_min_and_max_range() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_record();
    record.product_query = None;
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
    record.product_query = None;
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
    record.product_query = None;
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
    record.product_query = None;
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
    record.product_query = None;
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
    record.product_query = None;
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
    record.product_query = None;
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
    record.product_query = None;
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
async fn should_not_percolate_document_when_auction_start_query_is_given_but_product_has_no_auction_start() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut product = base_product_document();
    product.auction_start = None;

    let mut record = base_record();
    record.product_query = None;
    record.auction_start_query = Some(RangeQuery {
        min: Some(datetime!(2024-01-20 00:00:00 UTC)),
        max: Some(datetime!(2024-01-21 00:00:00 UTC)),
    });

    index_document(&repository, record).await;

    let actual = repository.percolate(&product).await.unwrap();

    assert!(actual.is_empty());
}

#[localstack_test(services = [OpenSearch()])]
async fn should_not_percolate_document_when_auction_end_query_is_given_but_product_has_no_auction_end() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut product = base_product_document();
    product.auction_end = None;

    let mut record = base_record();
    record.product_query = None;
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
    matching_record.product_query = Some("renaissance".try_into().unwrap());
    matching_record.category_id = HashSet::from_iter([CategoryId::from("furniture")]);
    matching_record.shop_name_query = HashSet::from_iter(["Imperial Antiques".into()]);
    matching_record.price_query = Some(RangeQuery {
        min: Some(100),
        max: Some(200),
    });

    let mut non_matching_record = base_record();
    non_matching_record.user_search_filter_id = UserSearchFilterId::new();
    non_matching_record.name = "non matching".into();
    non_matching_record.product_query = Some("renaissance".try_into().unwrap());
    non_matching_record.category_id = HashSet::from_iter([CategoryId::from("ceramics")]);
    non_matching_record.shop_name_query = HashSet::from_iter(["Imperial Antiques".into()]);

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
    first_record.product_query = Some("renaissance".try_into().unwrap());
    first_record.category_id = HashSet::from_iter([CategoryId::from("furniture")]);

    let mut second_record = base_record();
    second_record.user_search_filter_id = UserSearchFilterId::new();
    second_record.name = "second".into();
    second_record.product_query = None;
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
        product_query: Some("renaissance".try_into().unwrap()),
        category_id: HashSet::from_iter([CategoryId::from("furniture")]),
        period_id: HashSet::from_iter([PeriodId::from("baroque")]),
        shop_name_query: HashSet::new(),
        exclude_shop_name_query: HashSet::new(),
        shop_type_query: HashSet::new(),
        price_query: None,
        state_query: HashSet::new(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
        origin_year_query: None,
        authenticity_query: HashSet::new(),
        condition_query: HashSet::new(),
        provenance_query: HashSet::new(),
        restoration_query: HashSet::new(),
        language: common::language::record::LanguageRecord::En,
        currency: common::currency::record::CurrencyRecord::Eur,
        created: datetime!(2024-01-01 00:00:00 UTC),
        updated: datetime!(2024-01-02 00:00:00 UTC),
    }
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
    ProductDocument {
        product_id: ProductId::new(),
        product_slug_id: SlugId::from("product"),
        shop_slug_id: SlugId::from("imperial-antiques"),
        event_id: EventId::new(),
        shop_id: ShopId::new(),
        shops_product_id: ShopsProductId::from("shop-item-1"),
        shop_name: "Imperial Antiques".to_string(),
        shop_type: ShopTypeDocument::CommercialDealer,
        category_id: Some(CategoryId::from("furniture")),
        period_id: Some(PeriodId::from("baroque")),
        category_name_de: Some("Möbel".to_string()),
        category_name_en: Some("Furniture".to_string()),
        category_name_fr: Some("Meubles".to_string()),
        category_name_es: Some("Muebles".to_string()),
        category_name_it: Some("Mobili".to_string()),
        period_name_de: Some("Barock".to_string()),
        period_name_en: Some("Baroque".to_string()),
        period_name_fr: Some("Baroque".to_string()),
        period_name_es: Some("Barroco".to_string()),
        period_name_it: Some("Barocco".to_string()),
        title_native: TextDocument {
            text: "Cabinet renaissance impérial".to_string(),
            language: LanguageDocument::Fr,
        },
        title_de: Some("Renaissance Schrank".to_string()),
        title_en: Some("Renaissance cabinet".to_string()),
        title_fr: Some("Cabinet renaissance".to_string()),
        title_es: Some("Armario renacentista".to_string()),
        title_it: Some("Cabinet rinascimentale".to_string()),
        description_de: Some("Großer barocker Schrank aus Nussbaum".to_string()),
        description_en: Some(
            "Large renaissance cabinet with documented provenance and restored details".to_string(),
        ),
        description_fr: Some("Grand cabinet avec provenance documentée".to_string()),
        description_es: Some("Gran armario con procedencia documentada".to_string()),
        description_it: Some("Grande cabinet con provenienza documentata".to_string()),
        price_eur: Some(150),
        price_usd: None,
        price_gbp: None,
        price_aud: None,
        price_cad: None,
        price_nzd: None,
        price_estimate_min_eur: None,
        price_estimate_min_usd: None,
        price_estimate_min_gbp: None,
        price_estimate_min_aud: None,
        price_estimate_min_cad: None,
        price_estimate_min_nzd: None,
        price_estimate_max_eur: None,
        price_estimate_max_usd: None,
        price_estimate_max_gbp: None,
        price_estimate_max_aud: None,
        price_estimate_max_cad: None,
        price_estimate_max_nzd: None,
        state: ProductStateDocument::Listed,
        url: Url::parse("https://example.com/products/renaissance-cabinet").unwrap(),
        images: Faker.fake(),
        text_embedding: None,
        origin_year_min: Some(Year::from(1770)),
        origin_year: Some(Year::from(1780)),
        origin_year_max: Some(Year::from(1790)),
        authenticity: product::opensearch::authenticity_document::AuthenticityDocument::Original,
        condition: product::opensearch::condition_document::ConditionDocument::Good,
        provenance: product::opensearch::provenance_document::ProvenanceDocument::Complete,
        restoration: product::opensearch::restoration_document::RestorationDocument::Major,
        auction_start: Some(datetime!(2024-01-20 10:00:00 UTC)),
        auction_end: Some(datetime!(2024-01-25 18:00:00 UTC)),
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
    refresh_index("user_search_filter").await;
    document
}