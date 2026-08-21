use crate::core::product_search::ProductSearch;
use crate::core::sort_product_field::SortProductField;
use crate::opensearch::product_document::{ProductDocument, ProductDocumentSerdeField};
use crate::opensearch::product_state_document::ProductStateDocument;
use crate::opensearch::product_update_document::ProductUpdateDocument;
use async_trait::async_trait;
use common::currency::domain::Currency;
use common::language::domain::Language;
use common::opensearch::{bulk_response::BulkResponse, search_response::SearchResponse};
use common::pagination::cursor::Cursor;
use common::product_id::ProductId;
use common::product_lifecycle::document::ProductLifecycleDocument;
use common::query::any_of_query::AnyOfQuery;
use common::query::text_query::TextQuery;

use common::shop_name::ShopName;
use common::sort::{Sort, SortOrder};
use geo::opensearch::distance_to_opensearch_value;
use opensearch::http::Method;
use opensearch::http::headers::HeaderMap;
use opensearch::http::request::JsonBody;
use opensearch::{BulkOperation, BulkOperations, BulkParts, GetParts, SearchParts};
use serde::ser::Error;
use serde_json::json;
use shop::opensearch::{
    continent_document::ContinentDocument, shop_type_document::ShopTypeDocument,
};
use std::collections::HashMap;
use std::hash::Hash;
use std::ops::Deref;
use strum::EnumCount;
use time::format_description::well_known;

/// Name of the pre-registered OpenSearch search pipeline used for hybrid (BM25 + kNN) queries.
///
/// The pipeline must be registered on the cluster before hybrid searches are issued.
/// In tests this is done by [`test_api::opensearch::set_up_indices`].
pub const HYBRID_SEARCH_PIPELINE_NAME: &str = "hybrid-search-pipeline";

const DEFAULT_HYBRID_PAGE_SIZE: u64 = 20;
const HYBRID_K: u16 = 100;

#[async_trait]
#[mockall::automock]
pub trait ProductOpenSearchRepository {
    async fn create_product_documents(
        &self,
        documents: Vec<ProductDocument>,
    ) -> Result<BulkResponse, opensearch::Error>;

    async fn update_product_documents(
        &self,
        updates: HashMap<ProductId, ProductUpdateDocument>,
    ) -> Result<BulkResponse, opensearch::Error>;

    async fn delete_product_documents(
        &self,
        product_ids: Vec<ProductId>,
    ) -> Result<BulkResponse, opensearch::Error>;

    async fn search_product_documents(
        &self,
        search: &ProductSearch,
        sort: &Sort<SortProductField>,
        page: &Option<Cursor<serde_json::Value>>,
    ) -> Result<SearchResponse<ProductDocument>, opensearch::Error>;

    async fn search_product_documents_with_percolator_query(
        &self,
        search: &ProductSearch,
        size: u64,
    ) -> Result<SearchResponse<ProductDocument>, opensearch::Error>;

    async fn get_product_document_by_id(
        &self,
        product_id: &ProductId,
    ) -> Result<ProductDocument, opensearch::Error>;

    async fn k_nn_text(
        &self,
        embedding: &[f32],
        k: u16,
    ) -> Result<SearchResponse<ProductDocument>, opensearch::Error>;

    /// Native OpenSearch hybrid retrieval. The request body uses the `hybrid` query type
    /// (parallel BM25 + kNN) combined via the pre-registered search pipeline named
    /// [`HYBRID_SEARCH_PIPELINE_NAME`], which is passed as a URL query parameter.
    /// Pagination uses standard `search_after`.
    async fn hybrid_search_product_documents(
        &self,
        search: &ProductSearch,
        embedding: &[f32],
        cursor: &Option<Cursor<serde_json::Value>>,
    ) -> Result<SearchResponse<ProductDocument>, opensearch::Error>;
}

pub struct ProductOpenSearchRepositoryImpl<'a> {
    client: &'a opensearch::OpenSearch,
}

impl<'a> ProductOpenSearchRepositoryImpl<'a> {
    pub fn new(client: &'a opensearch::OpenSearch) -> Self {
        ProductOpenSearchRepositoryImpl { client }
    }
}

#[async_trait]
impl<'a> ProductOpenSearchRepository for ProductOpenSearchRepositoryImpl<'a> {
    async fn create_product_documents(
        &self,
        documents: Vec<ProductDocument>,
    ) -> Result<BulkResponse, opensearch::Error> {
        let mut ops = BulkOperations::new();

        for doc in documents {
            ops.push(BulkOperation::create(doc._id(), &doc))?;
        }

        let response = self
            .client
            .bulk(BulkParts::Index("products"))
            .body(vec![ops])
            .send()
            .await?
            .error_for_status_code()?;

        let payload = response.text().await?;
        let bulk_response = serde_json::from_str::<BulkResponse>(&payload)
            .map_err(|err| {
                serde_json::Error::custom(format!(
                    "Failed deserializing 'BulkResponse' with error '{err}'. Received payload: {payload}"
                ))
            })?;

        Ok(bulk_response)
    }

    async fn update_product_documents(
        &self,
        updates: HashMap<ProductId, ProductUpdateDocument>,
    ) -> Result<BulkResponse, opensearch::Error> {
        let mut ops = BulkOperations::new();
        for (_id, doc) in updates {
            ops.push(BulkOperation::update(
                _id,
                json!({
                "doc": doc
                }),
            ))?;
        }

        let response = self
            .client
            .bulk(BulkParts::Index("products"))
            .body(vec![ops])
            .send()
            .await?
            .error_for_status_code()?;

        let payload = response.text().await?;
        let bulk_response = serde_json::from_str::<BulkResponse>(&payload)
            .map_err(|err| {
                serde_json::Error::custom(format!(
                    "Failed deserializing 'BulkResponse' with error '{err}'. Received payload: {payload}"
                ))
            })?;

        Ok(bulk_response)
    }

    async fn delete_product_documents(
        &self,
        product_ids: Vec<ProductId>,
    ) -> Result<BulkResponse, opensearch::Error> {
        let mut ops = BulkOperations::new();
        for product_id in product_ids {
            ops.push(BulkOperation::<serde_json::Value>::delete(product_id))?;
        }

        let response = self
            .client
            .bulk(BulkParts::Index("products"))
            .body(vec![ops])
            .send()
            .await?
            .error_for_status_code()?;

        let payload = response.text().await?;
        if payload.trim().is_empty() {
            return Ok(BulkResponse {
                took: 0,
                errors: false,
                items: Vec::new(),
            });
        }
        let bulk_response = serde_json::from_str::<BulkResponse>(&payload).map_err(|err| {
            serde_json::Error::custom(format!(
                "Failed deserializing 'BulkResponse' with error '{err}'. Received payload: {payload}"
            ))
        })?;

        Ok(bulk_response)
    }

    async fn search_product_documents(
        &self,
        search: &ProductSearch,
        sort: &Sort<SortProductField>,
        cursor: &Option<Cursor<serde_json::Value>>,
    ) -> Result<SearchResponse<ProductDocument>, opensearch::Error> {
        let response = self
            .client
            .search(SearchParts::Index(&["products"]))
            .body(build_search_request(search, sort, cursor)?)
            .send()
            .await?
            .error_for_status_code()?;
        let payload = response.text().await?;
        let res = serde_json::from_str(&payload).map_err(|err| {
            serde_json::Error::custom(format!(
                "Failed deserializing SearchResponse<ProductDocument>: {err}. Payload: {payload}"
            ))
        })?;
        Ok(res)
    }

    async fn search_product_documents_with_percolator_query(
        &self,
        search: &ProductSearch,
        size: u64,
    ) -> Result<SearchResponse<ProductDocument>, opensearch::Error> {
        let response = self
            .client
            .search(SearchParts::Index(&["products"]))
            .body(build_percolator_search_request(search, size)?)
            .send()
            .await?
            .error_for_status_code()?;
        let payload = response.text().await?;
        let res = serde_json::from_str(&payload).map_err(|err| {
            serde_json::Error::custom(format!(
                "Failed deserializing SearchResponse<ProductDocument>: {err}. Payload: {payload}"
            ))
        })?;
        Ok(res)
    }

    async fn get_product_document_by_id(
        &self,
        product_id: &ProductId,
    ) -> Result<ProductDocument, opensearch::Error> {
        let mut response: serde_json::Value = self
            .client
            .get(GetParts::IndexId("products", &product_id.to_string()))
            .send()
            .await?
            .error_for_status_code()?
            .json()
            .await?;

        serde_json::from_value(response["_source"].take()).map_err(opensearch::Error::from)
    }

    async fn k_nn_text(
        &self,
        embedding: &[f32],
        k: u16,
    ) -> Result<SearchResponse<ProductDocument>, opensearch::Error> {
        let body = json!({
            "_source": {
              "excludes": [ProductDocumentSerdeField::Embedding]
            },
            "size": k,
            "query": {
              "knn": {
                ProductDocumentSerdeField::Embedding.as_str() : {
                  "vector": embedding,
                  "k": k,
                }
              }
            }
        });
        let response = self
            .client
            .search(SearchParts::Index(&["products"]))
            .body(body)
            .send()
            .await?
            .error_for_status_code()?;
        let payload = response.text().await?;
        let knn_response = serde_json::from_str::<SearchResponse<ProductDocument>>(&payload)
            .map_err(|err| {
                serde_json::Error::custom(format!(
                    "Failed deserializing 'SearchResponse<ProductDocument>' with error '{err}'. Received payload: {payload}"
                ))
            })?;
        Ok(knn_response)
    }

    async fn hybrid_search_product_documents(
        &self,
        search: &ProductSearch,
        embedding: &[f32],
        cursor: &Option<Cursor<serde_json::Value>>,
    ) -> Result<SearchResponse<ProductDocument>, opensearch::Error> {
        let body = build_hybrid_search_request(search, embedding, cursor)?;
        // Pass the pipeline name as a URL query parameter so `reqwest` URL-encodes it,
        // rather than interpolating it into the path string directly.
        let pipeline_param = &[("search_pipeline", HYBRID_SEARCH_PIPELINE_NAME)];
        let response = self
            .client
            .send(
                Method::Post,
                SearchParts::Index(&["products"]).url().as_ref(),
                HeaderMap::new(),
                Some(pipeline_param),
                Some(JsonBody::new(body)),
                None,
            )
            .await?;
        let status = response.status_code();
        let payload = response.text().await?;
        if !status.is_success() {
            return Err(serde_json::Error::custom(format!(
                "hybrid_search_product_documents failed with HTTP {status}: {payload}"
            ))
            .into());
        }
        let res = serde_json::from_str::<SearchResponse<ProductDocument>>(&payload)
            .map_err(|err| {
                serde_json::Error::custom(format!(
                    "Failed deserializing 'SearchResponse<ProductDocument>' with error '{err}'. Received payload: {payload}"
                ))
            })?;
        Ok(res)
    }
}

pub fn build_search_request(
    search: &ProductSearch,
    sort: &Sort<SortProductField>,
    cursor: &Option<Cursor<serde_json::Value>>,
) -> Result<serde_json::Value, serde_json::Error> {
    let mut body = json!({
        "_source": { "excludes": [ProductDocumentSerdeField::Embedding] },
        "query": build_search_query(search)?
    });

    // ---------- Pagination ----------
    if let Some(c) = cursor {
        body["size"] = json!(c.size);
        if let Some(sa) = &c.search_after {
            body["search_after"] = opensearch_search_after(sa);
        }
    }

    // ---------- Sorting ----------
    let price_field = price_field_for(&search.currency);
    let sort_field = match sort.sort {
        SortProductField::Score => "_score",
        SortProductField::Price => price_field,
        SortProductField::Created => ProductDocumentSerdeField::Created.as_str(),
        SortProductField::Updated => ProductDocumentSerdeField::Updated.as_str(),
    };

    let order = match sort.order {
        SortOrder::Asc => "asc",
        SortOrder::Desc => "desc",
    };

    let primary_sort = if matches!(sort.sort, SortProductField::Score) {
        json!({ sort_field: { "order": order } })
    } else {
        json!({ sort_field: { "order": order, "missing": "_last" } })
    };

    body["sort"] = json!([
        primary_sort,
        { ProductDocumentSerdeField::ProductId.as_str(): { "order": "asc" } }
    ]);

    Ok(body)
}

pub fn build_percolator_search_request(
    search: &ProductSearch,
    size: u64,
) -> Result<serde_json::Value, serde_json::Error> {
    Ok(json!({
        "_source": { "excludes": [ProductDocumentSerdeField::Embedding] },
        "size": size,
        "query": build_percolator_query(search)?,
        "sort": [
            { "_score": { "order": "desc" } },
            { ProductDocumentSerdeField::ProductId.as_str(): { "order": "asc" } }
        ]
    }))
}

pub fn build_search_query(search: &ProductSearch) -> Result<serde_json::Value, serde_json::Error> {
    let mut must = Vec::with_capacity(3);

    // ---------- Text search ----------
    let title_field = title_fields(&search.language);

    if let Some(product_query_clause) =
        build_product_query_clause(&search.product_query, title_field)
    {
        must.push(product_query_clause);
    }

    let (must_not, filter) = build_filter_clauses(search)?;

    Ok(json!({
        "bool": {
            "must": must,
            "must_not": must_not,
            "filter": filter
        }
    }))
}

pub fn build_percolator_query(
    search: &ProductSearch,
) -> Result<serde_json::Value, serde_json::Error> {
    let mut must = Vec::with_capacity(1);

    if let Some(product_query_clause) =
        build_percolator_product_query_clause(&search.product_query, title_fields(&search.language))
    {
        must.push(product_query_clause);
    }

    let (must_not, filter) = build_filter_clauses(search)?;

    Ok(json!({
        "bool": {
            "must": must,
            "must_not": must_not,
            "filter": filter
        }
    }))
}

fn title_fields(language: &Language) -> ProductDocumentSerdeField {
    match language {
        Language::De => ProductDocumentSerdeField::TitleDe,
        Language::En => ProductDocumentSerdeField::TitleEn,
        Language::Fr => ProductDocumentSerdeField::TitleFr,
        Language::Es => ProductDocumentSerdeField::TitleEs,
        Language::It => ProductDocumentSerdeField::TitleIt,
        // Ingestion-only languages fall back to English for search
        _ => ProductDocumentSerdeField::TitleEn,
    }
}

fn build_product_query_clause(
    product_queries: &[TextQuery<1>],
    title_field: ProductDocumentSerdeField,
) -> Option<serde_json::Value> {
    build_any_product_query_clause(product_queries, |product_query| {
        build_text_match_clause(product_query, title_field)
    })
}

fn build_percolator_product_query_clause(
    product_queries: &[TextQuery<1>],
    title_field: ProductDocumentSerdeField,
) -> Option<serde_json::Value> {
    build_any_product_query_clause(product_queries, |product_query| {
        build_percolator_text_match_clause(product_query, title_field)
    })
}

fn build_any_product_query_clause(
    product_queries: &[TextQuery<1>],
    build_clause: impl Fn(&str) -> serde_json::Value,
) -> Option<serde_json::Value> {
    match product_queries {
        [] => None,
        [product_query] => Some(build_clause(product_query.as_ref())),
        product_queries => Some(json!({
            "bool": {
                "should": product_queries
                    .iter()
                    .map(|product_query| build_clause(product_query.as_ref()))
                    .collect::<Vec<_>>(),
                "minimum_should_match": 1
            }
        })),
    }
}

fn build_text_match_clause(
    product_query: &str,
    title_field: ProductDocumentSerdeField,
) -> serde_json::Value {
    json!({
        "bool": {
            "must": [
              {
                "bool": {
                  "should": [
                    // Primary (preferred)
                    {
                      "multi_match": {
                        "query": product_query,
                        "fields": [
                          format!("{title_field}^5")
                        ],
                        "type": "best_fields",
                        "operator": "and"
                      }
                    },
                    // Fallback ONLY
                    {
                      "bool": {
                        "must": [
                          {
                            "multi_match": {
                              "query": product_query,
                              "fields": [
                                "titleNative.text^3"
                              ],
                              "type": "best_fields",
                              "operator": "and"
                            }
                          }
                        ],
                        "boost": 0.7
                      }
                    }
                  ],
                  "minimum_should_match": 1
                }
              }
            ],
            "should": [
                // Strong exact phrase (language-specific)
                {
                    "match_phrase": {
                        title_field.as_str(): {
                            "query": product_query,
                            "boost": 6
                        }
                    }
                },

                // Add native title phrase boost (lower!)
                {
                    "match_phrase": {
                        "titleNative.text": {
                            "query": product_query,
                            "boost": 3
                        }
                    }
                },

                // Fuzzy + recall layer (language-specific)
                {
                    "match": {
                        title_field.as_str(): {
                            "query": product_query,
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

fn build_percolator_text_match_clause(
    product_query: &str,
    title_field: ProductDocumentSerdeField,
) -> serde_json::Value {
    json!({
        "multi_match": {
            "query": product_query,
            "fields": [title_field.as_str(), "titleNative.text"],
            "type": "best_fields",
            "operator": "or",
            "minimum_should_match": "4<80%"
        }
    })
}

/// Builds the structural `(must_not, filter)` clauses derived from `search`.
/// Reused by live BM25, hybrid kNN, and percolator builders to keep the filter
/// surface in lockstep.
pub fn build_filter_clauses(
    search: &ProductSearch,
) -> Result<(Vec<serde_json::Value>, Vec<serde_json::Value>), serde_json::Error> {
    let mut must_not = Vec::with_capacity(1);
    let mut filter = Vec::with_capacity(16);

    let lifecycle_terms = if search.lifecycle_query.is_empty() {
        vec![ProductLifecycleDocument::Active.as_str().to_string()]
    } else {
        search
            .lifecycle_query
            .iter()
            .map(|v| ProductLifecycleDocument::from(*v).as_str().to_string())
            .collect::<Vec<_>>()
    };
    filter.push(json!({
        "terms": {
            ProductDocumentSerdeField::Lifecycle.as_str(): lifecycle_terms
        }
    }));

    // ---------- Exclusions ----------
    if !search.exclude_product_id_query.is_empty() {
        must_not.push(json!({
            "terms": {
                ProductDocumentSerdeField::ProductId.as_str(): search.exclude_product_id_query.iter().map(|v| v.to_string()).collect::<Vec<_>>()
            }
        }));
    }
    if !search.exclude_shop_name_query.is_empty() {
        must_not.push(json!({
            "terms": {
                "shopName": search.exclude_shop_name_query.iter().map(ShopName::as_ref).collect::<Vec<_>>()
            }
        }));
    }
    if !search.exclude_seller_name_query.is_empty() {
        must_not.push(json!({
            "terms": {
                "sellerName": search.exclude_seller_name_query.iter().map(ShopName::as_ref).collect::<Vec<_>>()
            }
        }));
    }
    if !search.exclude_shop_slug_id_query.is_empty() {
        must_not.push(json!({
            "terms": {
                ProductDocumentSerdeField::ShopSlugId.as_str(): search.exclude_shop_slug_id_query.iter().map(|v| v.to_string()).collect::<Vec<_>>()
            }
        }));
    }
    if !search.exclude_seller_slug_id_query.is_empty() {
        must_not.push(json!({
            "terms": {
                ProductDocumentSerdeField::SellerSlugId.as_str(): search.exclude_seller_slug_id_query.iter().map(|v| v.to_string()).collect::<Vec<_>>()
            }
        }));
    }

    // ---------- Price ----------
    let price_field = price_field_for(&search.currency);

    if let Some(min) = search.price_query.and_then(|q| q.min) {
        filter.push(json!({ "range": { price_field: { "gte": min.deref() } } }));
    }
    if let Some(max) = search.price_query.and_then(|q| q.max) {
        filter.push(json!({ "range": { price_field: { "lte": max.deref() } } }));
    }

    if !search.country_query.is_empty() {
        filter.push(json!({
            "terms": {
                ProductDocumentSerdeField::StructuredAddressCountry.as_str(): search.country_query.iter().map(|c| c.alpha2()).collect::<Vec<_>>()
            }
        }));
    }

    if !search.continent_query.is_empty() {
        filter.push(json!({
            "terms": {
                ProductDocumentSerdeField::StructuredAddressContinent.as_str(): search.continent_query.iter().map(|c| ContinentDocument::from(*c).as_str()).collect::<Vec<_>>()
            }
        }));
    }

    if let Some(query) = search.geo_address_distance_query {
        filter.push(json!({
            "geo_distance": {
                "distance": distance_to_opensearch_value(query.distance),
                ProductDocumentSerdeField::GeoAddress.as_str(): {
                    "lat": query.lat,
                    "lon": query.lon
                }
            }
        }));
    }

    // ---------- AnyOf filters ----------
    apply_any_of_filter(
        &mut filter,
        &search.shop_name_query,
        ProductDocumentSerdeField::ShopName,
        |v| v.as_ref(),
    );

    apply_any_of_filter(
        &mut filter,
        &search.seller_name_query,
        ProductDocumentSerdeField::SellerName,
        |v| v.as_ref(),
    );

    if !search.shop_slug_id_query.is_empty() {
        filter.push(json!({
            "terms": {
                ProductDocumentSerdeField::ShopSlugId.as_str(): search.shop_slug_id_query.iter().map(|v| v.to_string()).collect::<Vec<_>>()
            }
        }));
    }

    if !search.seller_slug_id_query.is_empty() {
        filter.push(json!({
            "terms": {
                ProductDocumentSerdeField::SellerSlugId.as_str(): search.seller_slug_id_query.iter().map(|v| v.to_string()).collect::<Vec<_>>()
            }
        }));
    }

    apply_any_of_filter(
        &mut filter,
        &search
            .state_query
            .iter()
            .map(|v| ProductStateDocument::from(*v))
            .collect(),
        ProductDocumentSerdeField::State,
        |v| v.as_str(),
    );

    apply_any_of_filter(
        &mut filter,
        &search
            .shop_type_query
            .iter()
            .map(|v| ShopTypeDocument::from(*v))
            .collect(),
        ProductDocumentSerdeField::ShopType,
        |v| v.as_str(),
    );

    // ---------- Created / Updated / Auction ----------
    for (query, field) in [
        (&search.created_query, ProductDocumentSerdeField::Created),
        (&search.updated_query, ProductDocumentSerdeField::Updated),
        (
            &search.auction_start_query,
            ProductDocumentSerdeField::AuctionStart,
        ),
        (
            &search.auction_end_query,
            ProductDocumentSerdeField::AuctionEnd,
        ),
    ] {
        if let Some(min) = query.and_then(|q| q.min) {
            let v = min
                .format(&well_known::Rfc3339)
                .map_err(serde_json::Error::custom)?;
            filter.push(json!({ "range": { field.as_str(): { "gte": v } } }));
        }
        if let Some(max) = query.and_then(|q| q.max) {
            let v = max
                .format(&well_known::Rfc3339)
                .map_err(serde_json::Error::custom)?;
            filter.push(json!({ "range": { field.as_str(): { "lte": v } } }));
        }
    }

    Ok((must_not, filter))
}

fn price_field_for(currency: &Currency) -> &'static str {
    match currency {
        Currency::Eur => ProductDocumentSerdeField::PriceEur.as_str(),
        Currency::Gbp => ProductDocumentSerdeField::PriceGbp.as_str(),
        Currency::Usd => ProductDocumentSerdeField::PriceUsd.as_str(),
        Currency::Aud => ProductDocumentSerdeField::PriceAud.as_str(),
        Currency::Cad => ProductDocumentSerdeField::PriceCad.as_str(),
        Currency::Nzd => ProductDocumentSerdeField::PriceNzd.as_str(),
        Currency::Cny => ProductDocumentSerdeField::PriceCny.as_str(),
        Currency::Brl => ProductDocumentSerdeField::PriceBrl.as_str(),
        Currency::Pln => ProductDocumentSerdeField::PricePln.as_str(),
        Currency::Try => ProductDocumentSerdeField::PriceTry.as_str(),
        Currency::Jpy => ProductDocumentSerdeField::PriceJpy.as_str(),
        Currency::Czk => ProductDocumentSerdeField::PriceCzk.as_str(),
        Currency::Rub => ProductDocumentSerdeField::PriceRub.as_str(),
        Currency::Aed => ProductDocumentSerdeField::PriceAed.as_str(),
        Currency::Sar => ProductDocumentSerdeField::PriceSar.as_str(),
        Currency::Hkd => ProductDocumentSerdeField::PriceHkd.as_str(),
        Currency::Sgd => ProductDocumentSerdeField::PriceSgd.as_str(),
        Currency::Chf => ProductDocumentSerdeField::PriceChf.as_str(),
    }
}

/// Builds the request body for an OpenSearch *native hybrid* search.
///
/// Combines a BM25 sub-query (text-match + filters) and a kNN sub-query in a single
/// `hybrid` query. Score fusion is performed by the pre-registered search pipeline
/// referenced by [`HYBRID_SEARCH_PIPELINE_NAME`], which is passed as the
/// `search_pipeline` URL query parameter by the caller.
///
/// Pagination uses standard `search_after` over `[_score desc]`.
pub fn build_hybrid_search_request(
    search: &ProductSearch,
    embedding: &[f32],
    cursor: &Option<Cursor<serde_json::Value>>,
) -> Result<serde_json::Value, serde_json::Error> {
    let (must_not, filter) = build_filter_clauses(search)?;

    // ---------- BM25 sub-query: text-match + filters ----------
    let title_field = title_fields(&search.language);
    let bm25_text_clause = build_product_query_clause(&search.product_query, title_field)
        // Defensive: hybrid search is only meaningful with a text query, but fall back to
        // a match-all so the request is still valid.
        .unwrap_or_else(|| json!({ "match_all": {} }));
    let bm25_subquery = json!({
        "bool": {
            "must": [bm25_text_clause],
            "filter": filter.clone(),
            "must_not": must_not.clone(),
        }
    });

    // ---------- kNN sub-query: vector + filters ----------
    let page_size = hybrid_page_size(cursor);
    let mut knn_body = json!({
        "vector": embedding,
        "k": HYBRID_K,
    });
    if !filter.is_empty() || !must_not.is_empty() {
        knn_body["filter"] = json!({
            "bool": {
                "must_not": must_not,
                "filter": filter,
            }
        });
    }
    let knn_subquery = json!({
        "knn": {
            ProductDocumentSerdeField::Embedding.as_str(): knn_body,
        }
    });

    let mut body = json!({
        "_source": {
            "excludes": [ProductDocumentSerdeField::Embedding]
        },
        "size": page_size,
        "query": {
            "hybrid": {
                "queries": [bm25_subquery, knn_subquery]
            }
        },
        // Sort by the RRF-fused score through a numeric script so OpenSearch emits a concrete
        // sort value that can be fed back via `search_after`. Native hybrid rejects `_score` as
        // a `search_after` sort key, and also forbids adding a second field sort criterion.
        // The tiny productId-derived tiebreaker keeps cursor paging stable when scores tie
        // without materially changing relevance ordering.
        "sort": [
            {
                "_script": {
                    "type": "number",
                    "script": {
                        "source": format!(
                            "return _score + (Math.abs(doc['{}'].value.hashCode()) * 1.0e-15);",
                            ProductDocumentSerdeField::ProductId.as_str()
                        )
                    },
                    "order": "desc"
                }
            }
        ]
    });

    if let Some(c) = cursor
        && let Some(sa) = &c.search_after
    {
        body["search_after"] = opensearch_search_after(sa);
    }

    Ok(body)
}

fn opensearch_search_after(search_after: &serde_json::Value) -> serde_json::Value {
    match search_after {
        serde_json::Value::Array(_) => search_after.clone(),
        _ => serde_json::Value::Array(vec![search_after.clone()]),
    }
}

fn hybrid_page_size(cursor: &Option<Cursor<serde_json::Value>>) -> u64 {
    cursor
        .as_ref()
        .map(|c| c.size)
        .unwrap_or(DEFAULT_HYBRID_PAGE_SIZE)
        .max(1)
}

fn apply_any_of_filter<T: Hash + Eq + EnumCount>(
    filter: &mut Vec<serde_json::Value>,
    query: &AnyOfQuery<T>,
    field: ProductDocumentSerdeField,
    to_str: fn(&T) -> &str,
) {
    let values: Vec<&str> = query.iter().map(to_str).collect();
    if !values.is_empty() && values.len() != T::COUNT {
        filter.push(json!({ "terms": { field.as_str(): values } }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::product_state::domain::ProductState;

    fn search_with_product_query(product_query: &str) -> ProductSearch {
        ProductSearch::new(Language::En, Currency::Eur)
            .with_product_query(product_query.try_into().unwrap())
    }

    fn search_with_product_queries(product_queries: &[&str]) -> ProductSearch {
        product_queries.iter().fold(
            ProductSearch::new(Language::En, Currency::Eur),
            |search, product_query| search.with_product_query((*product_query).try_into().unwrap()),
        )
    }

    #[test]
    fn should_build_live_search_query_without_percolator_score_workaround() {
        let mut search = ProductSearch::new(Language::En, Currency::Eur);
        search.state_query = [ProductState::Listed].into_iter().collect();

        let actual = build_search_query(&search).unwrap();

        assert!(actual.get("constant_score").is_none());
        assert_eq!(
            actual.pointer("/bool/filter/0/terms/lifecycle"),
            Some(&json!(["ACTIVE"]))
        );
        assert_eq!(
            actual.pointer("/bool/filter/1/terms/state"),
            Some(&json!(["LISTED"]))
        );
    }

    #[test]
    fn should_build_live_search_query_with_or_over_multiple_product_queries() {
        let search =
            search_with_product_queries(&["Madonna oil painting", "Virgin Mary oil painting"]);

        let actual = build_search_query(&search).unwrap();

        assert_eq!(
            actual.pointer("/bool/must/0/bool/minimum_should_match"),
            Some(&json!(1))
        );
        assert_eq!(
            actual
                .pointer("/bool/must/0/bool/should/0/bool/must/0/bool/should/0/multi_match/query"),
            Some(&json!("Madonna oil painting"))
        );
        assert_eq!(
            actual
                .pointer("/bool/must/0/bool/should/1/bool/must/0/bool/should/0/multi_match/query"),
            Some(&json!("Virgin Mary oil painting"))
        );
    }

    #[test]
    fn should_build_search_query_with_excluded_product_ids() {
        let excluded_product_id = ProductId::new();
        let mut search = ProductSearch::new(Language::En, Currency::Eur);
        search.exclude_product_id_query = [excluded_product_id].into_iter().collect();

        let actual = build_search_query(&search).unwrap();

        assert_eq!(
            actual.pointer("/bool/must_not/0/terms/productId"),
            Some(&json!([excluded_product_id.to_string()]))
        );
    }

    #[test]
    fn should_build_percolator_query_with_or_over_multiple_product_queries() {
        let search =
            search_with_product_queries(&["Madonna oil painting", "Virgin Mary oil painting"]);

        let actual = build_percolator_query(&search).unwrap();

        assert_eq!(
            actual.pointer("/bool/must/0/bool/minimum_should_match"),
            Some(&json!(1))
        );
        assert_eq!(
            actual.pointer("/bool/must/0/bool/should/0/multi_match/query"),
            Some(&json!("Madonna oil painting"))
        );
        assert_eq!(
            actual.pointer("/bool/must/0/bool/should/1/multi_match/query"),
            Some(&json!("Virgin Mary oil painting"))
        );
    }

    #[test]
    fn should_build_percolator_search_request_without_pagination_cursor() {
        let search = search_with_product_query("Ming dynasty blue white porcelain vase");

        let actual = build_percolator_search_request(&search, 10).unwrap();

        assert_eq!(actual.pointer("/size"), Some(&json!(10)));
        assert!(actual.get("search_after").is_none());
        assert_eq!(
            actual.pointer("/query/bool/must/0/multi_match/minimum_should_match"),
            Some(&json!("4<80%"))
        );
    }

    #[test]
    fn should_build_percolator_text_query_with_minimum_should_match() {
        let search = search_with_product_query("Ming dynasty blue white porcelain vase");

        let actual = build_percolator_query(&search).unwrap();

        assert_eq!(
            actual.pointer("/bool/must/0/multi_match/minimum_should_match"),
            Some(&json!("4<80%"))
        );
        assert_eq!(
            actual.pointer("/bool/must/0/multi_match/operator"),
            Some(&json!("or"))
        );
    }

    #[test]
    fn should_preserve_full_user_query_when_building_percolator_text_query() {
        let search = search_with_product_query("Antique art Ming porcelain vase");

        let actual = build_percolator_query(&search).unwrap();

        assert_eq!(
            actual.pointer("/bool/must/0/multi_match/query"),
            Some(&json!("Antique art Ming porcelain vase"))
        );
    }
}
