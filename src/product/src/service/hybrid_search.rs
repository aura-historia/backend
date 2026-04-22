use crate::core::product::{LocalizedProductView, Product};
use crate::core::product_search::ProductSearch;
use crate::core::sort_product_field::SortProductField;
use crate::opensearch::product_document::ProductDocument;
use crate::opensearch::repository::ProductOpenSearchRepository;
use crate::service::intent::{
    HybridSearchParams, IntentSignals, compute_intent_signals, intent_centroids,
};
use crate::service::query_embedding_service::{QueryEmbeddingError, QueryEmbeddingService};
use common::language::domain::Language;
use common::opensearch::search_response::SearchResponse;
use common::pagination::cursor::{Cursor, CursoredResult};
use common::sort::{Sort, SortOrder};
use std::collections::HashMap;
use tracing::warn;

/// Reciprocal Rank Fusion constant. The standard literature value is 60; it gives a smooth
/// decay so that hits ranked far down the list still contribute meaningfully.
pub const RRF_K: f32 = 60.0;

/// Rank-fuse two ranked lists of `ProductDocument`s using weighted Reciprocal Rank Fusion.
///
/// Each list is ordered descending by relevance (rank 0 = best). The fused score for a doc
/// is `bm25_weight / (RRF_K + rank_bm25) + vector_weight / (RRF_K + rank_knn)` (missing
/// rank in either list contributes 0). Returns docs sorted by fused score desc, productId asc.
pub fn rrf_fuse(
    bm25_hits: Vec<ProductDocument>,
    knn_hits: Vec<ProductDocument>,
    vector_weight: f32,
) -> Vec<(f32, ProductDocument)> {
    let bm25_weight = (1.0 - vector_weight).max(0.0);

    let mut scored: HashMap<common::product_id::ProductId, (f32, ProductDocument)> =
        HashMap::with_capacity(bm25_hits.len() + knn_hits.len());

    for (rank, doc) in bm25_hits.into_iter().enumerate() {
        let id = doc.product_id;
        let contribution = bm25_weight / (RRF_K + rank as f32);
        scored
            .entry(id)
            .and_modify(|e| e.0 += contribution)
            .or_insert((contribution, doc));
    }

    for (rank, doc) in knn_hits.into_iter().enumerate() {
        let id = doc.product_id;
        let contribution = vector_weight / (RRF_K + rank as f32);
        scored
            .entry(id)
            .and_modify(|e| e.0 += contribution)
            .or_insert_with(|| (contribution, doc));
    }

    let mut fused: Vec<(f32, ProductDocument)> = scored.into_values().collect();
    fused.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.product_id.cmp(&b.1.product_id))
    });
    fused
}

/// Outcome of a single hybrid search request.
pub struct HybridSearchOutcome {
    pub items: CursoredResult<LocalizedProductView, serde_json::Value>,
    pub intent: IntentSignals,
    pub params: HybridSearchParams,
}

#[derive(thiserror::Error, Debug)]
pub enum HybridSearchError {
    #[error("OpenSearchError: {0}")]
    OpenSearchError(#[from] opensearch::Error),
    #[error("QueryEmbeddingError: {0}")]
    QueryEmbeddingError(#[from] QueryEmbeddingError),
}

/// Run a hybrid (BM25 + kNN, RRF-fused) search for `search.product_query`.
///
/// `embedding_service` is used to embed the query (cached upstream). `embedding` may be
/// passed directly to skip the embedding step (used in tests / pagination warm path).
///
/// Pagination uses the cursor's `search_after = [fused_score, productId]` which is
/// re-applied to the freshly fused candidate list. The candidate_k is held constant to
/// guarantee stable pagination as required by the issue's guidelines.
pub async fn hybrid_search(
    repository: &(dyn ProductOpenSearchRepository + Sync),
    embedding_service: &(dyn QueryEmbeddingService + Sync),
    search: &ProductSearch,
    sort: &Option<Sort<SortProductField>>,
    page: &Option<Cursor<serde_json::Value>>,
    languages: &[Language],
) -> Result<HybridSearchOutcome, HybridSearchError> {
    let query_text = search
        .product_query
        .as_ref()
        .map(|q| q.as_ref().to_string())
        .unwrap_or_default();

    // 1. Embed the query (small, cached call).
    let embedding = embedding_service.embed_query(&query_text).await?;

    // 2. Fetch a small BM25 probe to feed the intent signal computation. We use the existing
    //    repository search with the user-supplied filters and a tiny page so the round-trip
    //    is cheap.
    let probe_sort = Sort {
        sort: SortProductField::Score,
        order: SortOrder::Desc,
    };
    let probe_cursor = Cursor::<serde_json::Value> {
        size: 20,
        search_after: None,
    };
    let bm25_probe = repository
        .search_product_documents(search, &probe_sort, &Some(probe_cursor))
        .await?;
    let bm25_scores: Vec<f32> = bm25_probe
        .hits
        .hits
        .iter()
        .filter_map(|h| h.score.map(|s| s as f32))
        .collect();

    // 3. Compute soft intent signals + adaptive params.
    let intent = compute_intent_signals(
        &query_text,
        Some(&embedding),
        &bm25_scores,
        intent_centroids(),
    );
    let params = HybridSearchParams::from_intent(&intent);

    // 4. Run BM25 (full candidate_k) and kNN in parallel.
    let bm25_cursor = Some(Cursor::<serde_json::Value> {
        size: params.candidate_k as u64,
        search_after: None,
    });
    let bm25_fut = repository.search_product_documents(search, &probe_sort, &bm25_cursor);
    let knn_fut = repository.knn_search_product_documents(search, &embedding, params.candidate_k);
    let (bm25_resp, knn_resp) = tokio::join!(bm25_fut, knn_fut);
    let bm25_resp: SearchResponse<ProductDocument> = bm25_resp?;
    let knn_resp: SearchResponse<ProductDocument> = knn_resp?;

    if bm25_resp.timed_out || knn_resp.timed_out {
        warn!(
            searchFilter = ?search,
            sort = ?sort,
            page = ?page,
            "Hybrid search: at least one of BM25/kNN OpenSearch requests timed out."
        );
    }

    // 5. Fuse via Reciprocal Rank Fusion.
    let bm25_docs: Vec<ProductDocument> =
        bm25_resp.hits.hits.into_iter().map(|h| h.source).collect();
    let knn_docs: Vec<ProductDocument> = knn_resp.hits.hits.into_iter().map(|h| h.source).collect();
    let total_unique = {
        // For total: count of distinct product ids among the candidate set.
        let mut ids: std::collections::HashSet<common::product_id::ProductId> =
            std::collections::HashSet::with_capacity(bm25_docs.len() + knn_docs.len());
        for d in &bm25_docs {
            ids.insert(d.product_id);
        }
        for d in &knn_docs {
            ids.insert(d.product_id);
        }
        ids.len() as u64
    };
    let mut fused = rrf_fuse(bm25_docs, knn_docs, params.vector_weight);

    // 6. Apply pagination via search_after = [fused_score, productId].
    if let Some(cursor) = page
        && let Some(sa) = &cursor.search_after
        && let Some((cursor_score, cursor_id)) = decode_cursor(sa)
    {
        fused.retain(|(score, doc)| {
            let s_cmp = score
                .partial_cmp(&cursor_score)
                .unwrap_or(std::cmp::Ordering::Equal);
            match s_cmp {
                std::cmp::Ordering::Less => true,
                std::cmp::Ordering::Greater => false,
                std::cmp::Ordering::Equal => doc.product_id > cursor_id,
            }
        });
    }

    let page_size = page.as_ref().map(|c| c.size).unwrap_or(20).max(1) as usize;
    let page_items: Vec<(f32, ProductDocument)> = fused.into_iter().take(page_size).collect();

    let next_search_after = page_items
        .last()
        .map(|(score, doc)| serde_json::json!([score, doc.product_id.to_string()]));

    let cursor = Cursor {
        size: page_items.len() as u64,
        search_after: next_search_after,
    };

    let currency = search.currency;
    let product_views: Vec<LocalizedProductView> = page_items
        .into_iter()
        .map(|(_score, doc)| Product::from(doc).localized(&currency, languages))
        .collect();

    Ok(HybridSearchOutcome {
        items: CursoredResult {
            items: product_views,
            cursor,
            total: Some(total_unique),
        },
        intent,
        params,
    })
}

fn decode_cursor(value: &serde_json::Value) -> Option<(f32, common::product_id::ProductId)> {
    let arr = value.as_array()?;
    let score = arr.first()?.as_f64()? as f32;
    let id_str = arr.get(1)?.as_str()?;
    let id = common::product_id::ProductId::try_from(id_str).ok()?;
    Some((score, id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opensearch::product_document::ProductDocument;
    use common::product_id::ProductId;
    use fake::{Fake, Faker};

    fn doc_with_id(id: ProductId) -> ProductDocument {
        let mut doc: ProductDocument = Faker.fake();
        doc.product_id = id;
        doc
    }

    #[test]
    fn should_rank_intersecting_documents_higher_when_present_in_both_lists() {
        let shared_id = ProductId::new();
        let bm25_only = ProductId::new();
        let knn_only = ProductId::new();

        let bm25 = vec![doc_with_id(bm25_only), doc_with_id(shared_id)];
        let knn = vec![doc_with_id(shared_id), doc_with_id(knn_only)];

        let fused = rrf_fuse(bm25, knn, 0.5);
        assert_eq!(fused[0].1.product_id, shared_id);
    }

    #[test]
    fn should_respect_vector_weight_zero_for_pure_bm25_ranking() {
        let id_a = ProductId::new();
        let id_b = ProductId::new();
        // BM25 ranks A first, kNN ranks B first.
        let bm25 = vec![doc_with_id(id_a), doc_with_id(id_b)];
        let knn = vec![doc_with_id(id_b), doc_with_id(id_a)];
        let fused = rrf_fuse(bm25, knn, 0.0);
        assert_eq!(fused[0].1.product_id, id_a);
    }

    #[test]
    fn should_respect_vector_weight_high_for_knn_dominance() {
        let id_a = ProductId::new();
        let id_b = ProductId::new();
        let bm25 = vec![doc_with_id(id_a), doc_with_id(id_b)];
        let knn = vec![doc_with_id(id_b), doc_with_id(id_a)];
        // vector_weight=0.8 ⟹ kNN dominates ⟹ B should rank first.
        let fused = rrf_fuse(bm25, knn, 0.8);
        assert_eq!(fused[0].1.product_id, id_b);
    }

    #[test]
    fn should_break_ties_by_product_id_ascending() {
        // Two singleton lists with same vector_weight=0.5 — both contribute identical RRF scores.
        let mut id_a = ProductId::new();
        let mut id_b = ProductId::new();
        if id_a > id_b {
            std::mem::swap(&mut id_a, &mut id_b);
        }
        let bm25 = vec![doc_with_id(id_a)];
        let knn = vec![doc_with_id(id_b)];
        let fused = rrf_fuse(bm25, knn, 0.5);
        assert_eq!(fused[0].1.product_id, id_a);
        assert_eq!(fused[1].1.product_id, id_b);
    }

    #[test]
    fn should_decode_pagination_cursor() {
        let id = ProductId::new();
        let v = serde_json::json!([0.0123_f64, id.to_string()]);
        let (score, decoded) = decode_cursor(&v).unwrap();
        assert!((score - 0.0123).abs() < 1e-6);
        assert_eq!(decoded, id);
    }

    #[test]
    fn should_return_none_for_malformed_pagination_cursor() {
        assert!(decode_cursor(&serde_json::json!("nope")).is_none());
        assert!(decode_cursor(&serde_json::json!([])).is_none());
        assert!(decode_cursor(&serde_json::json!([0.1])).is_none());
        // Valid array shape but the second element is not a parseable ProductId.
        assert!(decode_cursor(&serde_json::json!([0.1, "not-a-uuid"])).is_none());
    }
}
