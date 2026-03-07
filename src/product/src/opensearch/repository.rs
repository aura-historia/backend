use crate::core::product_search::ProductSearch;
use crate::core::sort_product_field::SortProductField;
use crate::opensearch::authenticity_document::AuthenticityDocument;
use crate::opensearch::condition_document::ConditionDocument;
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
use opensearch::{BulkOperation, BulkOperations, BulkParts, GetParts, SearchParts};
use serde::ser::Error;
use serde_json::json;
use shop::opensearch::shop_type_document::ShopTypeDocument;
use std::collections::HashMap;
use std::hash::Hash;
use std::ops::Deref;
use strum::EnumCount;
use time::format_description::well_known;

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
        text_embedding: &[f32],
        k: u16,
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
        let mut must = Vec::with_capacity(3);
        let mut must_not = Vec::with_capacity(1);
        let mut filter = Vec::with_capacity(16);

        // ---------- Text search ----------
        let (title_field, description_field) = match search.language {
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
        };

        if let Some(product_query) = search.product_query.as_ref() {
            must.push(json!({
                "multi_match": {
                    "query": product_query,
                    "fields": [
                        format!("{title_field}^3"),
                        format!("{description_field}^1"),
                    ],
                    "fuzziness": "AUTO",
                    "minimum_should_match": "70%"
                }
            }));
        }

        // ---------- Exclusions ----------
        if !search.exclude_shop_name_query.is_empty() {
            must_not.push(json!({
                "terms": {
                    "shopName": search.exclude_shop_name_query.iter().map(ShopName::as_ref).collect::<Vec<_>>()
                }
            }));
        }

        // ---------- Price ----------
        let price_field = match search.currency {
            Currency::Eur => ProductDocumentSerdeField::PriceEur.as_str(),
            Currency::Gbp => ProductDocumentSerdeField::PriceGbp.as_str(),
            Currency::Usd => ProductDocumentSerdeField::PriceUsd.as_str(),
            Currency::Aud => ProductDocumentSerdeField::PriceAud.as_str(),
            Currency::Cad => ProductDocumentSerdeField::PriceCad.as_str(),
            Currency::Nzd => ProductDocumentSerdeField::PriceNzd.as_str(),
        };

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

        // ---------- Source ----------
        let mut source_excludes = ProductDocumentSerdeField::description_fields();
        source_excludes.push(ProductDocumentSerdeField::TextEmbedding);

        // ---------- Primary Body ----------
        let mut body = json!({
            "_source": { "excludes": source_excludes },
            "query": {
                "bool": {
                    "must": must,
                    "must_not": must_not,
                    "filter": filter
                }
            }
        });

        // ---------- Pagination ----------
        if let Some(c) = cursor {
            body["size"] = json!(c.size);
            if let Some(sa) = &c.search_after {
                body["search_after"] = json!(sa);
            }
        }

        // ---------- Sorting ----------
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

        // ---------- Execute ----------
        let response = self
            .client
            .search(SearchParts::Index(&["products"]))
            .body(body)
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
        text_embedding: &[f32],
        k: u16,
    ) -> Result<SearchResponse<ProductDocument>, opensearch::Error> {
        let mut source_excludes = ProductDocumentSerdeField::description_fields();
        source_excludes.push(ProductDocumentSerdeField::TextEmbedding);

        let body = json!({
            "_source": {
              "excludes": source_excludes
            },
            "size": k,
            "query": {
              "knn": {
                ProductDocumentSerdeField::TextEmbedding.as_str() : {
                  "vector": text_embedding,
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
