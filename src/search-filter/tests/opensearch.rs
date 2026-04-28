use common::category_key::CategoryId;
use common::currency::record::CurrencyRecord;
use common::event_id::EventId;
use common::language::document::{LanguageDocument, TextDocument};
use common::language::record::LanguageRecord;
use common::period_key::PeriodId;
use common::product_id::ProductId;
use common::query::range_query::RangeQuery;
use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;
use common::slug_id::SlugId;
use common::user_id::UserId;
use common::user_search_filter_id::UserSearchFilterId;
use common::year::Year;
use fake::{Fake, Faker};
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
    record.category_id.clear();
    record.period_id.clear();
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
async fn should_percolate_document_when_origin_year_exactly_matches_stored_exact_year() {
    let client = get_opensearch_client().await;
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(client);

    let mut record = base_record();
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
    record.authenticity_query = HashSet::from_iter([
        product::dynamodb::authenticity_record::AuthenticityRecord::Questionable,
    ]);

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
    matching_record.product_query = Some("renaissance cabinet".try_into().unwrap());
    matching_record.category_id = HashSet::from_iter([CategoryId::from("furniture")]);
    matching_record.shop_name_query = HashSet::from_iter(["Imperial Antiques".into()]);
    matching_record.price_query = Some(RangeQuery {
        min: Some(100),
        max: Some(200),
    });

    let mut non_matching_record = base_record();
    non_matching_record.user_search_filter_id = UserSearchFilterId::new();
    non_matching_record.name = "non matching".into();
    non_matching_record.product_query = Some("renaissance cabinet".try_into().unwrap());
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
    first_record.product_query = Some("renaissance cabinet".try_into().unwrap());
    first_record.category_id = HashSet::from_iter([CategoryId::from("furniture")]);

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
        product_query: Some("renaissance cabinet".try_into().unwrap()),
        category_id: HashSet::from_iter([CategoryId::from("furniture")]),
        period_id: HashSet::from_iter([PeriodId::from("baroque")]),
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
    let shop_id = ShopId::new();
    ProductDocument {
        product_id: ProductId::new(),
        product_slug_id: SlugId::from("product"),
        shop_slug_id: SlugId::from("imperial-antiques"),
        seller_slug_id: SlugId::from("imperial-antiques"),
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
        images: Faker.fake(),
        embedding: None,
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
    refresh_index("user_search_filters").await;
    document
}

// =============================================================================
// Helpers for text-query-only percolation tests
// =============================================================================

/// Returns a filter record with **no** category, period, price, shop or other
/// structural filters — matching is driven purely by `product_query` text.
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
        product_query: None,
        category_id: HashSet::new(),
        period_id: HashSet::new(),
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
        origin_year_query: None,
        authenticity_query: HashSet::new(),
        condition_query: HashSet::new(),
        provenance_query: HashSet::new(),
        restoration_query: HashSet::new(),
        language: LanguageRecord::En,
        currency: CurrencyRecord::Eur,
        created: datetime!(2024-01-01 00:00:00 UTC),
        updated: datetime!(2024-01-02 00:00:00 UTC),
    }
}

// =============================================================================
// Product 1: Victorian Sterling Silver Tea Service  (London, 1872)
// =============================================================================

fn silver_tea_set_product_document() -> ProductDocument {
    let shop_id = ShopId::new();
    ProductDocument {
        product_id: ProductId::new(),
        product_slug_id: SlugId::from("victorian-silver-tea-service"),
        shop_slug_id: SlugId::from("silver-heirlooms-gallery"),
        seller_slug_id: SlugId::from("silver-heirlooms-gallery"),
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
        category_id: Some(CategoryId::from("silverware")),
        period_id: Some(PeriodId::from("victorian")),
        category_name_de: Some("Silberwaren".to_string()),
        category_name_en: Some("Silverware".to_string()),
        category_name_fr: Some("Argenterie".to_string()),
        category_name_es: Some("Platería".to_string()),
        category_name_it: Some("Argenteria".to_string()),
        period_name_de: Some("Viktorianisch".to_string()),
        period_name_en: Some("Victorian".to_string()),
        period_name_fr: Some("Victorien".to_string()),
        period_name_es: Some("Victoriano".to_string()),
        period_name_it: Some("Vittoriano".to_string()),
        title_native: TextDocument {
            text: "Victorian sterling silver tea service London 1872".to_string(),
            language: LanguageDocument::En,
        },
        // Each title is kept concise so the relevant tokens are easy to reason
        // about across language-specific analyzers.
        title_de: Some("Viktorianisches Sterling-Silber-Teeservice London 1872".to_string()),
        title_en: Some("Victorian sterling silver tea service London 1872".to_string()),
        title_fr: Some("Service argent sterling victorien Londres 1872".to_string()),
        title_es: Some("Servicio plata esterlina victoriano Londres 1872".to_string()),
        title_it: Some("Servizio argento sterling vittoriano Londra 1872".to_string()),
        description_de: Some(
            "Sechsteiliges Sterling-Silber-Teeservice von Goldschmied George Richards, \
             London 1872. Bestehend aus gravierter Teekanne, Heißwasserkrug, Sahnekanne, \
             Zuckerdose und Präsentationstablett. Vollständige Londoner Punzierung."
                .to_string(),
        ),
        description_en: Some(
            "Superb six-piece sterling silver tea service by London silversmith \
             George Richards, hallmarked 1872. Comprising engraved teapot, hot water jug, \
             cream jug, sugar bowl with tongs, and presentation tray. Complete London hallmarks."
                .to_string(),
        ),
        description_fr: Some(
            "Superbe service à thé en argent sterling par l'orfèvre londonien George Richards, \
             poinçonné 1872. Comprenant une théière gravée, pot à eau chaude, pot à crème \
             et sucrier. Poinçons londoniens complets."
                .to_string(),
        ),
        description_es: Some(
            "Soberbio servicio de té en plata esterlina del platero londinense George Richards, \
             punzonado 1872. Con tetera grabada, jarro de agua caliente, lechera y azucarera."
                .to_string(),
        ),
        description_it: Some(
            "Superbo servizio da tè in argento sterling dell'orefice londinese George Richards, \
             punzonato 1872. Comprende teiera incisa, bricco per acqua calda, lattiera e \
             zuccheriera."
                .to_string(),
        ),
        price_eur: None,
        price_usd: None,
        price_gbp: Some(800),
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
        images: Faker.fake(),
        embedding: None,
        origin_year_min: Some(Year::from(1870)),
        origin_year: Some(Year::from(1872)),
        origin_year_max: Some(Year::from(1878)),
        authenticity: product::opensearch::authenticity_document::AuthenticityDocument::Original,
        condition: product::opensearch::condition_document::ConditionDocument::Great,
        provenance: product::opensearch::provenance_document::ProvenanceDocument::Partial,
        restoration: product::opensearch::restoration_document::RestorationDocument::None,
        auction_start: None,
        auction_end: None,
        created: datetime!(2024-03-01 10:00:00 UTC),
        updated: datetime!(2024-03-05 14:00:00 UTC),
    }
}

/// Every query uses words that appear literally in the language-specific title of
/// the silver tea service; because both the percolator-stored query and the
/// percolated document run through the same language analyzer, their stems are
/// identical and the AND-operator `must` clause is always satisfied.
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
    record.product_query = Some(query.try_into().unwrap());
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
/// so the AND-operator `must` clause is never satisfied.
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
    record.product_query = Some(query.try_into().unwrap());
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
        product_slug_id: SlugId::from("ming-dynasty-blue-white-vase"),
        shop_slug_id: SlugId::from("oriental-antiquities"),
        seller_slug_id: SlugId::from("oriental-antiquities"),
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
        category_id: Some(CategoryId::from("ceramics")),
        period_id: Some(PeriodId::from("ming-dynasty")),
        category_name_de: Some("Keramik".to_string()),
        category_name_en: Some("Ceramics".to_string()),
        category_name_fr: Some("Céramique".to_string()),
        category_name_es: Some("Cerámica".to_string()),
        category_name_it: Some("Ceramica".to_string()),
        period_name_de: Some("Ming-Dynastie".to_string()),
        period_name_en: Some("Ming Dynasty".to_string()),
        period_name_fr: Some("Dynastie Ming".to_string()),
        period_name_es: Some("Dinastía Ming".to_string()),
        period_name_it: Some("Dinastia Ming".to_string()),
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
        description_de: Some(
            "Außergewöhnliche blau-weiße Porzellanvase aus der Ming-Dynastie, \
             Jiajing-Periode (1521–1567). Mit fünfklauigem Kaiserdrachen durch stilisierte \
             Wolken. Sechszeichen-Jiajing-Regierungszeichen am Boden. Shanghais Auktionshaus \
             1988, mit Exportzertifikat."
                .to_string(),
        ),
        description_en: Some(
            "Exceptional Ming dynasty blue-and-white porcelain vase from the Jiajing period \
             (1521–1567). The ovoid body is painted with a five-clawed imperial dragon chasing \
             a flaming pearl through stylised clouds. Six-character Jiajing reign mark on base. \
             Provenance: Shanghai auction house, 1988, export certificate included."
                .to_string(),
        ),
        description_fr: Some(
            "Exceptionnel vase en porcelaine bleu et blanc de la période Jiajing de la \
             dynastie Ming (1521–1567). La panse ovoïde est ornée d'un dragon impérial à cinq \
             griffes courant dans des nuages stylisés. Marque de règne Jiajing à six caractères \
             au fond."
                .to_string(),
        ),
        description_es: Some(
            "Excepcional vaso de porcelana azul y blanco del período Jiajing de la \
             dinastía Ming (1521–1567). El cuerpo ovoide está pintado con un dragón imperial \
             de cinco garras persiguiendo una perla llameante entre nubes estilizadas."
                .to_string(),
        ),
        description_it: Some(
            "Eccezionale vaso in porcellana bianco-blu del periodo Jiajing della \
             dinastia Ming (1521–1567). Il corpo ovoide è dipinto con un drago imperiale a \
             cinque artigli tra nuvole stilizzate."
                .to_string(),
        ),
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
        images: Faker.fake(),
        embedding: None,
        origin_year_min: Some(Year::from(1530)),
        origin_year: Some(Year::from(1545)),
        origin_year_max: Some(Year::from(1560)),
        authenticity: product::opensearch::authenticity_document::AuthenticityDocument::Original,
        condition: product::opensearch::condition_document::ConditionDocument::Good,
        provenance: product::opensearch::provenance_document::ProvenanceDocument::Complete,
        restoration: product::opensearch::restoration_document::RestorationDocument::None,
        auction_start: None,
        auction_end: None,
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
    record.product_query = Some(query.try_into().unwrap());
    record.language = language;

    let expected = index_document(&repository, record).await;
    let actual = repository
        .percolate(&ming_vase_product_document())
        .await
        .unwrap();

    assert_eq!(vec![expected], actual);
}

/// Queries about Victorian silverware, Rococo seating, and Baroque metalwork
/// contain no terms present in the Ming vase title, so the must clause fails.
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
    record.product_query = Some(query.try_into().unwrap());
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
        product_slug_id: SlugId::from("louis-xv-walnut-fauteuil-aubusson"),
        shop_slug_id: SlugId::from("maison-des-antiquites"),
        seller_slug_id: SlugId::from("maison-des-antiquites"),
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
        category_id: Some(CategoryId::from("furniture")),
        period_id: Some(PeriodId::from("rococo")),
        category_name_de: Some("Möbel".to_string()),
        category_name_en: Some("Furniture".to_string()),
        category_name_fr: Some("Mobilier".to_string()),
        category_name_es: Some("Muebles".to_string()),
        category_name_it: Some("Mobili".to_string()),
        period_name_de: Some("Rokoko".to_string()),
        period_name_en: Some("Rococo".to_string()),
        period_name_fr: Some("Rococo".to_string()),
        period_name_es: Some("Rococó".to_string()),
        period_name_it: Some("Rococò".to_string()),
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
        description_de: Some(
            "Eleganter Nussbaum-Fauteuil aus der Ludwig-XV-Periode mit Cabriole-Beinen, \
             um 1750. Sitzgestell und Rücken mit Blumen- und Muschelmotiven im Rokoko-Stil \
             geschnitzt. Mit originaler Aubusson-Tapisserie, die Blumensträuße auf \
             cremefarbenem Grund zeigt."
                .to_string(),
        ),
        description_en: Some(
            "Elegant Louis XV period carved walnut fauteuil with cabriole legs, circa 1750. \
             The serpentine seat rail and shaped back are carved with floral and shell motifs \
             in the Rococo style. Upholstered in original Aubusson tapestry depicting floral \
             bouquets on a cream ground. Stampmaker's mark on rear seat rail."
                .to_string(),
        ),
        description_fr: Some(
            "Élégant fauteuil en noyer sculpté de la période Louis XV aux pieds cambrés, \
             vers 1750. La ceinture serpentine et le dossier sont sculptés de motifs floraux \
             et de coquillages typiques du Rococo. Tapissé de tapisserie Aubusson d'origine \
             représentant des bouquets de fleurs sur fond crème."
                .to_string(),
        ),
        description_es: Some(
            "Elegante fauteuil en nogal tallado del período Luis XV con patas en cabriola, \
             hacia 1750. La ceintura serpentina y el respaldo tallados con motivos florales y \
             conchas del estilo Rococó. Tapizado en tapicería Aubusson original."
                .to_string(),
        ),
        description_it: Some(
            "Elegante fauteuil in noce intagliato del periodo Luigi XV con gambe a cabriole, \
             circa 1750. Stile Rococò evidente nelle decorazioni floreali e a conchiglia. \
             Tappezzeria Aubusson originale con motivi floreali su fondo crema."
                .to_string(),
        ),
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
        images: Faker.fake(),
        embedding: None,
        origin_year_min: Some(Year::from(1745)),
        origin_year: Some(Year::from(1750)),
        origin_year_max: Some(Year::from(1758)),
        authenticity: product::opensearch::authenticity_document::AuthenticityDocument::Original,
        condition: product::opensearch::condition_document::ConditionDocument::Good,
        provenance: product::opensearch::provenance_document::ProvenanceDocument::Claimed,
        restoration: product::opensearch::restoration_document::RestorationDocument::Minor,
        auction_start: None,
        auction_end: None,
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
    record.product_query = Some(query.try_into().unwrap());
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
    record.product_query = Some(query.try_into().unwrap());
    record.language = language;

    index_document(&repository, record).await;

    let actual = repository
        .percolate(&louis_xv_fauteuil_product_document())
        .await
        .unwrap();

    assert!(actual.is_empty());
}
