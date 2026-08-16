use common::currency::domain::Currency;
use common::event_id::EventId;
use common::fx_rate_id::FxRateId;
use common::language::domain::Language;
use common::pagination::cursor::Cursor;
use common::query::text_query::TextQuery;
use common::resource_state::domain::ResourceState;
use common::user_id::UserId;
use common::user_search_filter_id::UserSearchFilterId;
use common::user_search_filter_name::UserSearchFilterName;
use fxrate_core::{FX_RATE_SCALE, FxRateQuote, FxRateSource, NewFxRateSnapshot};
use indexmap::IndexSet;
use product_core::{
    product::{ProductAddress, ProductAuction, ProductPricing},
    product_image::ProductImage,
    product_search::ProductSearch,
    title::Title,
};
use product_service::ports::{
    ProductPriceFilterPlan, ProductSearchFilterMatchShopType, ProductSearchFilterMatchSource,
};
use search_filter_opensearch::OpenSearchSearchFilterIndex;
use search_filter_service::ports::{
    CompiledSearchFilterProjection, SearchFilterIndex, SearchFilterIndexQuery,
    SearchFilterProjection, SearchFilterProjectionWriteOutcome, SearchFilterView,
};
use std::collections::HashMap;
use strum::IntoEnumIterator;
use test_api::{
    IntegrationTestService, OpenSearch, aura_integration_test, get_opensearch_client, refresh_index,
};
use time::{OffsetDateTime, macros::datetime};
use url::Url;

#[aura_integration_test(services = [OpenSearch()])]
async fn should_index_query_percolate_and_delete_search_filter_document() {
    let client = get_opensearch_client().await.clone();
    let index = OpenSearchSearchFilterIndex::new(client);
    let view = sample_view("canonical opensearch renaissance cabinet");
    let price_filter = price_filter();

    index
        .upsert(&compiled_projection(&view, 1, &price_filter))
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

    let matching_product = product_source("canonical opensearch renaissance cabinet");
    let percolate_result = index
        .percolate(&matching_product, None)
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
        index
            .upsert(&compiled_projection(&view, 1, &price_filter))
            .await,
        Ok(SearchFilterProjectionWriteOutcome::Stale)
    ));
    refresh_index("user_search_filters").await;

    let after_delete = index
        .percolate(&matching_product, None)
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
    let price_filter = price_filter();

    index
        .upsert(&compiled_projection(&view, 1, &price_filter))
        .await
        .unwrap_or_else(|error| panic!("index failed: {error:?}"));
    refresh_index("user_search_filters").await;

    let matching_product = product_source("bronze floor lamp");
    let matches = index
        .percolate(&matching_product, None)
        .await
        .unwrap_or_else(|error| panic!("percolate failed: {error:?}"));
    assert!(
        matches
            .iter()
            .any(|item| item.search_filter_id == view.search_filter_id)
    );

    let non_matching_product = product_source("garden sculpture");
    let non_matches = index
        .percolate(&non_matching_product, None)
        .await
        .unwrap_or_else(|error| panic!("percolate failed: {error:?}"));
    assert!(
        !non_matches
            .iter()
            .any(|item| item.search_filter_id == view.search_filter_id)
    );
}

#[aura_integration_test(services = [OpenSearch()])]
async fn should_use_application_default_size_for_first_query_page() {
    let client = get_opensearch_client().await.clone();
    let index = OpenSearchSearchFilterIndex::new(client);
    let views: Vec<_> = (0..25)
        .map(|number| sample_view(&format!("application page size filter {number}")))
        .collect();

    let price_filter = price_filter();
    for view in &views {
        index
            .upsert(&compiled_projection(view, 1, &price_filter))
            .await
            .unwrap_or_else(|error| panic!("index failed: {error:?}"));
    }
    refresh_index("user_search_filters").await;

    let result = index
        .query(&SearchFilterIndexQuery {
            state: Some(ResourceState::Active),
            has_enhanced_search_description: Some(false),
            ..Default::default()
        })
        .await
        .unwrap_or_else(|error| panic!("query failed: {error:?}"));

    assert_eq!(21, result.cursor.size);
    assert_eq!(21, result.items.len());
    assert!(result.cursor.search_after.is_some());
}

#[aura_integration_test(services = [OpenSearch()])]
async fn should_filter_query_by_enhanced_description_presence() {
    let client = get_opensearch_client().await.clone();
    let index = OpenSearchSearchFilterIndex::new(client);
    let view = sample_view_with_enhanced_description("canonical enhanced brass lamp");
    let price_filter = price_filter();

    index
        .upsert(&compiled_projection(&view, 1, &price_filter))
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

fn product_source(title: &str) -> ProductSearchFilterMatchSource {
    let event_id = EventId::new();
    ProductSearchFilterMatchSource {
        event_id,
        event_kind: product_service::ports::ProductSearchFilterMatchSourceEventKind::Domain,
        current_event_id: event_id,
        projection_version: 1,
        product_id: common::product_id::ProductId::new(),
        product_slug_id: common::product_slug_id::ProductSlugId::from("product"),
        shop_id: common::shop_id::ShopId::new(),
        shop_slug_id: common::shop_slug_id::ShopSlugId::from("shop"),
        shop_name: common::shop_name::ShopName::from("Shop"),
        shop_type: ProductSearchFilterMatchShopType::Marketplace,
        seller_id: common::shop_id::ShopId::new(),
        seller_slug_id: common::seller_slug_id::SellerSlugId::from("seller"),
        seller_name: common::shop_name::ShopName::from("Seller"),
        shops_product_id: common::shops_product_id::ShopsProductId::from("sku-1"),
        address: ProductAddress::default(),
        product_title: None,
        product_description: None,
        titles: HashMap::from([(Language::En, Title::from(title))]),
        descriptions: HashMap::new(),
        pricing: ProductPricing::default(),
        sale_valuation: None,
        state: common::product_state::domain::ProductState::Available,
        lifecycle: common::product_lifecycle::domain::ProductLifecycle::Active,
        url: Url::parse("https://shop.example.test/products/sku-1")
            .unwrap_or_else(|error| panic!("test URL must be valid: {error}")),
        view_url: Url::parse("https://aura.example.test/products/product")
            .unwrap_or_else(|error| panic!("test URL must be valid: {error}")),
        image: None,
        images: IndexSet::<ProductImage>::new(),
        embedding: None,
        auction: ProductAuction::default(),
        created: OffsetDateTime::UNIX_EPOCH,
        updated: OffsetDateTime::UNIX_EPOCH,
    }
}

fn price_filter() -> ProductPriceFilterPlan {
    let snapshot = NewFxRateSnapshot::capture_eur(
        FxRateId::new(),
        OffsetDateTime::UNIX_EPOCH,
        FxRateSource::FxRatesApi,
        Currency::Eur,
        Currency::iter().map(|currency| FxRateQuote::new(currency, FX_RATE_SCALE)),
    )
    .unwrap_or_else(|error| panic!("failed to create test FX snapshot: {error}"))
    .into_persisted(
        1_i64
            .try_into()
            .unwrap_or_else(|error| panic!("invalid test FX generation: {error}")),
    );
    ProductPriceFilterPlan::compile(snapshot, Currency::Eur, None)
        .unwrap_or_else(|error| panic!("failed to compile test price filter: {error}"))
}

fn compiled_projection(
    view: &SearchFilterView,
    source_version: i64,
    price_filter_plan: &ProductPriceFilterPlan,
) -> CompiledSearchFilterProjection {
    CompiledSearchFilterProjection {
        projection: SearchFilterProjection {
            view: view.clone(),
            source_version,
        },
        price_filter_plan: price_filter_plan.clone(),
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
