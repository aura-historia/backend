use crate::core::product_search::ProductSearch;
use crate::core::sort_product_field::SortProductField;
use crate::opensearch::authenticity_document::AuthenticityDocument;
use crate::opensearch::condition_document::ConditionDocument;
use crate::opensearch::intent::HybridSearchParams;
use crate::opensearch::product_document::{ProductDocument, ProductDocumentSerdeField};
use crate::opensearch::product_state_document::ProductStateDocument;
use crate::opensearch::product_update_document::ProductUpdateDocument;
use crate::opensearch::provenance_document::ProvenanceDocument;
use crate::opensearch::restoration_document::RestorationDocument;
use async_trait::async_trait;
use common::currency::domain::Currency;
use common::language::domain::Language;
use common::opensearch::{bulk_response::BulkResponse, search_response::SearchResponse};
use common::pagination::cursor::Cursor;
use common::product_id::ProductId;
use common::query::any_of_query::AnyOfQuery;
use common::shop_name::ShopName;
use common::sort::{Sort, SortOrder};
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

    async fn search_product_documents(
        &self,
        search: &ProductSearch,
        sort: &Sort<SortProductField>,
        page: &Option<Cursor<serde_json::Value>>,
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
        params: HybridSearchParams,
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
        let mut source_excludes = ProductDocumentSerdeField::description_fields();
        source_excludes.push(ProductDocumentSerdeField::Embedding);

        let body = json!({
            "_source": {
              "excludes": source_excludes
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
        params: HybridSearchParams,
        cursor: &Option<Cursor<serde_json::Value>>,
    ) -> Result<SearchResponse<ProductDocument>, opensearch::Error> {
        let body = build_hybrid_search_request(search, embedding, params, cursor)?;
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
    let mut source_excludes = ProductDocumentSerdeField::description_fields();
    source_excludes.push(ProductDocumentSerdeField::Embedding);
    let mut body = json!({
        "_source": { "excludes": source_excludes },
        "query": build_search_query(search)?
    });

    // ---------- Pagination ----------
    if let Some(c) = cursor {
        body["size"] = json!(c.size);
        if let Some(sa) = &c.search_after {
            body["search_after"] = json!(sa);
        }
    }

    // ---------- Sorting ----------
    let price_field = price_field_for(&search.currency);
    let sort_field = match sort.sort {
        SortProductField::Score => "_score",
        SortProductField::Price => price_field,
        SortProductField::OriginYear => ProductDocumentSerdeField::OriginYear.as_str(),
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

pub fn build_search_query(search: &ProductSearch) -> Result<serde_json::Value, serde_json::Error> {
    let mut must = Vec::with_capacity(3);

    // ---------- Text search ----------
    let (title_field, description_field) = title_and_description_fields(&search.language);

    if let Some(product_query) = search.product_query.as_ref() {
        must.push(build_text_match_clause(
            product_query.as_ref(),
            title_field,
            description_field,
        ));
    }

    let (must_not, filter) = build_filter_clauses(search)?;

    // When there are no scoring clauses (no text query), a plain `bool` filter-only query
    // produces a relevance score of 0.0 in OpenSearch. This falls below the percolation
    // min_score threshold used in the search-filter percolator, which means filter-only
    // search alerts (e.g. "state = Listed") would never trigger any matches.
    //
    // Wrapping in `constant_score` gives every matching document a fixed boost above the
    // percolation min_score threshold (currently 3.1) so filter-only queries are returned
    // correctly while text queries continue to use real BM25 relevance scoring.
    if must.is_empty() {
        Ok(json!({
            "constant_score": {
                "filter": {
                    "bool": {
                        "must_not": must_not,
                        "filter": filter
                    }
                },
                "boost": 4.0
            }
        }))
    } else {
        Ok(json!({
            "bool": {
                "must": must,
                "must_not": must_not,
                "filter": filter
            }
        }))
    }
}

fn title_and_description_fields(
    language: &Language,
) -> (ProductDocumentSerdeField, ProductDocumentSerdeField) {
    match language {
        Language::De => (
            ProductDocumentSerdeField::TitleDe,
            ProductDocumentSerdeField::DescriptionDe,
        ),
        Language::En => (
            ProductDocumentSerdeField::TitleEn,
            ProductDocumentSerdeField::DescriptionEn,
        ),
        Language::Fr => (
            ProductDocumentSerdeField::TitleFr,
            ProductDocumentSerdeField::DescriptionFr,
        ),
        Language::Es => (
            ProductDocumentSerdeField::TitleEs,
            ProductDocumentSerdeField::DescriptionEs,
        ),
        Language::It => (
            ProductDocumentSerdeField::TitleIt,
            ProductDocumentSerdeField::DescriptionIt,
        ),
        // Ingestion-only languages fall back to English for search
        _ => (
            ProductDocumentSerdeField::TitleEn,
            ProductDocumentSerdeField::DescriptionEn,
        ),
    }
}

fn build_text_match_clause(
    product_query: &str,
    title_field: ProductDocumentSerdeField,
    description_field: ProductDocumentSerdeField,
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
                    "multi_match": {
                        "query": product_query,
                        "fields": [
                            format!("{title_field}^3"),
                            format!("{description_field}^1")
                        ],
                        "type": "best_fields",
                        "fuzziness": "AUTO:4,6",
                        "minimum_should_match": "2<75%"
                    }
                }
            ]
        }
    })
}

/// Builds the `(must_not, filter)` clauses derived from `search` (everything except the
/// BM25 text-match part). Reused by both BM25 and kNN search builders to keep the filter
/// surface in lockstep.
pub fn build_filter_clauses(
    search: &ProductSearch,
) -> Result<(Vec<serde_json::Value>, Vec<serde_json::Value>), serde_json::Error> {
    let mut must_not = Vec::with_capacity(1);
    let mut filter = Vec::with_capacity(16);

    // ---------- Exclusions ----------
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

    if !search.category_id.is_empty() {
        filter.push(json!({
            "terms": {
                ProductDocumentSerdeField::CategoryId.as_str(): search.category_id.iter().collect::<Vec<_>>()
            }
        }));
    }

    if !search.period_id.is_empty() {
        filter.push(json!({
            "terms": {
                ProductDocumentSerdeField::PeriodId.as_str(): search.period_id.iter().collect::<Vec<_>>()
            }
        }));
    }

    if !search.countries.is_empty() {
        filter.push(json!({
            "terms": {
                ProductDocumentSerdeField::StructuredAddressCountry.as_str(): search.countries.iter().map(|c| c.alpha2()).collect::<Vec<_>>()
            }
        }));
    }

    if !search.continents.is_empty() {
        filter.push(json!({
            "terms": {
                ProductDocumentSerdeField::StructuredAddressContinent.as_str(): search.continents.iter().map(|c| ContinentDocument::from(*c).as_str()).collect::<Vec<_>>()
            }
        }));
    }

    if let Some(min) = search.geo_address_lat_query.and_then(|q| q.min) {
        filter.push(json!({ "range": { ProductDocumentSerdeField::GeoAddressLat.as_str(): { "gte": min } } }));
    }
    if let Some(max) = search.geo_address_lat_query.and_then(|q| q.max) {
        filter.push(json!({ "range": { ProductDocumentSerdeField::GeoAddressLat.as_str(): { "lte": max } } }));
    }
    if let Some(min) = search.geo_address_lon_query.and_then(|q| q.min) {
        filter.push(json!({ "range": { ProductDocumentSerdeField::GeoAddressLon.as_str(): { "gte": min } } }));
    }
    if let Some(max) = search.geo_address_lon_query.and_then(|q| q.max) {
        filter.push(json!({ "range": { ProductDocumentSerdeField::GeoAddressLon.as_str(): { "lte": max } } }));
    }

    // ---------- Origin year (overlap semantics) ----------
    if let Some(origin_query) = &search.origin_year_query {
        let mut should = Vec::new();

        match (origin_query.min, origin_query.max) {
            (None, None) => {}
            (Some(qmin), Some(qmax)) if qmin == qmax => {
                should.push(json!({
                    "term": {
                        ProductDocumentSerdeField::OriginYear.as_str(): qmin
                    }
                }));
            }
            (qmin, qmax) => {
                let mut must = Vec::new();
                if let Some(qmax) = qmax {
                    must.push(json!({
                        "range": {
                            ProductDocumentSerdeField::OriginYearMin.as_str(): {
                                "lte": qmax
                            }
                        }
                    }));
                }
                if let Some(qmin) = qmin {
                    must.push(json!({
                        "range": {
                            ProductDocumentSerdeField::OriginYearMax.as_str(): {
                                "gte": qmin
                            }
                        }
                    }));
                }
                should.push(json!({
                    "bool": {
                        "must": must
                    }
                }));
            }
        }

        if !should.is_empty() {
            filter.push(json!({
                "bool": {
                    "should": should,
                    "minimum_should_match": 1
                }
            }));
        }
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
            .authenticity_query
            .iter()
            .map(|v| AuthenticityDocument::from(*v))
            .collect(),
        ProductDocumentSerdeField::Authenticity,
        |v| v.as_str(),
    );

    apply_any_of_filter(
        &mut filter,
        &search
            .condition_query
            .iter()
            .map(|v| ConditionDocument::from(*v))
            .collect(),
        ProductDocumentSerdeField::Condition,
        |v| v.as_str(),
    );

    apply_any_of_filter(
        &mut filter,
        &search
            .provenance_query
            .iter()
            .map(|v| ProvenanceDocument::from(*v))
            .collect(),
        ProductDocumentSerdeField::Provenance,
        |v| v.as_str(),
    );

    apply_any_of_filter(
        &mut filter,
        &search
            .restoration_query
            .iter()
            .map(|v| RestorationDocument::from(*v))
            .collect(),
        ProductDocumentSerdeField::Restoration,
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
    params: HybridSearchParams,
    cursor: &Option<Cursor<serde_json::Value>>,
) -> Result<serde_json::Value, serde_json::Error> {
    let (must_not, filter) = build_filter_clauses(search)?;

    let mut source_excludes = ProductDocumentSerdeField::description_fields();
    source_excludes.push(ProductDocumentSerdeField::Embedding);

    // ---------- BM25 sub-query: text-match + filters ----------
    let (title_field, description_field) = title_and_description_fields(&search.language);
    let bm25_text_clause = match search.product_query.as_ref() {
        Some(q) => build_text_match_clause(q.as_ref(), title_field, description_field),
        // Defensive: hybrid search is only meaningful with a text query, but fall back to
        // a match-all so the request is still valid.
        None => json!({ "match_all": {} }),
    };
    let bm25_subquery = if filter.is_empty() && must_not.is_empty() {
        bm25_text_clause
    } else {
        json!({
            "bool": {
                "must": [bm25_text_clause],
                "filter": filter.clone(),
                "must_not": must_not.clone(),
            }
        })
    };

    // ---------- kNN sub-query: vector + filters ----------
    let mut knn_body = json!({
        "vector": embedding,
        "k": params.candidate_k,
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

    let page_size = cursor.as_ref().map(|c| c.size).unwrap_or(20).max(1);
    let mut body = json!({
        "_source": { "excludes": source_excludes },
        "size": page_size,
        "query": {
            "hybrid": {
                "queries": [bm25_subquery, knn_subquery]
            }
        },
        // OpenSearch's `hybrid` query type forbids combining `_score` with any other sort
        // criterion — only a single sort field is allowed.  Sorting by `_score` preserves the
        // pipeline's RRF-fused relevance ordering so the most-relevant documents come first.
        // Ties on score (rare in practice) may produce non-deterministic page splits, which is
        // acceptable given that relevance ordering is the primary concern.
        "sort": [
            { "_score": { "order": "desc" } }
        ]
    });

    if let Some(c) = cursor
        && let Some(sa) = &c.search_after
    {
        body["search_after"] = sa.clone();
    }

    Ok(body)
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
