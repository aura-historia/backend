use crate::product_listing_document::{ProductListingDocument, ProductListingDocumentSerdeField};

use application::pagination::{Cursor, CursoredResult};
use domain_primitives::query::any_of_query::AnyOfQuery;
use domain_primitives::query::text_query::TextQuery;
use domain_primitives::sort::{Sort, SortOrder};
use geo::opensearch::distance_to_opensearch_value;
use localization::{Language, Localized};
use money::Currency;
use platform_opensearch::search_response::{SearchHit, SearchResponse};
use shop_core::shop_name::ShopName;

use money::Price;
use opensearch::http::Method;
use opensearch::http::headers::HeaderMap;
use opensearch::http::request::JsonBody;
use opensearch::{OpenSearch, SearchParts};
use product_listing_core::product_lifecycle::ProductLifecycle;
use product_listing_core::product_listing_search::ProductListingSearch;
use product_listing_core::product_state::ProductState;
use product_listing_core::sort_product_listing_field::SortProductListingField;
use product_listing_core::title::Title;
use product_listing_service::ports::{
    CompiledProductListingSearch, ProductListingPriceFilterPlan, ProductListingSearchReadError,
    ProductListingSearchReadRequest, ProductListingSearchReader,
};
use product_listing_service::use_cases::queries::search_product_listings::{
    ProductListingSearchReadResult, ProductListingSummary, ProductListingSummaryPriceValuation,
};
use serde::ser::Error;
use serde_json::json;
use shop_core::shop_type::ShopType;
use std::collections::HashMap;
use std::hash::Hash;
use strum::EnumCount;
use time::format_description::well_known;

const DEFAULT_INDEX: &str = "product-listings";
const HYBRID_SEARCH_PIPELINE_NAME: &str = "hybrid-search-pipeline";
const DEFAULT_HYBRID_PAGE_SIZE: u64 = 20;
const HYBRID_K: u16 = 100;

#[derive(Clone)]
pub struct OpenSearchProductListingSearchReader {
    client: OpenSearch,
    index: String,
}

impl OpenSearchProductListingSearchReader {
    pub fn new(client: OpenSearch) -> Self {
        Self {
            client,
            index: DEFAULT_INDEX.to_owned(),
        }
    }

    pub fn with_index(client: OpenSearch, index: impl Into<String>) -> Self {
        Self {
            client,
            index: index.into(),
        }
    }
}

#[async_trait::async_trait]
impl ProductListingSearchReader for OpenSearchProductListingSearchReader {
    #[tracing::instrument(name = "opensearch_product_search", skip_all)]
    async fn search(
        &self,
        request: &ProductListingSearchReadRequest,
    ) -> Result<ProductListingSearchReadResult, ProductListingSearchReadError> {
        let sort = request.sort.unwrap_or(Sort {
            sort: SortProductListingField::Score,
            order: SortOrder::Desc,
        });
        let body = build_search_request(request, &sort)
            .map_err(|_| ProductListingSearchReadError::ProductListingSearchReadModelInvalid)?;
        let response = self
            .client
            .search(SearchParts::Index(&[self.index.as_str()]))
            .body(body)
            .send()
            .await
            .map_err(|_| ProductListingSearchReadError::ProductListingSearchQueryFailed)?
            .error_for_status_code()
            .map_err(|_| ProductListingSearchReadError::ProductListingSearchQueryFailed)?;
        let payload = response
            .text()
            .await
            .map_err(|_| ProductListingSearchReadError::ProductListingSearchQueryFailed)?;
        let search_response =
            serde_json::from_str::<SearchResponse<ProductListingDocument>>(&payload)
                .map_err(|_| ProductListingSearchReadError::ProductListingSearchReadModelInvalid)?
                .into_non_timed_out("product search")
                .map_err(|_| ProductListingSearchReadError::ProductListingSearchQueryFailed)?;

        map_search_response(&request.compiled_search, search_response)
    }

    #[tracing::instrument(name = "opensearch_product_hybrid_search", skip_all)]
    async fn search_hybrid(
        &self,
        request: &ProductListingSearchReadRequest,
        embedding: &[f32],
    ) -> Result<ProductListingSearchReadResult, ProductListingSearchReadError> {
        let body = build_hybrid_search_request(request, embedding)
            .map_err(|_| ProductListingSearchReadError::ProductListingSearchReadModelInvalid)?;
        let pipeline = [("search_pipeline", HYBRID_SEARCH_PIPELINE_NAME)];
        let response = self
            .client
            .send(
                Method::Post,
                SearchParts::Index(&[self.index.as_str()]).url().as_ref(),
                HeaderMap::new(),
                Some(&pipeline),
                Some(JsonBody::new(body)),
                None,
            )
            .await
            .map_err(|_| ProductListingSearchReadError::ProductListingSearchQueryFailed)?;
        if !response.status_code().is_success() {
            return Err(ProductListingSearchReadError::ProductListingSearchQueryFailed);
        }
        let payload = response
            .text()
            .await
            .map_err(|_| ProductListingSearchReadError::ProductListingSearchQueryFailed)?;
        let search_response =
            serde_json::from_str::<SearchResponse<ProductListingDocument>>(&payload)
                .map_err(|_| ProductListingSearchReadError::ProductListingSearchReadModelInvalid)?
                .into_non_timed_out("product hybrid search")
                .map_err(|_| ProductListingSearchReadError::ProductListingSearchQueryFailed)?;

        map_hybrid_search_response(&request.compiled_search, search_response, &request.cursor)
    }
}

pub(crate) fn map_search_response(
    compiled_search: &CompiledProductListingSearch,
    search_response: SearchResponse<ProductListingDocument>,
) -> Result<ProductListingSearchReadResult, ProductListingSearchReadError> {
    let search = &compiled_search.search;
    let cursor = Cursor {
        size: search_response.hits.hits.len() as u64,
        search_after: search_response
            .hits
            .hits
            .last()
            .and_then(|last| last.sort.clone()),
    };
    let items = search_response
        .hits
        .hits
        .into_iter()
        .map(|hit| map_summary(search, &compiled_search.price_filter_plan, hit))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(CursoredResult {
        cursor,
        items,
        total: Some(search_response.hits.total.value),
    })
}

fn map_summary(
    search: &ProductListingSearch,
    price_filter: &ProductListingPriceFilterPlan,
    hit: SearchHit<ProductListingDocument>,
) -> Result<ProductListingSummary, ProductListingSearchReadError> {
    let document = hit.source;
    let display_price = resolve_price(&document, price_filter)?;
    let price_valuation = price_valuation(&document, price_filter)?;
    map_summary_fields(document, search.language, display_price, price_valuation)
}

fn map_summary_fields(
    document: ProductListingDocument,
    preferred_language: Language,
    display_price: Option<Price>,
    price_valuation: ProductListingSummaryPriceValuation,
) -> Result<ProductListingSummary, ProductListingSearchReadError> {
    let title = resolve_title(&document, preferred_language);

    Ok(ProductListingSummary {
        product_listing_id: document.product_listing_id,
        product_listing_slug_id: document.product_listing_slug_id,
        event_id: document.event_id,
        shop_id: document.shop_id,
        seller_id: document.seller_id,
        shop_listing_id: document.shop_listing_id,
        shop_name: ShopName::from(document.shop_name),
        shop_slug_id: document.shop_slug_id,
        title,
        display_price,
        price_valuation,
        state: document.state,
        lifecycle: document.lifecycle,
        url: document.url,
        view_url: document.view_url,
        images: document.images.into_iter().map(Into::into).collect(),
        updated: document.updated,
    })
}

fn resolve_title(
    document: &ProductListingDocument,
    preferred_language: Language,
) -> Option<Localized<Language, Title>> {
    let mut titles = HashMap::new();
    insert_title(&mut titles, Language::De, &document.title_de);
    insert_title(&mut titles, Language::En, &document.title_en);
    insert_title(&mut titles, Language::Fr, &document.title_fr);
    insert_title(&mut titles, Language::Es, &document.title_es);
    insert_title(&mut titles, Language::It, &document.title_it);
    titles
        .entry(document.title.language)
        .or_insert_with(|| Title::from(document.title.text.clone()));
    Language::resolve(&[preferred_language], titles)
}

fn insert_title(titles: &mut HashMap<Language, Title>, language: Language, title: &Option<String>) {
    if let Some(title) = title {
        titles.entry(language).or_insert_with(|| Title::from(title));
    }
}

fn price_valuation(
    document: &ProductListingDocument,
    price_filter: &ProductListingPriceFilterPlan,
) -> Result<ProductListingSummaryPriceValuation, ProductListingSearchReadError> {
    if let (Some(fx_rate_id), Some(sold_at)) = (document.sale_fx_rate_id, document.sold_at) {
        return Ok(ProductListingSummaryPriceValuation::Sale {
            fx_rate_id,
            sold_at,
        });
    }
    if document.has_sale_valuation() {
        return Err(ProductListingSearchReadError::ProductListingSearchReadModelInvalid);
    }
    Ok(ProductListingSummaryPriceValuation::Current {
        fx_rate_id: price_filter.fx_rate_id,
        captured_at: price_filter.captured_at(),
    })
}

fn resolve_price(
    document: &ProductListingDocument,
    price_filter: &ProductListingPriceFilterPlan,
) -> Result<Option<Price>, ProductListingSearchReadError> {
    document
        .validate()
        .map_err(|_| ProductListingSearchReadError::ProductListingSearchReadModelInvalid)?;

    if document.has_sale_valuation() {
        return Ok(document
            .sale_price(price_filter.target_currency)
            .map(|amount| Price::new(amount.into(), price_filter.target_currency)));
    }

    document
        .source_price()
        .map(|(amount, currency)| {
            price_filter
                .convert_active_source_amount(currency, amount)
                .map(|amount| Price::new(amount.into(), price_filter.target_currency))
                .map_err(|_| ProductListingSearchReadError::ProductListingSearchReadModelInvalid)
        })
        .transpose()
}

fn map_hybrid_search_response(
    compiled_search: &CompiledProductListingSearch,
    search_response: SearchResponse<ProductListingDocument>,
    cursor: &Option<Cursor<serde_json::Value>>,
) -> Result<ProductListingSearchReadResult, ProductListingSearchReadError> {
    let search = &compiled_search.search;
    let requested_size = cursor
        .as_ref()
        .map(|cursor| cursor.size)
        .unwrap_or(DEFAULT_HYBRID_PAGE_SIZE);
    let item_count = search_response.hits.hits.len() as u64;
    let search_after = (item_count >= requested_size)
        .then(|| {
            search_response
                .hits
                .hits
                .last()
                .and_then(|hit| hit.sort.clone())
        })
        .flatten();
    let items = search_response
        .hits
        .hits
        .into_iter()
        .map(|hit| map_summary(search, &compiled_search.price_filter_plan, hit))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(ProductListingSearchReadResult {
        cursor: Cursor {
            size: item_count,
            search_after,
        },
        items,
        total: None,
    })
}

pub(crate) fn build_search_request(
    request: &ProductListingSearchReadRequest,
    sort: &Sort<SortProductListingField>,
) -> Result<serde_json::Value, serde_json::Error> {
    let mut body = json!({
        "_source": { "excludes": [ProductListingDocumentSerdeField::Embedding] },
        "query": build_search_query(&request.compiled_search)?
    });
    let cursor = &request.cursor;

    if let Some(cursor) = cursor {
        body["size"] = json!(cursor.size);
        if let Some(search_after) = &cursor.search_after {
            body["search_after"] = opensearch_search_after(search_after);
        }
    }

    let sort_field = match sort.sort {
        SortProductListingField::Score => "_score",
        SortProductListingField::Created => ProductListingDocumentSerdeField::Created.as_str(),
        SortProductListingField::Updated => ProductListingDocumentSerdeField::Updated.as_str(),
    };
    let order = match sort.order {
        SortOrder::Asc => "asc",
        SortOrder::Desc => "desc",
    };
    let primary_sort = if matches!(sort.sort, SortProductListingField::Score) {
        json!({ sort_field: { "order": order } })
    } else {
        json!({ sort_field: { "order": order, "missing": "_last" } })
    };
    body["sort"] = json!([
        primary_sort,
        { ProductListingDocumentSerdeField::ProductListingId.as_str(): { "order": "asc" } }
    ]);

    Ok(body)
}

pub(crate) fn build_search_query(
    compiled_search: &CompiledProductListingSearch,
) -> Result<serde_json::Value, serde_json::Error> {
    let search = &compiled_search.search;
    let mut must = Vec::with_capacity(1);
    if let Some(product_listing_query_clause) = build_product_listing_query_clause(
        &search.product_listing_query,
        title_field(&search.language),
    ) {
        must.push(product_listing_query_clause);
    }
    let (must_not, filter) =
        build_product_index_filter_clauses(search, &compiled_search.price_filter_plan)?;

    Ok(json!({
        "bool": {
            "must": must,
            "must_not": must_not,
            "filter": filter
        }
    }))
}

pub(crate) fn build_hybrid_search_request(
    request: &ProductListingSearchReadRequest,
    embedding: &[f32],
) -> Result<serde_json::Value, serde_json::Error> {
    let search = &request.compiled_search.search;
    let cursor = &request.cursor;
    let (must_not, filter) =
        build_product_index_filter_clauses(search, &request.compiled_search.price_filter_plan)?;
    let title_field = title_field(&search.language);
    let bm25_text = build_product_listing_query_clause(&search.product_listing_query, title_field)
        .unwrap_or_else(|| json!({ "match_all": {} }));
    let bm25 = json!({
        "bool": {
            "must": [bm25_text],
            "filter": filter.clone(),
            "must_not": must_not.clone(),
        }
    });
    let mut knn = json!({
        "vector": embedding,
        "k": HYBRID_K,
    });
    if !filter.is_empty() || !must_not.is_empty() {
        knn["filter"] = json!({
            "bool": {
                "filter": filter,
                "must_not": must_not,
            }
        });
    }
    let mut body = json!({
        "_source": { "excludes": [ProductListingDocumentSerdeField::Embedding] },
        "size": cursor
            .as_ref()
            .map(|cursor| cursor.size)
            .unwrap_or(DEFAULT_HYBRID_PAGE_SIZE)
            .max(1),
        "query": {
            "hybrid": {
                "queries": [
                    bm25,
                    { "knn": { ProductListingDocumentSerdeField::Embedding.as_str(): knn } }
                ]
            }
        },
        "sort": [{
            "_script": {
                "type": "number",
                "script": {
                    "source": format!(
                        "return _score + (Math.abs(doc['{}'].value.hashCode()) * 1.0e-15);",
                        ProductListingDocumentSerdeField::ProductListingId.as_str()
                    )
                },
                "order": "desc"
            }
        }]
    });
    if let Some(search_after) = cursor
        .as_ref()
        .and_then(|cursor| cursor.search_after.as_ref())
    {
        body["search_after"] = opensearch_search_after(search_after);
    }

    Ok(body)
}

fn opensearch_search_after(search_after: &serde_json::Value) -> serde_json::Value {
    match search_after {
        serde_json::Value::Array(_) => search_after.clone(),
        _ => serde_json::Value::Array(vec![search_after.clone()]),
    }
}

fn title_field(language: &Language) -> ProductListingDocumentSerdeField {
    match language {
        Language::De => ProductListingDocumentSerdeField::TitleDe,
        Language::En => ProductListingDocumentSerdeField::TitleEn,
        Language::Fr => ProductListingDocumentSerdeField::TitleFr,
        Language::Es => ProductListingDocumentSerdeField::TitleEs,
        Language::It => ProductListingDocumentSerdeField::TitleIt,
        _ => ProductListingDocumentSerdeField::TitleEn,
    }
}

fn build_product_listing_query_clause(
    product_queries: &[TextQuery<1>],
    title_field: ProductListingDocumentSerdeField,
) -> Option<serde_json::Value> {
    match product_queries {
        [] => None,
        [product_listing_query] => Some(build_text_match_clause(
            product_listing_query.as_ref(),
            title_field,
        )),
        product_queries => Some(json!({
            "bool": {
                "should": product_queries
                    .iter()
                    .map(|product_listing_query| build_text_match_clause(product_listing_query.as_ref(), title_field))
                    .collect::<Vec<_>>(),
                "minimum_should_match": 1
            }
        })),
    }
}

fn build_text_match_clause(
    product_listing_query: &str,
    title_field: ProductListingDocumentSerdeField,
) -> serde_json::Value {
    json!({
        "bool": {
            "must": [{
                "bool": {
                    "should": [
                        {
                            "multi_match": {
                                "query": product_listing_query,
                                "fields": [format!("{title_field}^5")],
                                "type": "best_fields",
                                "operator": "and"
                            }
                        },
                        {
                            "bool": {
                                "must": [{
                                    "multi_match": {
                                        "query": product_listing_query,
                                        "fields": ["title.text^3"],
                                        "type": "best_fields",
                                        "operator": "and"
                                    }
                                }],
                                "boost": 0.7
                            }
                        }
                    ],
                    "minimum_should_match": 1
                }
            }],
            "should": [
                {
                    "match_phrase": {
                        title_field.as_str(): {
                            "query": product_listing_query,
                            "boost": 6
                        }
                    }
                },
                {
                    "match_phrase": {
                        "title.text": {
                            "query": product_listing_query,
                            "boost": 3
                        }
                    }
                },
                {
                    "match": {
                        title_field.as_str(): {
                            "query": product_listing_query,
                            "fuzziness": "AUTO:4,6",
                            "minimum_should_match": "2<75%",
                            "boost": 3
                        }
                    }
                }
            ]
        }
    })
}

pub(crate) fn build_product_index_filter_clauses(
    search: &ProductListingSearch,
    price_filter: &ProductListingPriceFilterPlan,
) -> Result<(Vec<serde_json::Value>, Vec<serde_json::Value>), serde_json::Error> {
    let (must_not, mut filter) = build_common_filter_clauses(search)?;
    if let Some(price_clause) = build_product_index_price_clause(price_filter) {
        filter.push(price_clause);
    }
    Ok((must_not, filter))
}

pub(crate) fn build_common_filter_clauses(
    search: &ProductListingSearch,
) -> Result<(Vec<serde_json::Value>, Vec<serde_json::Value>), serde_json::Error> {
    let mut must_not = Vec::new();
    let mut filter = Vec::new();

    let lifecycle_terms = if search.lifecycle_query.is_empty() {
        vec![ProductLifecycle::Active.as_str().to_owned()]
    } else {
        search
            .lifecycle_query
            .iter()
            .map(|value| value.as_str().to_owned())
            .collect()
    };
    filter.push(json!({
        "terms": {
            ProductListingDocumentSerdeField::Lifecycle.as_str(): lifecycle_terms
        }
    }));

    if !search.exclude_product_listing_id_query.is_empty() {
        must_not.push(json!({
            "terms": {
                ProductListingDocumentSerdeField::ProductListingId.as_str(): search.exclude_product_listing_id_query.iter().map(ToString::to_string).collect::<Vec<_>>()
            }
        }));
    }
    if !search.exclude_shop_name_query.is_empty() {
        must_not.push(json!({
            "terms": {
                ProductListingDocumentSerdeField::ShopName.as_str(): search.exclude_shop_name_query.iter().map(ShopName::as_ref).collect::<Vec<_>>()
            }
        }));
    }
    if !search.exclude_seller_name_query.is_empty() {
        must_not.push(json!({
            "terms": {
                ProductListingDocumentSerdeField::SellerName.as_str(): search.exclude_seller_name_query.iter().map(ShopName::as_ref).collect::<Vec<_>>()
            }
        }));
    }
    if !search.exclude_shop_slug_id_query.is_empty() {
        must_not.push(json!({
            "terms": {
                ProductListingDocumentSerdeField::ShopSlugId.as_str(): search.exclude_shop_slug_id_query.iter().map(ToString::to_string).collect::<Vec<_>>()
            }
        }));
    }
    if !search.exclude_seller_slug_id_query.is_empty() {
        must_not.push(json!({
            "terms": {
                ProductListingDocumentSerdeField::SellerSlugId.as_str(): search.exclude_seller_slug_id_query.iter().map(ToString::to_string).collect::<Vec<_>>()
            }
        }));
    }

    if !search.country_query.is_empty() {
        filter.push(json!({
            "terms": {
                ProductListingDocumentSerdeField::StructuredAddressCountry.as_str(): search.country_query.iter().map(|country| country.alpha2()).collect::<Vec<_>>()
            }
        }));
    }
    if !search.continent_query.is_empty() {
        filter.push(json!({
            "terms": {
                ProductListingDocumentSerdeField::StructuredAddressContinent.as_str(): search.continent_query.iter().map(|continent| (*continent).as_str()).collect::<Vec<_>>()
            }
        }));
    }
    if let Some(query) = search.geo_address_distance_query {
        filter.push(json!({
            "geo_distance": {
                "distance": distance_to_opensearch_value(query.distance),
                ProductListingDocumentSerdeField::GeoAddress.as_str(): {
                    "lat": query.lat,
                    "lon": query.lon
                }
            }
        }));
    }

    apply_any_of_filter(
        &mut filter,
        &search.shop_name_query,
        ProductListingDocumentSerdeField::ShopName,
        ShopName::as_ref,
    );
    apply_any_of_filter(
        &mut filter,
        &search.seller_name_query,
        ProductListingDocumentSerdeField::SellerName,
        ShopName::as_ref,
    );
    if !search.shop_slug_id_query.is_empty() {
        filter.push(json!({
            "terms": {
                ProductListingDocumentSerdeField::ShopSlugId.as_str(): search.shop_slug_id_query.iter().map(ToString::to_string).collect::<Vec<_>>()
            }
        }));
    }
    if !search.seller_slug_id_query.is_empty() {
        filter.push(json!({
            "terms": {
                ProductListingDocumentSerdeField::SellerSlugId.as_str(): search.seller_slug_id_query.iter().map(ToString::to_string).collect::<Vec<_>>()
            }
        }));
    }

    apply_any_of_filter(
        &mut filter,
        &search.state_query,
        ProductListingDocumentSerdeField::State,
        |value: &ProductState| value.as_str(),
    );
    apply_any_of_filter(
        &mut filter,
        &search.shop_type_query,
        ProductListingDocumentSerdeField::ShopType,
        |value: &ShopType| value.as_str(),
    );

    for (query, field) in [
        (
            &search.created_query,
            ProductListingDocumentSerdeField::Created,
        ),
        (
            &search.updated_query,
            ProductListingDocumentSerdeField::Updated,
        ),
        (
            &search.auction_start_query,
            ProductListingDocumentSerdeField::AuctionStart,
        ),
        (
            &search.auction_end_query,
            ProductListingDocumentSerdeField::AuctionEnd,
        ),
    ] {
        if let Some(min) = query.and_then(|query| query.min) {
            let value = min
                .format(&well_known::Rfc3339)
                .map_err(serde_json::Error::custom)?;
            filter.push(json!({ "range": { field.as_str(): { "gte": value } } }));
        }
        if let Some(max) = query.and_then(|query| query.max) {
            let value = max
                .format(&well_known::Rfc3339)
                .map_err(serde_json::Error::custom)?;
            filter.push(json!({ "range": { field.as_str(): { "lte": value } } }));
        }
    }

    Ok((must_not, filter))
}

/// Renders the pinned price clause for the persistent ProductListing index only.
pub(crate) fn build_product_index_price_clause(
    price_filter: &ProductListingPriceFilterPlan,
) -> Option<serde_json::Value> {
    if !price_filter.has_price_filter() {
        return None;
    }

    let active_ranges = price_filter
        .active_native_ranges
        .iter()
        .map(|range| {
            json!({
                "bool": {
                    "filter": [
                        { "term": { "sourcePrice.currency": currency_code(range.source_currency) } },
                        { "range": { "sourcePrice.amount": range_query(range.lower, range.upper) } }
                    ]
                }
            })
        })
        .collect::<Vec<_>>();
    let active = (!active_ranges.is_empty()).then(|| {
        json!({
            "bool": {
                "must_not": [{ "exists": { "field": "saleFxRateId" } }],
                "filter": [{
                    "bool": {
                        "should": active_ranges,
                        "minimum_should_match": 1
                    }
                }]
            }
        })
    });
    let sold = json!({
        "bool": {
            "filter": [
                { "exists": { "field": "saleFxRateId" } },
                { "range": {
                    sale_price_field_for(price_filter.target_currency): display_range_query(price_filter)
                } }
            ]
        }
    });
    let should = active.into_iter().chain([sold]).collect::<Vec<_>>();

    Some(json!({
        "bool": {
            "should": should,
            "minimum_should_match": 1
        }
    }))
}

fn range_query(lower: u64, upper: Option<u64>) -> serde_json::Value {
    match upper {
        Some(upper) => json!({ "gte": lower, "lte": upper }),
        None => json!({ "gte": lower }),
    }
}

fn display_range_query(price_filter: &ProductListingPriceFilterPlan) -> serde_json::Value {
    let mut range = serde_json::Map::new();
    if let Some(lower) = price_filter.sold_display_range.min {
        range.insert("gte".to_owned(), json!(u64::from(lower)));
    }
    if let Some(upper) = price_filter.sold_display_range.max {
        range.insert("lte".to_owned(), json!(u64::from(upper)));
    }
    serde_json::Value::Object(range)
}

fn currency_code(currency: Currency) -> &'static str {
    match currency {
        Currency::Eur => "EUR",
        Currency::Gbp => "GBP",
        Currency::Usd => "USD",
        Currency::Aud => "AUD",
        Currency::Cad => "CAD",
        Currency::Nzd => "NZD",
        Currency::Cny => "CNY",
        Currency::Brl => "BRL",
        Currency::Pln => "PLN",
        Currency::Try => "TRY",
        Currency::Jpy => "JPY",
        Currency::Czk => "CZK",
        Currency::Rub => "RUB",
        Currency::Aed => "AED",
        Currency::Sar => "SAR",
        Currency::Hkd => "HKD",
        Currency::Sgd => "SGD",
        Currency::Chf => "CHF",
    }
}

fn sale_price_field_for(currency: Currency) -> &'static str {
    match currency {
        Currency::Eur => "salePrices.eur",
        Currency::Gbp => "salePrices.gbp",
        Currency::Usd => "salePrices.usd",
        Currency::Aud => "salePrices.aud",
        Currency::Cad => "salePrices.cad",
        Currency::Nzd => "salePrices.nzd",
        Currency::Cny => "salePrices.cny",
        Currency::Brl => "salePrices.brl",
        Currency::Pln => "salePrices.pln",
        Currency::Try => "salePrices.try",
        Currency::Jpy => "salePrices.jpy",
        Currency::Czk => "salePrices.czk",
        Currency::Rub => "salePrices.rub",
        Currency::Aed => "salePrices.aed",
        Currency::Sar => "salePrices.sar",
        Currency::Hkd => "salePrices.hkd",
        Currency::Sgd => "salePrices.sgd",
        Currency::Chf => "salePrices.chf",
    }
}

fn apply_any_of_filter<T: Hash + Eq + EnumCount>(
    filter: &mut Vec<serde_json::Value>,
    query: &AnyOfQuery<T>,
    field: ProductListingDocumentSerdeField,
    to_str: fn(&T) -> &str,
) {
    let values = query.iter().map(to_str).collect::<Vec<_>>();
    if !values.is_empty() && values.len() != T::COUNT {
        filter.push(json!({ "terms": { field.as_str(): values } }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::product_listing_document::{SalePricesDocument, SourcePriceDocument, TextDocument};
    use domain_primitives::event_id::EventId;
    use fxrate_core::{FX_RATE_SCALE, FxRateId, FxRateQuote, FxRateSource, NewFxRateSnapshot};
    use geo::core::distance::{Distance, DistanceUnit, GeoDistanceQuery};
    use indexmap::IndexSet;
    use money::MonetaryAmount;
    use product_listing_core::{
        product_lifecycle::ProductLifecycle, product_listing_id::ProductListingId,
        product_listing_slug_id::ProductListingSlugId, product_state::ProductState,
        shop_listing_id::ShopListingId,
    };
    use shop_core::shop_id::ShopId;
    use shop_core::shop_slug_id::ShopSlugId;
    use shop_core::shop_type::ShopType;
    use strum::IntoEnumIterator;
    use time::{OffsetDateTime, macros::datetime};
    use url::Url;

    fn price_filter(
        target_currency: Currency,
        display_amount: Option<u64>,
    ) -> Result<ProductListingPriceFilterPlan, Box<dyn std::error::Error>> {
        price_filter_range(
            target_currency,
            display_amount.map(|amount| domain_primitives::query::range_query::RangeQuery {
                min: Some(MonetaryAmount::from(amount)),
                max: Some(MonetaryAmount::from(amount)),
            }),
        )
    }

    fn price_filter_range(
        target_currency: Currency,
        display_range: Option<domain_primitives::query::range_query::RangeQuery<MonetaryAmount>>,
    ) -> Result<ProductListingPriceFilterPlan, Box<dyn std::error::Error>> {
        let snapshot = NewFxRateSnapshot::capture_eur(
            FxRateId::new(),
            OffsetDateTime::UNIX_EPOCH,
            FxRateSource::FxRatesApi,
            Currency::Eur,
            Currency::iter().map(|currency| {
                FxRateQuote::new(
                    currency,
                    if currency == Currency::Usd {
                        1_100_000
                    } else {
                        FX_RATE_SCALE
                    },
                )
            }),
        )?
        .into_persisted(1_i64.try_into()?);
        Ok(ProductListingPriceFilterPlan::compile(
            snapshot,
            target_currency,
            display_range,
        )?)
    }

    fn document() -> Result<ProductListingDocument, url::ParseError> {
        Ok(ProductListingDocument {
            product_listing_id: ProductListingId::new(),
            product_listing_slug_id: ProductListingSlugId::from("vase-abcdef"),
            shop_slug_id: ShopSlugId::from("shop"),
            seller_slug_id: shop_core::seller_slug_id::SellerSlugId::from("seller"),
            event_id: EventId::new(),
            shop_id: ShopId::new(),
            seller_id: ShopId::new(),
            shop_listing_id: ShopListingId::from("sku-1"),
            shop_name: "Shop".to_owned(),
            seller_name: "Seller".to_owned(),
            shop_type: ShopType::CommercialDealer,
            structured_address_addressline: None,
            structured_address_addressline_extra: None,
            structured_address_locality: None,
            structured_address_region: None,
            structured_address_postal_code: None,
            structured_address_country: None,
            structured_address_continent: None,
            geo_address: None,
            title: TextDocument::new("Vase", Language::En),
            title_de: None,
            title_en: Some("Vase".to_owned()),
            title_fr: None,
            title_es: None,
            title_it: None,
            source_price: Some(SourcePriceDocument {
                amount: 100,
                currency: Currency::Eur,
            }),
            sale_prices: None,
            sale_fx_rate_id: None,
            sold_at: None,
            state: ProductState::Available,
            lifecycle: ProductLifecycle::Active,
            url: Url::parse("https://shop.example/product_listings/sku-1")?,
            view_url: Url::parse("https://aura.example/product_listings/vase-abcdef")?,
            images: IndexSet::new(),
            embedding: None,
            auction_start: None,
            auction_end: None,
            created: datetime!(2025-01-01 0:00 UTC),
            updated: datetime!(2025-01-02 0:00 UTC),
        })
    }

    #[test]
    fn should_render_geo_distance_with_opensearch_distance_format()
    -> Result<(), Box<dyn std::error::Error>> {
        let search = ProductListingSearch::new(Language::En, Currency::Eur)
            .with_geo_address_distance_query(GeoDistanceQuery {
                lat: 52.52,
                lon: 13.405,
                distance: Distance {
                    amount: 50.0,
                    unit: DistanceUnit::Kilometers,
                },
            });

        let (_, filters) = build_common_filter_clauses(&search)?;
        let distance_filter = filters
            .iter()
            .find(|filter| filter.get("geo_distance").is_some())
            .ok_or("missing geo distance filter")?;

        assert_eq!(
            Some(&json!("50km")),
            distance_filter.pointer("/geo_distance/distance")
        );
        assert_eq!(
            Some(&json!(52.52)),
            distance_filter.pointer("/geo_distance/geoAddress/lat")
        );
        Ok(())
    }

    #[test]
    fn should_render_active_and_sold_price_branches_from_plan()
    -> Result<(), Box<dyn std::error::Error>> {
        let clause = build_product_index_price_clause(&price_filter(Currency::Usd, Some(110))?)
            .ok_or("missing price clause")?;

        assert_eq!(
            clause.pointer("/bool/should/0/bool/must_not/0/exists/field"),
            Some(&json!("saleFxRateId"))
        );
        assert_eq!(
            clause.pointer(
                "/bool/should/0/bool/filter/0/bool/should/0/bool/filter/0/term/sourcePrice.currency"
            ),
            Some(&json!("EUR"))
        );
        assert_eq!(
            clause.pointer("/bool/should/0/bool/filter/0/bool/should/0/bool/filter/1/range/sourcePrice.amount/gte"),
            Some(&json!(100))
        );
        assert_eq!(
            clause.pointer("/bool/should/1/bool/filter/0/exists/field"),
            Some(&json!("saleFxRateId"))
        );
        assert_eq!(
            clause.pointer("/bool/should/1/bool/filter/1/range/salePrices.usd/lte"),
            Some(&json!(110))
        );
        Ok(())
    }

    #[test]
    fn should_not_render_price_clause_without_price_filter()
    -> Result<(), Box<dyn std::error::Error>> {
        assert!(build_product_index_price_clause(&price_filter(Currency::Usd, None)?).is_none());
        Ok(())
    }

    #[test]
    fn should_render_open_price_bounds_without_inventing_limits()
    -> Result<(), Box<dyn std::error::Error>> {
        let max_only = build_product_index_price_clause(&price_filter_range(
            Currency::Usd,
            Some(domain_primitives::query::range_query::RangeQuery {
                min: None,
                max: Some(MonetaryAmount::from(110_u64)),
            }),
        )?)
        .ok_or("missing max-only clause")?;
        let min_only = build_product_index_price_clause(&price_filter_range(
            Currency::Usd,
            Some(domain_primitives::query::range_query::RangeQuery {
                min: Some(MonetaryAmount::from(110_u64)),
                max: None,
            }),
        )?)
        .ok_or("missing min-only clause")?;

        assert!(
            max_only
                .pointer("/bool/should/1/bool/filter/1/range/salePrices.usd/gte")
                .is_none()
        );
        assert_eq!(
            Some(&json!(110)),
            max_only.pointer("/bool/should/1/bool/filter/1/range/salePrices.usd/lte")
        );
        assert_eq!(
            Some(&json!(110)),
            min_only.pointer("/bool/should/1/bool/filter/1/range/salePrices.usd/gte")
        );
        assert!(
            min_only
                .pointer("/bool/should/1/bool/filter/1/range/salePrices.usd/lte")
                .is_none()
        );
        Ok(())
    }

    #[test]
    fn should_use_distinct_price_clauses_for_search_and_percolation()
    -> Result<(), Box<dyn std::error::Error>> {
        let search = ProductListingSearch::new(Language::En, Currency::Usd).with_price_query(
            domain_primitives::query::range_query::RangeQuery {
                min: Some(MonetaryAmount::from(110_u64)),
                max: None,
            },
        );
        let compiled_search = CompiledProductListingSearch {
            search: search.clone(),
            price_filter_plan: price_filter(Currency::Usd, Some(110))?,
        };

        let product_index = build_search_query(&compiled_search)?;
        let percolator = crate::product_listing_percolator_query::build_percolator_query(&search)?;

        assert!(product_index.to_string().contains("sourcePrice"));
        assert!(percolator.to_string().contains("priceByCurrency.usd"));
        assert!(!percolator.to_string().contains("sourcePrice"));
        Ok(())
    }

    #[test]
    fn should_convert_active_price_with_pinned_plan() -> Result<(), Box<dyn std::error::Error>> {
        let price = resolve_price(&document()?, &price_filter(Currency::Usd, Some(110))?)?;

        assert_eq!(
            price,
            Some(Price::new(MonetaryAmount::from(110_u64), Currency::Usd))
        );
        Ok(())
    }

    #[test]
    fn should_use_exact_target_sale_price() -> Result<(), Box<dyn std::error::Error>> {
        let mut document = document()?;
        document.sale_prices = Some(SalePricesDocument {
            eur: 100,
            gbp: 80,
            usd: 777,
            aud: 100,
            cad: 100,
            nzd: 100,
            cny: 100,
            brl: 100,
            pln: 100,
            r#try: 100,
            jpy: 100,
            czk: 100,
            rub: 100,
            aed: 100,
            sar: 100,
            hkd: 100,
            sgd: 100,
            chf: 100,
        });
        document.sale_fx_rate_id = Some(FxRateId::new());
        document.sold_at = Some(OffsetDateTime::UNIX_EPOCH);

        let price = resolve_price(&document, &price_filter(Currency::Usd, Some(110))?)?;

        assert_eq!(
            price,
            Some(Price::new(MonetaryAmount::from(777_u64), Currency::Usd))
        );
        Ok(())
    }

    #[test]
    fn should_map_sold_document_without_sale_prices_to_sale_valuation_and_no_display_price()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut document = document()?;
        let fx_rate_id = FxRateId::new();
        document.source_price = None;
        document.sale_fx_rate_id = Some(fx_rate_id);
        document.sold_at = Some(OffsetDateTime::UNIX_EPOCH);

        let summary = map_summary_fields(
            document.clone(),
            Language::En,
            resolve_price(&document, &price_filter(Currency::Usd, None)?)?,
            price_valuation(&document, &price_filter(Currency::Usd, None)?)?,
        )?;

        assert_eq!(None, summary.display_price);
        assert_eq!(
            ProductListingSummaryPriceValuation::Sale {
                fx_rate_id,
                sold_at: OffsetDateTime::UNIX_EPOCH,
            },
            summary.price_valuation
        );
        Ok(())
    }

    #[test]
    fn should_reject_invalid_sale_projection_when_mapping_price()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut document = document()?;
        document.sale_fx_rate_id = Some(FxRateId::new());

        assert!(matches!(
            resolve_price(&document, &price_filter(Currency::Usd, None)?),
            Err(ProductListingSearchReadError::ProductListingSearchReadModelInvalid)
        ));
        Ok(())
    }
}
