use common::currency::domain::Currency;
use common::language::domain::Language;
use common::pagination::cursor::Cursor;
use common::query::text_query::TextQuery;
use common::resource_state::domain::ResourceState;
use common::user_id::UserId;
use common::user_search_filter_id::UserSearchFilterId;
use common::user_search_filter_name::UserSearchFilterName;
use product_core::product_search::ProductSearch;
use search_filter_opensearch::OpenSearchSearchFilterIndex;
use search_filter_service::ports::{
    SearchFilterIndex, SearchFilterIndexQuery, SearchFilterProjection,
    SearchFilterProjectionWriteOutcome, SearchFilterView,
};
use serde_json::json;
use test_api::{
    IntegrationTestService, OpenSearch, aura_integration_test, get_opensearch_client, refresh_index,
};
use time::macros::datetime;

#[aura_integration_test(services = [OpenSearch()])]
async fn should_index_query_percolate_and_delete_search_filter_document() {
    let client = get_opensearch_client().await.clone();
    let index = OpenSearchSearchFilterIndex::new(client);
    let view = sample_view("canonical opensearch renaissance cabinet");

    index
        .upsert(&projection(&view, 1))
        .await
        .unwrap_or_else(|error| panic!("index failed: {error:?}"));
    refresh_index("user_search_filters").await;

    let query_result = index
        .query(&SearchFilterIndexQuery {
            state: Some(ResourceState::Active),
            has_enhanced_search_description: Some(false),
            cursor: Some(Cursor {
                size: 50,
                search_after: None,
            }),
        })
        .await
        .unwrap_or_else(|error| panic!("query failed: {error:?}"));
    let indexed = match query_result
        .items
        .iter()
        .find(|item| item.search_filter_id == view.search_filter_id)
    {
        Some(item) => item,
        None => panic!("indexed search filter was not returned"),
    };
    assert_eq!(view.search, indexed.search);

    let percolate_result = index
        .percolate(json!({
            "titleEn":"canonical opensearch renaissance cabinet",
            "lifecycle":"ACTIVE"
        }))
        .await
        .unwrap_or_else(|error| panic!("percolate failed: {error:?}"));
    assert!(
        percolate_result
            .iter()
            .any(|item| item.search_filter_id == view.search_filter_id)
    );

    index
        .delete(view.search_filter_id, 2)
        .await
        .unwrap_or_else(|error| panic!("delete failed: {error:?}"));
    assert!(matches!(
        index.upsert(&projection(&view, 1)).await,
        Ok(SearchFilterProjectionWriteOutcome::Stale)
    ));
    refresh_index("user_search_filters").await;

    let after_delete = index
        .percolate(json!({
            "titleEn":"canonical opensearch renaissance cabinet",
            "lifecycle":"ACTIVE"
        }))
        .await
        .unwrap_or_else(|error| panic!("percolate after delete failed: {error:?}"));
    assert!(
        !after_delete
            .iter()
            .any(|item| item.search_filter_id == view.search_filter_id)
    );
}

#[aura_integration_test(services = [OpenSearch()])]
async fn should_percolate_any_of_multiple_product_queries() {
    let client = get_opensearch_client().await.clone();
    let index = OpenSearchSearchFilterIndex::new(client);
    let view = SearchFilterView {
        search: ProductSearch::new(Language::En, Currency::Eur)
            .with_product_query(text_query("copper table lamp"))
            .with_product_query(text_query("bronze floor lamp")),
        ..sample_view("copper table lamp")
    };

    index
        .upsert(&projection(&view, 1))
        .await
        .unwrap_or_else(|error| panic!("index failed: {error:?}"));
    refresh_index("user_search_filters").await;

    let matches = index
        .percolate(json!({
            "titleEn": "bronze floor lamp",
            "title": {"text": "bronze floor lamp", "language": "en"},
            "lifecycle": "ACTIVE"
        }))
        .await
        .unwrap_or_else(|error| panic!("percolate failed: {error:?}"));
    assert!(
        matches
            .iter()
            .any(|item| item.search_filter_id == view.search_filter_id)
    );

    let non_matches = index
        .percolate(json!({
            "titleEn": "garden sculpture",
            "title": {"text": "garden sculpture", "language": "en"},
            "lifecycle": "ACTIVE"
        }))
        .await
        .unwrap_or_else(|error| panic!("percolate failed: {error:?}"));
    assert!(
        !non_matches
            .iter()
            .any(|item| item.search_filter_id == view.search_filter_id)
    );
}

#[aura_integration_test(services = [OpenSearch()])]
async fn should_filter_query_by_enhanced_description_presence() {
    let client = get_opensearch_client().await.clone();
    let index = OpenSearchSearchFilterIndex::new(client);
    let view = sample_view_with_enhanced_description("canonical enhanced brass lamp");

    index
        .upsert(&projection(&view, 1))
        .await
        .unwrap_or_else(|error| panic!("index failed: {error:?}"));
    refresh_index("user_search_filters").await;

    let query_result = index
        .query(&SearchFilterIndexQuery {
            state: Some(ResourceState::Active),
            has_enhanced_search_description: Some(true),
            cursor: Some(Cursor {
                size: 50,
                search_after: None,
            }),
        })
        .await
        .unwrap_or_else(|error| panic!("query failed: {error:?}"));

    assert!(
        query_result
            .items
            .iter()
            .any(|item| item.search_filter_id == view.search_filter_id)
    );
}

fn projection(view: &SearchFilterView, source_version: i64) -> SearchFilterProjection {
    SearchFilterProjection {
        view: view.clone(),
        source_version,
    }
}

fn sample_view(query_text: &str) -> SearchFilterView {
    SearchFilterView {
        search_filter_id: UserSearchFilterId::new(),
        user_id: UserId::new(),
        name: UserSearchFilterName::from("canonical search"),
        notifications: true,
        state: ResourceState::Active,
        search: ProductSearch::new(Language::En, Currency::Eur)
            .with_product_query(text_query(query_text)),
        embedding: None,
        created: datetime!(2026-01-01 00:00:00 UTC),
        updated: datetime!(2026-01-01 00:00:00 UTC),
        last_hybrid_search_matched: datetime!(2026-01-01 00:00:00 UTC),
    }
}

fn sample_view_with_enhanced_description(query_text: &str) -> SearchFilterView {
    SearchFilterView {
        search: ProductSearch::new(Language::En, Currency::Eur)
            .with_product_query(text_query(query_text))
            .with_enhanced_search_description("brass lamp".into()),
        ..sample_view(query_text)
    }
}

fn text_query(value: &str) -> TextQuery<1> {
    match value.try_into() {
        Ok(query) => query,
        Err(error) => panic!("invalid text query: {error:?}"),
    }
}
