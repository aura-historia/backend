use crate::continent_document::ContinentDocument;
use crate::product_document::{ProductDocument, ProductDocumentSerdeField};
use crate::product_state_document::ProductStateDocument;
use crate::shop_type_document::ShopTypeDocument;
use common::currency::domain::Currency;
use common::language::domain::Language;

use common::localized::Localized;
use common::opensearch::search_response::{SearchHit, SearchResponse};
use common::pagination::cursor::{Cursor, CursoredResult};
use common::price::domain::{MonetaryAmount, Price};
use common::product_lifecycle::document::ProductLifecycleDocument;
use common::query::any_of_query::AnyOfQuery;
use common::query::text_query::TextQuery;
use common::shop_name::ShopName;
use common::sort::{Sort, SortOrder};
use opensearch::http::Method;
use opensearch::http::headers::HeaderMap;
use opensearch::http::request::JsonBody;
use opensearch::{OpenSearch, SearchParts};
use product_core::product_search::ProductSearch;
use product_core::sort_product_field::SortProductField;
use product_core::title::Title;
use product_service::ports::{ProductSearchReadError, ProductSearchReader};
use product_service::use_cases::queries::search_products::{
    ProductSearchReadResult, ProductSummary, SearchProductsRequest,
};
use serde::ser::Error;
use serde_json::json;
use std::collections::HashMap;
use std::hash::Hash;
use std::ops::Deref;
use strum::EnumCount;
use time::format_description::well_known;

const DEFAULT_INDEX: &str = "products";
const HYBRID_SEARCH_PIPELINE_NAME: &str = "hybrid-search-pipeline";
const DEFAULT_HYBRID_PAGE_SIZE: u64 = 20;
const HYBRID_K: u16 = 100;

#[derive(Clone)]
pub struct OpenSearchProductSearchReader {
    client: OpenSearch,
    index: String,
}

impl OpenSearchProductSearchReader {
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
impl ProductSearchReader for OpenSearchProductSearchReader {
    #[tracing::instrument(name = "opensearch_product_search", skip_all)]
    async fn search(
        &self,
        request: &SearchProductsRequest,
    ) -> Result<ProductSearchReadResult, ProductSearchReadError> {
        let sort = request.sort.unwrap_or(Sort {
            sort: SortProductField::Score,
            order: SortOrder::Desc,
        });
        let body = build_search_request(&request.search, &sort, &request.cursor)
            .map_err(|_| ProductSearchReadError::ProductSearchReadModelInvalid)?;
        let response = self
            .client
            .search(SearchParts::Index(&[self.index.as_str()]))
            .body(body)
            .send()
            .await
            .map_err(|_| ProductSearchReadError::ProductSearchQueryFailed)?
            .error_for_status_code()
            .map_err(|_| ProductSearchReadError::ProductSearchQueryFailed)?;
        let payload = response
            .text()
            .await
            .map_err(|_| ProductSearchReadError::ProductSearchQueryFailed)?;
        let search_response = serde_json::from_str::<SearchResponse<ProductDocument>>(&payload)
            .map_err(|_| ProductSearchReadError::ProductSearchReadModelInvalid)?
            .into_non_timed_out("product search")
            .map_err(|_| ProductSearchReadError::ProductSearchQueryFailed)?;

        Ok(map_search_response(&request.search, search_response))
    }

    #[tracing::instrument(name = "opensearch_product_hybrid_search", skip_all)]
    async fn search_hybrid(
        &self,
        request: &SearchProductsRequest,
        embedding: &[f32],
    ) -> Result<ProductSearchReadResult, ProductSearchReadError> {
        let body = build_hybrid_search_request(&request.search, embedding, &request.cursor)
            .map_err(|_| ProductSearchReadError::ProductSearchReadModelInvalid)?;
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
            .map_err(|_| ProductSearchReadError::ProductSearchQueryFailed)?;
        if !response.status_code().is_success() {
            return Err(ProductSearchReadError::ProductSearchQueryFailed);
        }
        let payload = response
            .text()
            .await
            .map_err(|_| ProductSearchReadError::ProductSearchQueryFailed)?;
        let search_response = serde_json::from_str::<SearchResponse<ProductDocument>>(&payload)
            .map_err(|_| ProductSearchReadError::ProductSearchReadModelInvalid)?
            .into_non_timed_out("product hybrid search")
            .map_err(|_| ProductSearchReadError::ProductSearchQueryFailed)?;

        Ok(map_hybrid_search_response(
            &request.search,
            search_response,
            &request.cursor,
        ))
    }
}

pub(crate) fn map_search_response(
    search: &ProductSearch,
    search_response: SearchResponse<ProductDocument>,
) -> ProductSearchReadResult {
    CursoredResult {
        cursor: Cursor {
            size: search_response.hits.hits.len() as u64,
            search_after: search_response
                .hits
                .hits
                .last()
                .and_then(|last| last.sort.clone()),
        },
        items: search_response
            .hits
            .hits
            .into_iter()
            .map(|hit| map_summary(search, hit))
            .collect(),
        total: Some(search_response.hits.total.value),
    }
}

fn map_summary(search: &ProductSearch, hit: SearchHit<ProductDocument>) -> ProductSummary {
    let document = hit.source;
    let title = resolve_title(&document, search.language);
    let price = resolve_price(&document, search.currency);
    ProductSummary {
        product_id: document.product_id,
        product_slug_id: document.product_slug_id,
        event_id: document.event_id,
        shop_id: document.shop_id,
        seller_id: document.seller_id,
        shops_product_id: document.shops_product_id,
        shop_name: ShopName::from(document.shop_name),
        shop_slug_id: document.shop_slug_id,
        title,
        price,
        state: document.state.into(),
        lifecycle: document.lifecycle.into(),
        url: document.url,
        view_url: document.view_url,
        images: document.images.into_iter().map(Into::into).collect(),
        updated: document.updated,
    }
}

fn resolve_title(
    document: &ProductDocument,
    preferred_language: Language,
) -> Option<Localized<Language, Title>> {
    let mut titles = HashMap::new();
    insert_title(&mut titles, Language::De, &document.title_de);
    insert_title(&mut titles, Language::En, &document.title_en);
    insert_title(&mut titles, Language::Fr, &document.title_fr);
    insert_title(&mut titles, Language::Es, &document.title_es);
    insert_title(&mut titles, Language::It, &document.title_it);
    titles
        .entry(Language::from(document.title.language))
        .or_insert_with(|| Title::from(document.title.text.clone()));
    Language::resolve(&[preferred_language], titles)
}

fn insert_title(titles: &mut HashMap<Language, Title>, language: Language, title: &Option<String>) {
    if let Some(title) = title {
        titles.entry(language).or_insert_with(|| Title::from(title));
    }
}

fn resolve_price(document: &ProductDocument, preferred_currency: Currency) -> Option<Price> {
    let mut prices = HashMap::new();
    insert_price(&mut prices, Currency::Eur, document.price_eur);
    insert_price(&mut prices, Currency::Usd, document.price_usd);
    insert_price(&mut prices, Currency::Gbp, document.price_gbp);
    insert_price(&mut prices, Currency::Aud, document.price_aud);
    insert_price(&mut prices, Currency::Cad, document.price_cad);
    insert_price(&mut prices, Currency::Nzd, document.price_nzd);
    insert_price(&mut prices, Currency::Cny, document.price_cny);
    insert_price(&mut prices, Currency::Brl, document.price_brl);
    insert_price(&mut prices, Currency::Pln, document.price_pln);
    insert_price(&mut prices, Currency::Try, document.price_try);
    insert_price(&mut prices, Currency::Jpy, document.price_jpy);
    insert_price(&mut prices, Currency::Czk, document.price_czk);
    insert_price(&mut prices, Currency::Rub, document.price_rub);
    insert_price(&mut prices, Currency::Aed, document.price_aed);
    insert_price(&mut prices, Currency::Sar, document.price_sar);
    insert_price(&mut prices, Currency::Hkd, document.price_hkd);
    insert_price(&mut prices, Currency::Sgd, document.price_sgd);
    insert_price(&mut prices, Currency::Chf, document.price_chf);
    Currency::resolve(&[preferred_currency], prices)
}

fn insert_price(
    prices: &mut HashMap<Currency, MonetaryAmount>,
    currency: Currency,
    amount: Option<u64>,
) {
    if let Some(amount) = amount {
        prices.insert(currency, MonetaryAmount::from(amount));
    }
}

fn map_hybrid_search_response(
    search: &ProductSearch,
    search_response: SearchResponse<ProductDocument>,
    cursor: &Option<Cursor<serde_json::Value>>,
) -> ProductSearchReadResult {
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

    ProductSearchReadResult {
        cursor: Cursor {
            size: item_count,
            search_after,
        },
        items: search_response
            .hits
            .hits
            .into_iter()
            .map(|hit| map_summary(search, hit))
            .collect(),
        total: None,
    }
}

pub(crate) fn build_search_request(
    search: &ProductSearch,
    sort: &Sort<SortProductField>,
    cursor: &Option<Cursor<serde_json::Value>>,
) -> Result<serde_json::Value, serde_json::Error> {
    let mut body = json!({
        "_source": { "excludes": [ProductDocumentSerdeField::Embedding] },
        "query": build_search_query(search)?
    });

    if let Some(cursor) = cursor {
        body["size"] = json!(cursor.size);
        if let Some(search_after) = &cursor.search_after {
            body["search_after"] = opensearch_search_after(search_after);
        }
    }

    let sort_field = match sort.sort {
        SortProductField::Score => "_score",
        SortProductField::Price => price_field_for(&search.currency),
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

pub(crate) fn build_search_query(
    search: &ProductSearch,
) -> Result<serde_json::Value, serde_json::Error> {
    let mut must = Vec::with_capacity(1);
    if let Some(product_query_clause) =
        build_product_query_clause(&search.product_query, title_field(&search.language))
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

pub(crate) fn build_hybrid_search_request(
    search: &ProductSearch,
    embedding: &[f32],
    cursor: &Option<Cursor<serde_json::Value>>,
) -> Result<serde_json::Value, serde_json::Error> {
    let (must_not, filter) = build_filter_clauses(search)?;
    let title_field = title_field(&search.language);
    let bm25_text = build_product_query_clause(&search.product_query, title_field)
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
        "_source": { "excludes": [ProductDocumentSerdeField::Embedding] },
        "size": cursor
            .as_ref()
            .map(|cursor| cursor.size)
            .unwrap_or(DEFAULT_HYBRID_PAGE_SIZE)
            .max(1),
        "query": {
            "hybrid": {
                "queries": [
                    bm25,
                    { "knn": { ProductDocumentSerdeField::Embedding.as_str(): knn } }
                ]
            }
        },
        "sort": [{
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

fn title_field(language: &Language) -> ProductDocumentSerdeField {
    match language {
        Language::De => ProductDocumentSerdeField::TitleDe,
        Language::En => ProductDocumentSerdeField::TitleEn,
        Language::Fr => ProductDocumentSerdeField::TitleFr,
        Language::Es => ProductDocumentSerdeField::TitleEs,
        Language::It => ProductDocumentSerdeField::TitleIt,
        _ => ProductDocumentSerdeField::TitleEn,
    }
}

fn build_product_query_clause(
    product_queries: &[TextQuery<1>],
    title_field: ProductDocumentSerdeField,
) -> Option<serde_json::Value> {
    match product_queries {
        [] => None,
        [product_query] => Some(build_text_match_clause(product_query.as_ref(), title_field)),
        product_queries => Some(json!({
            "bool": {
                "should": product_queries
                    .iter()
                    .map(|product_query| build_text_match_clause(product_query.as_ref(), title_field))
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
            "must": [{
                "bool": {
                    "should": [
                        {
                            "multi_match": {
                                "query": product_query,
                                "fields": [format!("{title_field}^5")],
                                "type": "best_fields",
                                "operator": "and"
                            }
                        },
                        {
                            "bool": {
                                "must": [{
                                    "multi_match": {
                                        "query": product_query,
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
                            "query": product_query,
                            "boost": 6
                        }
                    }
                },
                {
                    "match_phrase": {
                        "title.text": {
                            "query": product_query,
                            "boost": 3
                        }
                    }
                },
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

pub(crate) fn build_filter_clauses(
    search: &ProductSearch,
) -> Result<(Vec<serde_json::Value>, Vec<serde_json::Value>), serde_json::Error> {
    let mut must_not = Vec::new();
    let mut filter = Vec::new();

    let lifecycle_terms = if search.lifecycle_query.is_empty() {
        vec![ProductLifecycleDocument::Active.as_str().to_owned()]
    } else {
        search
            .lifecycle_query
            .iter()
            .map(|value| ProductLifecycleDocument::from(*value).as_str().to_owned())
            .collect()
    };
    filter.push(json!({
        "terms": {
            ProductDocumentSerdeField::Lifecycle.as_str(): lifecycle_terms
        }
    }));

    if !search.exclude_product_id_query.is_empty() {
        must_not.push(json!({
            "terms": {
                ProductDocumentSerdeField::ProductId.as_str(): search.exclude_product_id_query.iter().map(ToString::to_string).collect::<Vec<_>>()
            }
        }));
    }
    if !search.exclude_shop_name_query.is_empty() {
        must_not.push(json!({
            "terms": {
                ProductDocumentSerdeField::ShopName.as_str(): search.exclude_shop_name_query.iter().map(ShopName::as_ref).collect::<Vec<_>>()
            }
        }));
    }
    if !search.exclude_seller_name_query.is_empty() {
        must_not.push(json!({
            "terms": {
                ProductDocumentSerdeField::SellerName.as_str(): search.exclude_seller_name_query.iter().map(ShopName::as_ref).collect::<Vec<_>>()
            }
        }));
    }
    if !search.exclude_shop_slug_id_query.is_empty() {
        must_not.push(json!({
            "terms": {
                ProductDocumentSerdeField::ShopSlugId.as_str(): search.exclude_shop_slug_id_query.iter().map(ToString::to_string).collect::<Vec<_>>()
            }
        }));
    }
    if !search.exclude_seller_slug_id_query.is_empty() {
        must_not.push(json!({
            "terms": {
                ProductDocumentSerdeField::SellerSlugId.as_str(): search.exclude_seller_slug_id_query.iter().map(ToString::to_string).collect::<Vec<_>>()
            }
        }));
    }

    let price_field = price_field_for(&search.currency);
    if let Some(min) = search.price_query.and_then(|query| query.min) {
        filter.push(json!({ "range": { price_field: { "gte": min.deref() } } }));
    }
    if let Some(max) = search.price_query.and_then(|query| query.max) {
        filter.push(json!({ "range": { price_field: { "lte": max.deref() } } }));
    }

    if !search.country_query.is_empty() {
        filter.push(json!({
            "terms": {
                ProductDocumentSerdeField::StructuredAddressCountry.as_str(): search.country_query.iter().map(|country| country.alpha2()).collect::<Vec<_>>()
            }
        }));
    }
    if !search.continent_query.is_empty() {
        filter.push(json!({
            "terms": {
                ProductDocumentSerdeField::StructuredAddressContinent.as_str(): search.continent_query.iter().map(|continent| ContinentDocument::from(*continent).as_str()).collect::<Vec<_>>()
            }
        }));
    }
    if let Some(query) = search.geo_address_distance_query {
        filter.push(json!({
            "geo_distance": {
                "distance": query.distance.opensearch_value(),
                ProductDocumentSerdeField::GeoAddress.as_str(): {
                    "lat": query.lat,
                    "lon": query.lon
                }
            }
        }));
    }

    apply_any_of_filter(
        &mut filter,
        &search.shop_name_query,
        ProductDocumentSerdeField::ShopName,
        ShopName::as_ref,
    );
    apply_any_of_filter(
        &mut filter,
        &search.seller_name_query,
        ProductDocumentSerdeField::SellerName,
        ShopName::as_ref,
    );
    if !search.shop_slug_id_query.is_empty() {
        filter.push(json!({
            "terms": {
                ProductDocumentSerdeField::ShopSlugId.as_str(): search.shop_slug_id_query.iter().map(ToString::to_string).collect::<Vec<_>>()
            }
        }));
    }
    if !search.seller_slug_id_query.is_empty() {
        filter.push(json!({
            "terms": {
                ProductDocumentSerdeField::SellerSlugId.as_str(): search.seller_slug_id_query.iter().map(ToString::to_string).collect::<Vec<_>>()
            }
        }));
    }

    let state_query = search
        .state_query
        .iter()
        .map(|value| ProductStateDocument::from(*value))
        .collect();
    apply_any_of_filter(
        &mut filter,
        &state_query,
        ProductDocumentSerdeField::State,
        ProductStateDocument::as_str,
    );
    let shop_type_query = search
        .shop_type_query
        .iter()
        .map(|value| ShopTypeDocument::from(*value))
        .collect();
    apply_any_of_filter(
        &mut filter,
        &shop_type_query,
        ProductDocumentSerdeField::ShopType,
        ShopTypeDocument::as_str,
    );

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

fn apply_any_of_filter<T: Hash + Eq + EnumCount>(
    filter: &mut Vec<serde_json::Value>,
    query: &AnyOfQuery<T>,
    field: ProductDocumentSerdeField,
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
    use crate::product_image_document::ProductImageDocument;
    use crate::prohibited_content_document::ProhibitedContentDocument;
    use common::event_id::EventId;
    use common::language::document::{LanguageDocument, TextDocument};
    use common::opensearch::search_response::{HitsMetadata, ShardStats, TotalHits};
    use common::product_id::ProductId;
    use common::product_lifecycle::domain::ProductLifecycle;
    use common::product_slug_id::ProductSlugId;
    use common::product_state::domain::ProductState;
    use common::query::range_query::RangeQuery;
    use common::seller_slug_id::SellerSlugId;
    use common::shop_id::ShopId;
    use common::shop_slug_id::ShopSlugId;
    use common::shops_product_id::ShopsProductId;
    use geo::core::continent::Continent;
    use indexmap::IndexSet;
    use isocountry::CountryCode;

    use serde_json::Value;
    use shop_core::shop_type::ShopType;
    use std::collections::HashSet;
    use time::macros::datetime;
    use url::Url;

    fn text_query(value: &str) -> Result<TextQuery<1>, Box<dyn std::error::Error>> {
        Ok(value.try_into()?)
    }

    fn search_with_product_query(
        product_query: &str,
    ) -> Result<ProductSearch, Box<dyn std::error::Error>> {
        Ok(ProductSearch::new(Language::En, Currency::Eur)
            .with_product_query(text_query(product_query)?))
    }

    fn product_document() -> Result<ProductDocument, url::ParseError> {
        Ok(ProductDocument {
            product_id: ProductId::new(),
            product_slug_id: ProductSlugId::from("vase-abcdef"),
            shop_slug_id: ShopSlugId::from("shop"),
            seller_slug_id: SellerSlugId::from("seller"),
            event_id: EventId::new(),
            shop_id: ShopId::new(),
            seller_id: ShopId::new(),
            shops_product_id: ShopsProductId::from("sku-1"),
            shop_name: "Shop".to_owned(),
            seller_name: "Seller".to_owned(),
            shop_type: ShopTypeDocument::CommercialDealer,
            structured_address_addressline: None,
            structured_address_addressline_extra: None,
            structured_address_locality: None,
            structured_address_region: None,
            structured_address_postal_code: None,
            structured_address_country: None,
            structured_address_continent: None,
            geo_address: None,
            title: TextDocument::new("Vase", LanguageDocument::En),
            title_de: Some("Deutsche Vase".to_owned()),
            title_en: Some("English vase".to_owned()),
            title_fr: None,
            title_es: None,
            title_it: None,
            price_eur: Some(100),
            price_usd: Some(125),
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
            state: ProductStateDocument::Available,
            lifecycle: ProductLifecycleDocument::Active,
            url: Url::parse("https://shop.example/products/sku-1")?,
            view_url: Url::parse("https://aura.example/products/vase-abcdef")?,
            images: [ProductImageDocument {
                url: Url::parse("https://example.com/image.jpg")?,
                prohibited_content: ProhibitedContentDocument::None,
            }]
            .into_iter()
            .collect(),
            embedding: None,
            auction_start: None,
            auction_end: None,
            created: datetime!(2025-01-01 0:00 UTC),
            updated: datetime!(2025-01-02 0:00 UTC),
        })
    }

    fn search_response(
        document: ProductDocument,
        sort: Option<Value>,
    ) -> SearchResponse<ProductDocument> {
        SearchResponse {
            took: 1,
            timed_out: false,
            shards: ShardStats {
                total: 1,
                successful: 1,
                skipped: 0,
                failed: 0,
            },
            hits: HitsMetadata {
                total: TotalHits {
                    value: 1,
                    relation: "eq".to_owned(),
                },
                max_score: None,
                hits: vec![SearchHit {
                    index: "products".to_owned(),
                    id: document.product_id.to_string(),
                    score: None,
                    sort,
                    matched_queries: Vec::new(),
                    source: document,
                }],
            },
        }
    }

    #[test]
    fn should_build_search_request_with_default_filters_sort_and_source_excludes()
    -> Result<(), serde_json::Error> {
        let search = ProductSearch::new(Language::En, Currency::Eur);
        let sort = Sort {
            sort: SortProductField::Score,
            order: SortOrder::Desc,
        };

        let actual = build_search_request(&search, &sort, &None)?;

        assert_eq!(
            actual.pointer("/_source/excludes/0"),
            Some(&json!("embedding"))
        );
        assert_eq!(
            actual.pointer("/query/bool/filter/0/terms/lifecycle"),
            Some(&json!(["ACTIVE"]))
        );
        assert_eq!(actual.pointer("/sort/0/_score/order"), Some(&json!("desc")));
        assert_eq!(
            actual.pointer("/sort/1/productId/order"),
            Some(&json!("asc"))
        );
        assert!(actual.get("size").is_none());
        Ok(())
    }

    #[test]
    fn should_build_hybrid_search_request_with_text_filters_and_cursor()
    -> Result<(), Box<dyn std::error::Error>> {
        let search = ProductSearch::new(Language::En, Currency::Usd)
            .with_product_query("vintage brass lamp".try_into()?)
            .with_state_query([ProductState::Available].into_iter().collect());
        let cursor = Some(Cursor {
            size: 10,
            search_after: Some(json!([0.123])),
        });

        let actual = build_hybrid_search_request(&search, &[1.0; 3], &cursor)?;

        assert_eq!(actual.pointer("/size"), Some(&json!(10)));
        assert_eq!(
            actual.pointer(
                "/query/hybrid/queries/0/bool/must/0/bool/must/0/bool/should/0/multi_match/query"
            ),
            Some(&json!("vintage brass lamp"))
        );
        assert_eq!(
            actual.pointer("/query/hybrid/queries/0/bool/filter/1/terms/state"),
            Some(&json!(["AVAILABLE"]))
        );
        assert_eq!(
            actual.pointer("/query/hybrid/queries/1/knn/embedding/vector"),
            Some(&json!([1.0, 1.0, 1.0]))
        );
        assert_eq!(
            actual
                .pointer("/query/hybrid/queries/1/knn/embedding/filter/bool/filter/1/terms/state"),
            Some(&json!(["AVAILABLE"]))
        );
        assert_eq!(actual.pointer("/search_after"), Some(&json!([0.123])));
        assert_eq!(
            actual.pointer("/sort/0/_script/order"),
            Some(&json!("desc"))
        );
        Ok(())
    }

    #[test]
    fn should_build_search_request_with_price_sort_cursor_and_search_after()
    -> Result<(), serde_json::Error> {
        let search = ProductSearch::new(Language::En, Currency::Usd);
        let sort = Sort {
            sort: SortProductField::Price,
            order: SortOrder::Asc,
        };
        let cursor = Some(Cursor {
            size: 10,
            search_after: Some(json!(123)),
        });

        let actual = build_search_request(&search, &sort, &cursor)?;

        assert_eq!(actual.pointer("/size"), Some(&json!(10)));
        assert_eq!(actual.pointer("/search_after"), Some(&json!([123])));
        assert_eq!(
            actual.pointer("/sort/0/priceUsd/order"),
            Some(&json!("asc"))
        );
        assert_eq!(
            actual.pointer("/sort/0/priceUsd/missing"),
            Some(&json!("_last"))
        );
        Ok(())
    }

    #[test]
    fn should_keep_array_search_after_when_cursor_value_is_array() -> Result<(), serde_json::Error>
    {
        let search = ProductSearch::new(Language::En, Currency::Usd);
        let sort = Sort {
            sort: SortProductField::Score,
            order: SortOrder::Desc,
        };
        let cursor = Some(Cursor {
            size: 10,
            search_after: Some(json!([123, "abc"])),
        });

        let actual = build_search_request(&search, &sort, &cursor)?;

        assert_eq!(actual.pointer("/search_after"), Some(&json!([123, "abc"])));
        Ok(())
    }

    #[test]
    fn should_build_live_search_query_with_or_over_multiple_product_queries()
    -> Result<(), Box<dyn std::error::Error>> {
        let search = ["Madonna oil painting", "Virgin Mary oil painting"]
            .into_iter()
            .try_fold(
                ProductSearch::new(Language::En, Currency::Eur),
                |search, query| {
                    Ok::<_, Box<dyn std::error::Error>>(
                        search.with_product_query(text_query(query)?),
                    )
                },
            )?;

        let actual = build_search_query(&search)?;

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
        Ok(())
    }

    #[test]
    fn should_build_query_for_exclusions_filters_ranges_and_dates()
    -> Result<(), Box<dyn std::error::Error>> {
        let excluded_product_id = ProductId::new();
        let excluded_shop_slug_id = ShopSlugId::from("bad-shop");
        let excluded_seller_slug_id = SellerSlugId::from("bad-seller");
        let shop_slug_id = ShopSlugId::from("shop");
        let seller_slug_id = SellerSlugId::from("seller");
        let search = ProductSearch::new(Language::De, Currency::Eur)
            .with_exclude_product_id_query(HashSet::from([excluded_product_id]).into())
            .with_exclude_shop_name_query(HashSet::from([ShopName::from("Bad Shop")]).into())
            .with_exclude_seller_name_query(HashSet::from([ShopName::from("Bad Seller")]).into())
            .with_exclude_shop_slug_id_query(HashSet::from([excluded_shop_slug_id.clone()]).into())
            .with_exclude_seller_slug_id_query(
                HashSet::from([excluded_seller_slug_id.clone()]).into(),
            )
            .with_price_query(RangeQuery {
                min: Some(100_u64.into()),
                max: Some(999_u64.into()),
            })
            .with_country_query(HashSet::from([CountryCode::DEU]).into())
            .with_continent_query(HashSet::from([Continent::Europe]).into())
            .with_shop_name_query(HashSet::from([ShopName::from("Shop")]).into())
            .with_seller_name_query(HashSet::from([ShopName::from("Seller")]).into())
            .with_shop_slug_id_query(HashSet::from([shop_slug_id.clone()]).into())
            .with_seller_slug_id_query(HashSet::from([seller_slug_id.clone()]).into())
            .with_state_query(HashSet::from([ProductState::Available]).into())
            .with_shop_type_query(HashSet::from([ShopType::CommercialDealer]).into())
            .with_lifecycle_query(HashSet::from([ProductLifecycle::Deleted]).into())
            .with_created_query(RangeQuery {
                min: Some(datetime!(2025-01-01 0:00 UTC)),
                max: None,
            })
            .with_updated_query(RangeQuery {
                min: None,
                max: Some(datetime!(2025-01-02 0:00 UTC)),
            });

        let actual = build_search_query(&search)?;

        assert_eq!(
            actual.pointer("/bool/must_not/0/terms/productId"),
            Some(&json!([excluded_product_id.to_string()]))
        );
        assert_eq!(
            actual.pointer("/bool/must_not/3/terms/shopSlugId"),
            Some(&json!([excluded_shop_slug_id.to_string()]))
        );
        assert_eq!(
            actual.pointer("/bool/must_not/4/terms/sellerSlugId"),
            Some(&json!([excluded_seller_slug_id.to_string()]))
        );
        assert_eq!(
            actual.pointer("/bool/filter/0/terms/lifecycle"),
            Some(&json!(["DELETED"]))
        );
        assert_eq!(
            actual.pointer("/bool/filter/1/range/priceEur/gte"),
            Some(&json!(100))
        );
        assert_eq!(
            actual.pointer("/bool/filter/2/range/priceEur/lte"),
            Some(&json!(999))
        );
        assert_eq!(
            actual.pointer("/bool/filter/3/terms/structuredAddressCountry"),
            Some(&json!(["DE"]))
        );
        assert_eq!(
            actual.pointer("/bool/filter/4/terms/structuredAddressContinent"),
            Some(&json!(["EUROPE"]))
        );
        assert_eq!(
            actual.pointer("/bool/filter/7/terms/shopSlugId"),
            Some(&json!([shop_slug_id.to_string()]))
        );
        assert_eq!(
            actual.pointer("/bool/filter/8/terms/sellerSlugId"),
            Some(&json!([seller_slug_id.to_string()]))
        );
        assert_eq!(
            actual.pointer("/bool/filter/9/terms/state"),
            Some(&json!(["AVAILABLE"]))
        );
        assert_eq!(
            actual.pointer("/bool/filter/10/terms/shopType"),
            Some(&json!(["COMMERCIAL_DEALER"]))
        );
        assert_eq!(
            actual.pointer("/bool/filter/11/range/created/gte"),
            Some(&json!("2025-01-01T00:00:00Z"))
        );
        assert_eq!(
            actual.pointer("/bool/filter/12/range/updated/lte"),
            Some(&json!("2025-01-02T00:00:00Z"))
        );
        Ok(())
    }

    #[test]
    fn should_preserve_open_search_hit_order_when_mapping_product_summaries()
    -> Result<(), Box<dyn std::error::Error>> {
        let first = product_document()?;
        let second = ProductDocument {
            product_id: ProductId::new(),
            ..product_document()?
        };
        let first_product_id = first.product_id;
        let second_product_id = second.product_id;
        let response = SearchResponse {
            took: 1,
            timed_out: false,
            shards: ShardStats {
                total: 1,
                successful: 1,
                skipped: 0,
                failed: 0,
            },
            hits: HitsMetadata {
                total: TotalHits {
                    value: 2,
                    relation: "eq".to_owned(),
                },
                max_score: None,
                hits: vec![
                    SearchHit {
                        index: "products".to_owned(),
                        id: first_product_id.to_string(),
                        score: Some(0.9),
                        sort: None,
                        matched_queries: Vec::new(),
                        source: first,
                    },
                    SearchHit {
                        index: "products".to_owned(),
                        id: second_product_id.to_string(),
                        score: Some(0.8),
                        sort: None,
                        matched_queries: Vec::new(),
                        source: second,
                    },
                ],
            },
        };

        let actual =
            map_search_response(&ProductSearch::new(Language::En, Currency::Eur), response);

        assert_eq!(
            vec![first_product_id, second_product_id],
            actual
                .items
                .into_iter()
                .map(|item| item.product_id)
                .collect::<Vec<_>>()
        );
        Ok(())
    }

    #[test]
    fn should_map_search_response_to_product_summaries() -> Result<(), Box<dyn std::error::Error>> {
        let document = ProductDocument {
            images: IndexSet::from([ProductImageDocument {
                url: Url::parse("https://example.com/other.jpg")?,
                prohibited_content: ProhibitedContentDocument::None,
            }]),
            ..product_document()?
        };
        let expected_product_id = document.product_id;
        let expected_event_id = document.event_id;
        let expected_shop_id = document.shop_id;
        let expected_seller_id = document.seller_id;
        let expected_shops_product_id = document.shops_product_id.clone();
        let response = search_response(
            document,
            Some(json!([125, expected_product_id.to_string()])),
        );
        let search = ProductSearch::new(Language::De, Currency::Usd);

        let actual = map_search_response(&search, response);

        assert_eq!(Some(1), actual.total);
        assert_eq!(1, actual.cursor.size);
        assert_eq!(
            Some(json!([125, expected_product_id.to_string()])),
            actual.cursor.search_after
        );
        assert_eq!(expected_product_id, actual.items[0].product_id);
        assert_eq!(expected_event_id, actual.items[0].event_id);
        assert_eq!(expected_shop_id, actual.items[0].shop_id);
        assert_eq!(expected_seller_id, actual.items[0].seller_id);
        assert_eq!(expected_shops_product_id, actual.items[0].shops_product_id);
        assert_eq!(ShopName::from("Shop"), actual.items[0].shop_name);
        assert_eq!(
            Some(Language::De),
            actual.items[0]
                .title
                .as_ref()
                .map(|title| title.localization)
        );
        assert_eq!(
            Some("Deutsche Vase"),
            actual.items[0]
                .title
                .as_ref()
                .map(|title| title.payload.as_ref())
        );
        assert_eq!(
            Some(Price::new(MonetaryAmount::from(125_u64), Currency::Usd)),
            actual.items[0].price
        );
        assert_eq!(ProductState::Available, actual.items[0].state);
        assert_eq!(ProductLifecycle::Active, actual.items[0].lifecycle);
        assert_eq!(1, actual.items[0].images.len());
        Ok(())
    }

    #[test]
    fn should_fallback_title_and_price_when_preferred_missing() -> Result<(), url::ParseError> {
        let document = ProductDocument {
            title_de: None,
            title_en: Some("English vase".to_owned()),
            price_usd: None,
            price_eur: Some(100),
            ..product_document()?
        };

        let title = resolve_title(&document, Language::De);
        let price = resolve_price(&document, Currency::Usd);

        assert_eq!(
            Some(Language::En),
            title.as_ref().map(|title| title.localization)
        );
        assert_eq!(
            Some("English vase"),
            title.as_ref().map(|title| title.payload.as_ref())
        );
        assert_eq!(
            Some(Price::new(MonetaryAmount::from(100_u64), Currency::Eur)),
            price
        );
        Ok(())
    }

    #[test]
    fn should_build_query_with_geo_distance_filter() -> Result<(), serde_json::Error> {
        let search = ProductSearch::new(Language::En, Currency::Eur)
            .with_geo_address_distance_query(common::distance::domain::GeoDistanceQuery {
                lat: 52.52,
                lon: 13.405,
                distance: common::distance::domain::Distance {
                    amount: 10.0,
                    unit: common::distance::domain::DistanceUnit::Kilometers,
                },
            });

        let actual = build_search_query(&search)?;

        assert_eq!(
            actual.pointer("/bool/filter/1/geo_distance/distance"),
            Some(&json!("10km"))
        );
        assert_eq!(
            actual.pointer("/bool/filter/1/geo_distance/geoAddress/lat"),
            Some(&json!(52.52))
        );
        assert_eq!(
            actual.pointer("/bool/filter/1/geo_distance/geoAddress/lon"),
            Some(&json!(13.405))
        );
        Ok(())
    }

    #[test]
    fn should_use_english_title_field_for_ingestion_only_languages()
    -> Result<(), Box<dyn std::error::Error>> {
        let search = ProductSearch::new(Language::Zh, Currency::Eur)
            .with_product_query(text_query("porcelain")?);

        let actual = build_search_query(&search)?;

        assert_eq!(
            actual.pointer("/bool/must/0/bool/must/0/bool/should/0/multi_match/fields/0"),
            Some(&json!("titleEn^5"))
        );
        Ok(())
    }

    #[test]
    fn should_build_query_with_single_product_query() -> Result<(), Box<dyn std::error::Error>> {
        let search = search_with_product_query("Ming dynasty blue white porcelain vase")?;

        let actual = build_search_query(&search)?;

        assert_eq!(
            actual.pointer("/bool/must/0/bool/must/0/bool/should/0/multi_match/query"),
            Some(&json!("Ming dynasty blue white porcelain vase"))
        );
        assert_eq!(
            actual.pointer("/bool/must/0/bool/should/2/match/titleEn/minimum_should_match"),
            Some(&json!("2<75%"))
        );
        Ok(())
    }
}
