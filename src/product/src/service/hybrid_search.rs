//! Adaptive hybrid (BM25 + kNN, RRF-fused) retrieval orchestrated against an
//! OpenSearch cluster. The actual hybrid query and rank fusion run server-side via
//! OpenSearch's native `hybrid` query and the pre-registered `score-ranker-processor`
//! pipeline (RRF with `rank_constant = 60`). This module only computes the soft intent
//! signals + adaptive `HybridSearchParams` and dispatches the request through the
//! repository.

use crate::core::product::{LocalizedProductView, Product};
use crate::core::product_search::ProductSearch;
use crate::opensearch::intent::{
    HybridSearchParams, IntentSignals, compute_intent_signals, intent_centroids,
};
use crate::opensearch::repository::ProductOpenSearchRepository;
use common::language::domain::Language;
use common::pagination::cursor::{Cursor, CursoredResult};
use tracing::warn;

#[derive(thiserror::Error, Debug)]
pub enum HybridSearchError {
    #[error("OpenSearchError: {0}")]
    OpenSearchError(#[from] opensearch::Error),
}

/// Outcome of a single hybrid search request: the cursored result plus the derived
/// intent signals & params so callers can introspect / log them.
pub struct HybridSearchOutcome {
    pub items: CursoredResult<LocalizedProductView, serde_json::Value>,
    pub intent: IntentSignals,
    pub params: HybridSearchParams,
}

/// Run a hybrid (BM25 + kNN) search using OpenSearch's native `hybrid` query with the
/// pre-registered RRF search pipeline. Caller supplies the pre-computed query `embedding`
/// (typically via `MultimodalEmbeddingService::embed_query`).
///
/// Intent signals are derived from the query text and embedding alone (no extra probe
/// query). Pagination uses standard OpenSearch `search_after` over `[_score desc]`. Ties
/// on score are non-deterministic; callers that need fully stable pagination should request
/// a fresh first page. The candidate window is held stable per request via
/// `params.candidate_k`.
pub async fn hybrid_search(
    repository: &(dyn ProductOpenSearchRepository + Sync),
    search: &ProductSearch,
    embedding: &[f32],
    page: &Option<Cursor<serde_json::Value>>,
    languages: &[Language],
) -> Result<HybridSearchOutcome, HybridSearchError> {
    let query_text = search
        .product_query
        .as_ref()
        .map(|q| q.as_ref().to_string())
        .unwrap_or_default();

    // Compute soft intent signals from query text + embedding only (no BM25 probe).
    let intent = compute_intent_signals(&query_text, Some(embedding), &[], intent_centroids());
    let params = HybridSearchParams::from(&intent);

    // Issue the native OpenSearch hybrid query with RRF fusion server-side.
    let response = repository
        .hybrid_search_product_documents(search, embedding, params, page)
        .await?;

    if response.timed_out {
        warn!(
            searchFilter = ?search,
            page = ?page,
            took = response.took,
            "Hybrid search OpenSearch request timed out."
        );
    }

    let cursor = Cursor {
        size: response.hits.hits.len() as u64,
        search_after: response.hits.hits.last().and_then(|last| last.sort.clone()),
    };
    let total = response.hits.total.value;
    let currency = search.currency;
    let product_views: Vec<LocalizedProductView> = response
        .hits
        .hits
        .into_iter()
        .map(|hit| Product::from(hit.source).localized(&currency, languages))
        .collect();

    Ok(HybridSearchOutcome {
        items: CursoredResult {
            items: product_views,
            cursor,
            total: Some(total),
        },
        intent,
        params,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::product_search::ProductSearch;
    use crate::opensearch::product_document::ProductDocument;
    use crate::opensearch::repository::MockProductOpenSearchRepository;
    use common::opensearch::search_response::{
        HitsMetadata, SearchHit, SearchResponse, ShardStats, TotalHits,
    };
    use fake::{Fake, Faker};

    fn mk_search() -> ProductSearch {
        let mut s: ProductSearch = Faker.fake();
        s.product_query = Some("art deco lamp".try_into().unwrap());
        s
    }

    fn mk_response(docs: Vec<ProductDocument>) -> SearchResponse<ProductDocument> {
        let total = docs.len() as u64;
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
                    value: total,
                    relation: "eq".to_string(),
                },
                max_score: Some(1.0),
                hits: docs
                    .into_iter()
                    .enumerate()
                    .map(|(i, d)| {
                        let id = d.product_id.to_string();
                        SearchHit {
                            index: "products".to_string(),
                            id: id.clone(),
                            score: Some(1.0 / (i as f64 + 1.0)),
                            sort: Some(serde_json::json!([1.0 / (i as f64 + 1.0)])),
                            source: d,
                        }
                    })
                    .collect(),
            },
        }
    }

    #[tokio::test]
    async fn should_dispatch_native_hybrid_query_for_text_search() {
        let mut repo = MockProductOpenSearchRepository::default();
        let docs: Vec<ProductDocument> = (0..3).map(|_| Faker.fake::<ProductDocument>()).collect();
        let hybrid_response = mk_response(docs);

        repo.expect_hybrid_search_product_documents()
            .return_once(move |_, _, _, _| Box::pin(async move { Ok(hybrid_response) }));

        let search = mk_search();
        let embedding = vec![0.1f32; 768];
        let outcome = hybrid_search(&repo, &search, &embedding, &None, &[search.language])
            .await
            .unwrap();
        assert_eq!(outcome.items.items.len(), 3);
        assert!(outcome.params.vector_weight <= 1.0 - HybridSearchParams::MIN_BM25_WEIGHT);
        assert!(outcome.params.candidate_k >= HybridSearchParams::MIN_CANDIDATE_K);
        assert!(outcome.params.candidate_k <= HybridSearchParams::MAX_CANDIDATE_K);
    }

    #[tokio::test]
    async fn should_propagate_pagination_cursor_from_opensearch_response() {
        let mut repo = MockProductOpenSearchRepository::default();
        let docs: Vec<ProductDocument> = (0..2).map(|_| Faker.fake::<ProductDocument>()).collect();
        let hybrid = mk_response(docs);
        repo.expect_hybrid_search_product_documents()
            .return_once(move |_, _, _, _| Box::pin(async move { Ok(hybrid) }));

        let search = mk_search();
        let outcome = hybrid_search(
            &repo,
            &search,
            &vec![0.0f32; 768],
            &None,
            &[search.language],
        )
        .await
        .unwrap();
        assert!(outcome.items.cursor.search_after.is_some());
    }
}
