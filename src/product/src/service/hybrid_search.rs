//! Adaptive product retrieval that runs lexical BM25 and semantic kNN as separate
//! OpenSearch queries and fuses the candidates in Rust.
//!
//! The flow is:
//! 1. Run a small BM25 probe to measure how sharp the lexical intent already is.
//! 2. Derive soft intent signals from query text, embedding, and the probe scores.
//! 3. Route clearly precision-dominant queries to the regular BM25 path.
//! 4. Otherwise fetch BM25 and filtered semantic candidates independently, apply hard
//!    semantic relevance cutoffs to vector-only candidates, and rank with weighted RRF.
//! 5. Return a deterministic cursor over the fused order so user-facing endless scroll
//!    does not depend on OpenSearch native hybrid pagination quirks.

use crate::core::product::{LocalizedProductView, Product};
use crate::core::product_search::ProductSearch;
use crate::core::sort_product_field::SortProductField;
use crate::opensearch::intent::{
    HybridSearchParams, IntentSignals, compute_intent_signals, intent_centroids,
    semantic_dropout_floor, should_prefer_lexical_search,
};
use crate::opensearch::product_document::ProductDocument;
use crate::opensearch::repository::ProductOpenSearchRepository;
use common::language::domain::Language;
use common::opensearch::search_response::{OpenSearchTimedOutError, SearchHit, SearchResponse};
use common::pagination::cursor::{Cursor, CursoredResult};
use common::product_id::ProductId;
use common::sort::{Sort, SortOrder};
use serde_json::{Value, json};
use std::cmp::Ordering;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};

const DEFAULT_PAGE_SIZE: u64 = 20;
const HYBRID_BM25_PROBE_SIZE: u64 = 8;
const HYBRID_CANDIDATE_OVERSAMPLE: u64 = 5;
const HYBRID_MAX_CANDIDATE_WINDOW: u64 = 1_200;
const HYBRID_RRF_K: f64 = 60.0;
const HYBRID_CURSOR_STRATEGY: &str = "manual_hybrid_rrf_v1";
const SCORE_EPSILON: f64 = 1e-9;

// Empirical relevance guardrails for Gemini Embedding 2 product embeddings.
//
// These thresholds deliberately prefer precision over recall. Hybrid search is used for
// user-facing product discovery where false positives are worse than false negatives, so
// semantic candidates must clear both an absolute intent-derived floor and a tight relative
// tail cutoff from the best semantic hit. A significant score gap is treated as the start of
// the noisy ANN tail and raises the floor further.
const SEMANTIC_TAIL_MARGIN_MIN: f32 = 0.10;
const SEMANTIC_TAIL_MARGIN_VECTOR_WEIGHT_BONUS: f32 = 0.08;
const SEMANTIC_TAIL_MARGIN_MAX: f32 = 0.18;
const SEMANTIC_RELATIVE_FLOOR_MAX: f32 = 0.84;
const SEMANTIC_SIGNIFICANT_GAP: f32 = 0.05;

// BM25 scores are not globally comparable, but within one request a candidate that falls far
// below the best lexical hit is usually broad-query tail noise. Keep the floor relative and low
// enough to preserve flat exact-title result sets while dropping clearly weak lexical tails.
const BM25_RELATIVE_FLOOR_MIN: f64 = 0.12;
const BM25_RELATIVE_FLOOR_VECTOR_WEIGHT_BONUS: f64 = 0.08;

struct HybridFilterContext<'a> {
    query_text: &'a str,
    embedding: &'a [f32],
    languages: &'a [Language],
    params: HybridSearchParams,
    min_semantic_cosine: f32,
}

#[derive(Debug, Clone, Default)]
struct HybridCursorState {
    rank_after: usize,
    product_id: Option<String>,
    fused_score: Option<f64>,
}

struct SemanticCandidate {
    source: ProductDocument,
    score: Option<f64>,
    semantic_cosine: Option<f32>,
}

struct FusionCandidate {
    product_id_sort: String,
    doc: ProductDocument,
    bm25_rank: Option<usize>,
    bm25_score: Option<f64>,
    vector_rank: Option<usize>,
    semantic_cosine: Option<f32>,
}

struct RankedHybridHit {
    product_id_sort: String,
    doc: ProductDocument,
    fused_score: f64,
    bm25_score: Option<f64>,
    semantic_cosine: Option<f32>,
}

#[derive(thiserror::Error, Debug)]
pub enum HybridSearchError {
    #[error("OpenSearchError: {0}")]
    OpenSearchError(#[from] opensearch::Error),
    #[error("OpenSearchTimedOut: {0}")]
    OpenSearchTimedOut(#[from] OpenSearchTimedOutError),
}

/// Outcome of a single adaptive search request: the cursored result plus the derived
/// intent signals & params so callers can introspect / log them.
pub struct HybridSearchOutcome {
    pub items: CursoredResult<LocalizedProductView, serde_json::Value>,
    pub intent: IntentSignals,
    pub params: HybridSearchParams,
}

/// Run an adaptive search.
///
/// Highly specific product lookups stay on the lexical BM25 path. Broader visual / style /
/// exploratory queries use independent BM25 and kNN retrieval, hard-cut weak semantic tail
/// hits, then fuse the remaining candidates in Rust with deterministic pagination.
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

    if query_text.is_empty() {
        let intent = IntentSignals::uniform();
        let params = HybridSearchParams::from(&intent);
        let response = repository
            .search_product_documents(search, &score_sort(), page)
            .await?
            .into_non_timed_out("product lexical fallback search")?;

        return Ok(HybridSearchOutcome {
            items: map_search_response(search, languages, response),
            intent,
            params,
        });
    }

    let probe_cursor = Some(Cursor {
        size: HYBRID_BM25_PROBE_SIZE,
        search_after: None,
    });
    let probe_response = repository
        .search_product_documents(search, &score_sort(), &probe_cursor)
        .await?
        .into_non_timed_out("product hybrid bm25 probe")?;

    let bm25_scores = probe_response
        .hits
        .hits
        .iter()
        .filter_map(|hit| hit.score.map(|score| score as f32))
        .collect::<Vec<_>>();
    let intent = compute_intent_signals(
        &query_text,
        Some(embedding),
        &bm25_scores,
        intent_centroids(),
    );
    let params = HybridSearchParams::from(&intent);

    if should_prefer_lexical_search(&intent) {
        let response = repository
            .search_product_documents(search, &score_sort(), page)
            .await?
            .into_non_timed_out("product lexical precision search")?;

        return Ok(HybridSearchOutcome {
            items: map_search_response(search, languages, response),
            intent,
            params,
        });
    }

    let filter_context = HybridFilterContext {
        query_text: &query_text,
        embedding,
        languages,
        params,
        min_semantic_cosine: semantic_dropout_floor(&intent),
    };
    let items = fetch_fused_hybrid_page(repository, search, page, &filter_context).await?;

    Ok(HybridSearchOutcome {
        items,
        intent,
        params,
    })
}

async fn fetch_fused_hybrid_page(
    repository: &(dyn ProductOpenSearchRepository + Sync),
    search: &ProductSearch,
    page: &Option<Cursor<serde_json::Value>>,
    context: &HybridFilterContext<'_>,
) -> Result<CursoredResult<LocalizedProductView, serde_json::Value>, HybridSearchError> {
    let requested_size = requested_page_size(page);
    let cursor_state = parse_hybrid_cursor(page);
    let candidate_window = candidate_window(requested_size, &cursor_state, context.params);
    let bm25_cursor = Some(Cursor {
        size: candidate_window,
        search_after: None,
    });
    let semantic_k = candidate_window.min(u16::MAX as u64) as u16;
    let sort = score_sort();

    let (bm25_response, semantic_response) = tokio::try_join!(
        repository.search_product_documents(search, &sort, &bm25_cursor),
        repository.semantic_search_product_documents(search, context.embedding, semantic_k),
    )?;

    let bm25_response = bm25_response.into_non_timed_out("product hybrid bm25 candidates")?;
    let semantic_response =
        semantic_response.into_non_timed_out("product hybrid semantic candidates")?;

    let bm25_total = bm25_response.hits.total.value;
    let bm25_hit_count = bm25_response.hits.hits.len() as u64;
    let semantic_hit_count = semantic_response.hits.hits.len() as u64;
    let ranked = fuse_hybrid_hits(
        search,
        context,
        bm25_response.hits.hits,
        semantic_response.hits.hits,
    );

    let start_index = page_start_index(&ranked, &cursor_state);
    let page_hits = ranked
        .iter()
        .skip(start_index)
        .take(requested_size as usize)
        .collect::<Vec<_>>();

    let items = page_hits
        .iter()
        .map(|hit| Product::from(hit.doc.clone()).localized(&search.currency, context.languages))
        .collect::<Vec<_>>();

    let page_end = start_index.saturating_add(page_hits.len());
    let bm25_has_more = bm25_hit_count >= candidate_window && bm25_total > candidate_window;
    let semantic_has_more =
        semantic_hit_count >= candidate_window && candidate_window < HYBRID_MAX_CANDIDATE_WINDOW;
    let maybe_more =
        page_end < ranked.len() || (!page_hits.is_empty() && (bm25_has_more || semantic_has_more));
    let next_search_after = page_hits
        .last()
        .and_then(|last| maybe_more.then(|| build_hybrid_cursor(page_end, last)));

    Ok(CursoredResult {
        cursor: Cursor {
            size: items.len() as u64,
            search_after: next_search_after,
        },
        items,
        // The fused result set is intentionally candidate-window based and client-pruned.
        // Returning the raw BM25 total would be misleading once semantic-only hits are added
        // and weak vector candidates are removed.
        total: None,
    })
}

fn fuse_hybrid_hits(
    search: &ProductSearch,
    context: &HybridFilterContext<'_>,
    bm25_hits: Vec<SearchHit<ProductDocument>>,
    semantic_hits: Vec<SearchHit<ProductDocument>>,
) -> Vec<RankedHybridHit> {
    let mut candidates = HashMap::<ProductId, FusionCandidate>::with_capacity(
        bm25_hits.len().saturating_add(semantic_hits.len()),
    );

    for (idx, hit) in bm25_hits.into_iter().enumerate() {
        let doc = hit.source;
        let product_id = doc.product_id;
        candidates.insert(
            product_id,
            FusionCandidate {
                product_id_sort: product_id.to_string(),
                doc,
                bm25_rank: Some(idx + 1),
                bm25_score: hit.score,
                vector_rank: None,
                semantic_cosine: None,
            },
        );
    }

    let mut semantic_candidates = semantic_hits
        .into_iter()
        .map(|hit| SemanticCandidate {
            semantic_cosine: hit
                .source
                .embedding
                .as_deref()
                .and_then(|doc_embedding| cosine_similarity(context.embedding, doc_embedding)),
            score: hit.score,
            source: hit.source,
        })
        .collect::<Vec<_>>();

    let semantic_floor = semantic_acceptance_floor(context, &semantic_candidates);
    semantic_candidates
        .retain(|candidate| should_keep_semantic_candidate(semantic_floor, candidate));
    semantic_candidates.sort_by(compare_semantic_candidates);

    for (idx, candidate) in semantic_candidates.into_iter().enumerate() {
        let product_id = candidate.source.product_id;
        let product_id_sort = product_id.to_string();
        match candidates.entry(product_id) {
            Entry::Occupied(mut entry) => {
                let entry = entry.get_mut();
                entry.vector_rank = Some(idx + 1);
                entry.semantic_cosine =
                    max_optional_f32(entry.semantic_cosine, candidate.semantic_cosine);
                if entry.doc.embedding.is_none() && candidate.source.embedding.is_some() {
                    entry.doc = candidate.source;
                }
            }
            Entry::Vacant(entry) => {
                entry.insert(FusionCandidate {
                    product_id_sort,
                    doc: candidate.source,
                    bm25_rank: None,
                    bm25_score: None,
                    vector_rank: Some(idx + 1),
                    semantic_cosine: candidate.semantic_cosine,
                });
            }
        }
    }

    let bm25_floor = bm25_acceptance_floor(candidates.values(), context.params);
    let bm25_weight =
        (1.0 - context.params.vector_weight).max(HybridSearchParams::MIN_BM25_WEIGHT) as f64;
    let vector_weight = context.params.vector_weight as f64;
    let mut ranked = candidates
        .into_values()
        .filter(|candidate| {
            should_keep_fusion_candidate(search, context, bm25_floor, semantic_floor, candidate)
        })
        .map(|candidate| {
            let fused_score = candidate
                .bm25_rank
                .map(|rank| reciprocal_rank_score(rank, bm25_weight))
                .unwrap_or_default()
                + candidate
                    .vector_rank
                    .map(|rank| reciprocal_rank_score(rank, vector_weight))
                    .unwrap_or_default();

            RankedHybridHit {
                product_id_sort: candidate.product_id_sort,
                doc: candidate.doc,
                fused_score,
                bm25_score: candidate.bm25_score,
                semantic_cosine: candidate.semantic_cosine,
            }
        })
        .collect::<Vec<_>>();

    ranked.sort_by(compare_ranked_hybrid_hits);
    ranked
}

fn semantic_acceptance_floor(
    context: &HybridFilterContext<'_>,
    candidates: &[SemanticCandidate],
) -> f32 {
    let top_semantic = candidates
        .iter()
        .filter_map(|candidate| candidate.semantic_cosine)
        .max_by(|lhs, rhs| lhs.partial_cmp(rhs).unwrap_or(Ordering::Equal));

    let mut semantic_cosines = candidates
        .iter()
        .filter_map(|candidate| candidate.semantic_cosine)
        .filter(|cosine| cosine.is_finite())
        .collect::<Vec<_>>();
    semantic_cosines.sort_by(|lhs, rhs| rhs.partial_cmp(lhs).unwrap_or(Ordering::Equal));

    match top_semantic {
        Some(top) => {
            let relative_floor = top - semantic_tail_margin(context.params);
            let gap_floor =
                semantic_gap_floor(&semantic_cosines).unwrap_or(context.min_semantic_cosine);
            relative_floor
                .max(gap_floor)
                .max(context.min_semantic_cosine)
                .clamp(context.min_semantic_cosine, SEMANTIC_RELATIVE_FLOOR_MAX)
        }
        None => context.min_semantic_cosine,
    }
}

fn semantic_tail_margin(params: HybridSearchParams) -> f32 {
    (SEMANTIC_TAIL_MARGIN_MIN + SEMANTIC_TAIL_MARGIN_VECTOR_WEIGHT_BONUS * params.vector_weight)
        .clamp(SEMANTIC_TAIL_MARGIN_MIN, SEMANTIC_TAIL_MARGIN_MAX)
}

fn semantic_gap_floor(sorted_cosines_desc: &[f32]) -> Option<f32> {
    sorted_cosines_desc.windows(2).find_map(|window| {
        let before_gap = window[0];
        let after_gap = window[1];
        (before_gap - after_gap >= SEMANTIC_SIGNIFICANT_GAP).then_some(before_gap)
    })
}

fn should_keep_semantic_candidate(semantic_floor: f32, candidate: &SemanticCandidate) -> bool {
    candidate
        .semantic_cosine
        .is_some_and(|cosine| cosine + 1e-6 >= semantic_floor)
}

fn bm25_acceptance_floor<'a>(
    candidates: impl Iterator<Item = &'a FusionCandidate>,
    params: HybridSearchParams,
) -> Option<f64> {
    let top_bm25 = candidates
        .filter_map(|candidate| candidate.bm25_score)
        .filter(|score| score.is_finite() && *score > 0.0)
        .max_by(|lhs, rhs| lhs.partial_cmp(rhs).unwrap_or(Ordering::Equal))?;
    let relative_floor = BM25_RELATIVE_FLOOR_MIN
        + BM25_RELATIVE_FLOOR_VECTOR_WEIGHT_BONUS * params.vector_weight as f64;
    Some(top_bm25 * relative_floor)
}

fn should_keep_fusion_candidate(
    search: &ProductSearch,
    context: &HybridFilterContext<'_>,
    bm25_floor: Option<f64>,
    semantic_floor: f32,
    candidate: &FusionCandidate,
) -> bool {
    let has_lexical_quality = bm25_floor.is_some_and(|floor| {
        candidate
            .bm25_score
            .is_some_and(|score| score + SCORE_EPSILON >= floor)
            && hit_has_text_anchor(search, context.query_text, &candidate.doc)
    });
    let has_semantic_quality = candidate.semantic_cosine.is_some_and(|cosine| {
        cosine + 1e-6 >= semantic_floor
            && semantic_has_required_anchor(search, context.query_text, &candidate.doc)
    });

    has_lexical_quality || has_semantic_quality
}

fn reciprocal_rank_score(rank: usize, weight: f64) -> f64 {
    weight / (HYBRID_RRF_K + rank as f64)
}

fn candidate_window(
    requested_size: u64,
    cursor: &HybridCursorState,
    params: HybridSearchParams,
) -> u64 {
    let rank_after = cursor.rank_after as u64;
    rank_after
        .saturating_add(requested_size.saturating_mul(HYBRID_CANDIDATE_OVERSAMPLE))
        .max(params.candidate_k as u64)
        .max(requested_size)
        .min(HYBRID_MAX_CANDIDATE_WINDOW)
}

fn parse_hybrid_cursor(page: &Option<Cursor<Value>>) -> HybridCursorState {
    let Some(search_after) = page
        .as_ref()
        .and_then(|cursor| cursor.search_after.as_ref())
    else {
        return HybridCursorState::default();
    };
    let Some(obj) = search_after.as_object() else {
        return HybridCursorState::default();
    };

    let strategy_matches = obj
        .get("strategy")
        .and_then(Value::as_str)
        .is_some_and(|strategy| strategy == HYBRID_CURSOR_STRATEGY);
    if !strategy_matches && !obj.contains_key("rankAfter") {
        return HybridCursorState::default();
    }

    HybridCursorState {
        rank_after: obj
            .get("rankAfter")
            .and_then(Value::as_u64)
            .unwrap_or_default() as usize,
        product_id: obj
            .get("productId")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        fused_score: obj.get("score").and_then(Value::as_f64),
    }
}

fn page_start_index(ranked: &[RankedHybridHit], cursor: &HybridCursorState) -> usize {
    if let Some(product_id) = cursor.product_id.as_deref() {
        if let Some(pos) = ranked.iter().position(|hit| {
            hit.product_id_sort == product_id
                && cursor
                    .fused_score
                    .map(|score| scores_equal(hit.fused_score, score))
                    .unwrap_or(true)
        }) {
            return pos + 1;
        }

        if let Some(pos) = ranked
            .iter()
            .position(|hit| hit.product_id_sort == product_id)
        {
            return pos + 1;
        }
    }

    cursor.rank_after.min(ranked.len())
}

fn build_hybrid_cursor(rank_after: usize, last: &RankedHybridHit) -> Value {
    json!({
        "strategy": HYBRID_CURSOR_STRATEGY,
        "rankAfter": rank_after,
        "score": last.fused_score,
        "productId": last.product_id_sort,
    })
}

fn scores_equal(lhs: f64, rhs: f64) -> bool {
    (lhs - rhs).abs() <= SCORE_EPSILON
}

fn compare_ranked_hybrid_hits(lhs: &RankedHybridHit, rhs: &RankedHybridHit) -> Ordering {
    compare_f64_desc(lhs.fused_score, rhs.fused_score)
        .then_with(|| compare_option_f32_desc(lhs.semantic_cosine, rhs.semantic_cosine))
        .then_with(|| compare_option_f64_desc(lhs.bm25_score, rhs.bm25_score))
        .then_with(|| lhs.product_id_sort.cmp(&rhs.product_id_sort))
}

fn compare_semantic_candidates(lhs: &SemanticCandidate, rhs: &SemanticCandidate) -> Ordering {
    compare_option_f32_desc(lhs.semantic_cosine, rhs.semantic_cosine)
        .then_with(|| compare_option_f64_desc(lhs.score, rhs.score))
        .then_with(|| lhs.source.product_id.cmp(&rhs.source.product_id))
}

fn compare_f64_desc(lhs: f64, rhs: f64) -> Ordering {
    rhs.partial_cmp(&lhs).unwrap_or(Ordering::Equal)
}

fn compare_option_f64_desc(lhs: Option<f64>, rhs: Option<f64>) -> Ordering {
    match (lhs, rhs) {
        (Some(lhs), Some(rhs)) => compare_f64_desc(lhs, rhs),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn compare_option_f32_desc(lhs: Option<f32>, rhs: Option<f32>) -> Ordering {
    match (lhs, rhs) {
        (Some(lhs), Some(rhs)) => rhs.partial_cmp(&lhs).unwrap_or(Ordering::Equal),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn max_optional_f32(lhs: Option<f32>, rhs: Option<f32>) -> Option<f32> {
    match (lhs, rhs) {
        (Some(lhs), Some(rhs)) => Some(lhs.max(rhs)),
        (Some(lhs), None) => Some(lhs),
        (None, Some(rhs)) => Some(rhs),
        (None, None) => None,
    }
}

fn hit_has_text_anchor(search: &ProductSearch, query_text: &str, doc: &ProductDocument) -> bool {
    searchable_titles(search, doc)
        .into_iter()
        .any(|title| title_has_query_anchor(title, query_text))
}

fn semantic_has_required_anchor(
    search: &ProductSearch,
    query_text: &str,
    doc: &ProductDocument,
) -> bool {
    let required_tokens = semantic_required_anchor_tokens(query_text);
    if required_tokens.is_empty() {
        return true;
    }

    searchable_titles(search, doc)
        .into_iter()
        .any(|title| title_satisfies_required_anchor(title, &required_tokens))
}

fn searchable_titles<'a>(search: &ProductSearch, doc: &'a ProductDocument) -> Vec<&'a str> {
    let mut titles = Vec::with_capacity(2);
    if let Some(title) = localized_title_for_search(search, doc) {
        titles.push(title);
    }
    titles.push(doc.title_native.text.as_str());
    titles
}

fn localized_title_for_search<'a>(
    search: &ProductSearch,
    doc: &'a ProductDocument,
) -> Option<&'a str> {
    match search.language {
        Language::De => doc.title_de.as_deref(),
        Language::En => doc.title_en.as_deref(),
        Language::Fr => doc.title_fr.as_deref(),
        Language::Es => doc.title_es.as_deref(),
        Language::It => doc.title_it.as_deref(),
        _ => doc.title_en.as_deref(),
    }
}

fn title_satisfies_required_anchor(title: &str, required_tokens: &[String]) -> bool {
    let title_tokens = anchor_tokens(title);
    let object_anchor_matches = required_tokens
        .last()
        .is_none_or(|token| title_tokens.contains(token));
    if !object_anchor_matches {
        return false;
    }

    let matched = required_tokens
        .iter()
        .filter(|token| title_tokens.contains(*token))
        .count();

    if required_tokens.len() <= 3 {
        matched == required_tokens.len()
    } else {
        matched as f32 / required_tokens.len() as f32 >= 0.75
    }
}

fn semantic_required_anchor_tokens(query_text: &str) -> Vec<String> {
    anchor_tokens_in_order(query_text)
        .into_iter()
        .filter(|token| !is_low_precision_semantic_anchor_token(token))
        .collect()
}

fn is_low_precision_semantic_anchor_token(token: &str) -> bool {
    // These broad browsing / condition words are too generic to prove semantic relevance on
    // their own. Style/period wording is ignored only for generic translations of "style" or
    // "period"; concrete period names like "deco", "nouveau", "bauhaus", or "biedermeier"
    // intentionally stay anchorable. Colour, material, origin, and object words also stay
    // anchorable because false positives are worse than dropping a plausible but unanchored hit.
    matches!(
        token,
        // English
        "antique"
            | "antiques"
            | "antiquity"
            | "antiquities"
            | "vintage"
            | "ancient"
            | "old"
            | "rare"
            | "style"
            | "period"
            | "pair"
            | "set"
            // German
            | "antik"
            | "antike"
            | "antikes"
            | "antiker"
            | "antiken"
            | "antiquität"
            | "antiquitaet"
            | "antiquitäten"
            | "antiquitaeten"
            | "alt"
            | "alte"
            | "altes"
            | "alter"
            | "alten"
            | "selten"
            | "seltene"
            | "seltener"
            | "seltenes"
            | "seltenen"
            | "seltenem"
            | "stil"
            | "periode"
            | "epoche"
            | "paar"
            | "satz"
            | "garnitur"
            // French
            | "antiquité"
            | "antiquite"
            | "antiquités"
            | "antiquites"
            | "ancien"
            | "ancienne"
            | "anciens"
            | "anciennes"
            | "vieux"
            | "vieille"
            | "vieilles"
            | "rares"
            | "période"
            | "époque"
            | "epoque"
            | "paire"
            | "ensemble"
            | "lot"
            | "série"
            | "serie"
            // Spanish
            | "antigüedad"
            | "antiguedad"
            | "antigüedades"
            | "antiguedades"
            | "antiguo"
            | "antigua"
            | "antiguos"
            | "antiguas"
            | "viejo"
            | "vieja"
            | "viejos"
            | "viejas"
            | "raro"
            | "rara"
            | "raros"
            | "raras"
            | "estilo"
            | "período"
            | "periodo"
            | "época"
            | "epoca"
            | "par"
            | "pareja"
            | "conjunto"
            | "juego"
            | "lote"
            // Italian
            | "antico"
            | "antica"
            | "antichi"
            | "antiche"
            | "antichità"
            | "antichita"
            | "vecchio"
            | "vecchia"
            | "vecchi"
            | "vecchie"
            | "rari"
            | "stile"
            | "coppia"
            | "paio"
    )
}

fn title_has_query_anchor(title: &str, query_text: &str) -> bool {
    let normalized_query = normalize_phrase_for_anchor(query_text);
    if normalized_query.is_empty() {
        return false;
    }

    let normalized_title = normalize_phrase_for_anchor(title);
    if normalized_title.contains(&normalized_query) {
        return true;
    }

    let query_tokens = anchor_tokens(query_text);
    if query_tokens.is_empty() {
        return false;
    }
    let title_tokens = anchor_tokens(title);
    let matched = query_tokens
        .iter()
        .filter(|token| title_tokens.contains(*token))
        .count();
    let coverage = matched as f32 / query_tokens.len() as f32;
    let minimum_coverage = if query_tokens.len() <= 2 { 1.0 } else { 0.75 };

    coverage >= minimum_coverage
}

fn normalize_phrase_for_anchor(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn anchor_tokens(text: &str) -> HashSet<String> {
    anchor_tokens_in_order(text).into_iter().collect()
}

fn anchor_tokens_in_order(text: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|token| {
            !token.is_empty() && (token.len() >= 3 || token.chars().all(|ch| ch.is_ascii_digit()))
        })
        .filter_map(|token| {
            let token = token.to_string();
            seen.insert(token.clone()).then_some(token)
        })
        .collect()
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> Option<f32> {
    if a.is_empty() || b.is_empty() || a.len() != b.len() {
        return None;
    }

    let a_norm = a.iter().map(|value| value * value).sum::<f32>().sqrt();
    let b_norm = b.iter().map(|value| value * value).sum::<f32>().sqrt();
    if a_norm == 0.0 || b_norm == 0.0 {
        return None;
    }

    let dot = a
        .iter()
        .zip(b.iter())
        .map(|(lhs, rhs)| lhs * rhs)
        .sum::<f32>();
    Some(dot / (a_norm * b_norm))
}

fn requested_page_size(page: &Option<Cursor<serde_json::Value>>) -> u64 {
    page.as_ref()
        .map(|cursor| cursor.size)
        .unwrap_or(DEFAULT_PAGE_SIZE)
        .max(1)
}

fn score_sort() -> Sort<SortProductField> {
    Sort {
        sort: SortProductField::Score,
        order: SortOrder::Desc,
    }
}

fn map_search_response(
    search: &ProductSearch,
    languages: &[Language],
    response: SearchResponse<ProductDocument>,
) -> CursoredResult<LocalizedProductView, serde_json::Value> {
    let cursor = Cursor {
        size: response.hits.hits.len() as u64,
        search_after: response.hits.hits.last().and_then(|last| last.sort.clone()),
    };
    let total = response.hits.total.value;
    let currency = search.currency;
    let items = response
        .hits
        .hits
        .into_iter()
        .map(|hit| Product::from(hit.source).localized(&currency, languages))
        .collect();

    CursoredResult {
        items,
        cursor,
        total: Some(total),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opensearch::repository::MockProductOpenSearchRepository;
    use common::language::document::{LanguageDocument, TextDocument};
    use common::opensearch::search_response::{
        HitsMetadata, SearchHit, SearchResponse, ShardStats, TotalHits,
    };
    use fake::{Fake, Faker};
    use mockall::Sequence;
    use serde_json::json;

    fn mk_search() -> ProductSearch {
        let mut s: ProductSearch = Faker.fake();
        s.language = Language::En;
        s.product_query = Some("art deco lamp".try_into().unwrap());
        s
    }

    fn mk_doc(title: &str) -> ProductDocument {
        let mut doc: ProductDocument = Faker.fake();
        doc.embedding = None;
        doc.title_en = Some(title.to_string());
        doc.title_native = TextDocument {
            text: title.to_string(),
            language: LanguageDocument::En,
        };
        doc
    }

    fn one_hot_embedding(slot: usize) -> Vec<f32> {
        let mut embedding = vec![0.0_f32; 768];
        embedding[slot] = 1.0;
        embedding
    }

    fn embedding_with_cosine(cosine: f32) -> Vec<f32> {
        let cosine = cosine.clamp(-1.0, 1.0);
        let mut embedding = vec![0.0_f32; 768];
        embedding[0] = cosine;
        embedding[1] = (1.0 - cosine * cosine).sqrt();
        embedding
    }

    fn mk_hit(
        doc: ProductDocument,
        score: f64,
        sort: serde_json::Value,
        matched_queries: Vec<&str>,
    ) -> SearchHit<ProductDocument> {
        SearchHit {
            index: "products".to_string(),
            id: doc.product_id.to_string(),
            score: Some(score),
            sort: Some(sort),
            matched_queries: matched_queries.into_iter().map(str::to_string).collect(),
            source: doc,
        }
    }

    fn mk_response(hits: Vec<SearchHit<ProductDocument>>) -> SearchResponse<ProductDocument> {
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
                    value: hits.len() as u64,
                    relation: "eq".to_string(),
                },
                max_score: hits.first().and_then(|hit| hit.score),
                hits,
            },
        }
    }

    fn timed_out_response() -> SearchResponse<ProductDocument> {
        SearchResponse {
            took: 120,
            timed_out: true,
            shards: ShardStats {
                total: 4,
                successful: 3,
                skipped: 0,
                failed: 1,
            },
            hits: HitsMetadata {
                total: TotalHits {
                    value: 0,
                    relation: "eq".to_string(),
                },
                max_score: None,
                hits: vec![],
            },
        }
    }

    fn flat_probe() -> SearchResponse<ProductDocument> {
        mk_response(vec![
            mk_hit(mk_doc("art deco lamp"), 1.0, json!([1.0]), vec![]),
            mk_hit(mk_doc("art deco floor lamp"), 0.99, json!([0.99]), vec![]),
            mk_hit(mk_doc("art deco table lamp"), 0.98, json!([0.98]), vec![]),
        ])
    }

    #[tokio::test]
    async fn should_dispatch_parallel_bm25_and_semantic_queries_for_text_search() {
        let mut repo = MockProductOpenSearchRepository::default();
        let probe = flat_probe();
        let bm25_doc = mk_doc("art deco lamp");
        let bm25_candidates =
            mk_response(vec![mk_hit(bm25_doc, 1.0, json!([1.0, "bm25"]), vec![])]);
        let mut semantic_doc = mk_doc("art deco lamp");
        semantic_doc.embedding = Some(one_hot_embedding(0));
        let semantic_candidates = mk_response(vec![mk_hit(
            semantic_doc,
            1.0,
            json!([1.0, "semantic"]),
            vec![],
        )]);

        repo.expect_search_product_documents()
            .times(2)
            .returning(move |_, _, cursor| {
                let response = if cursor.as_ref().map(|c| c.size) == Some(HYBRID_BM25_PROBE_SIZE) {
                    probe.clone()
                } else {
                    bm25_candidates.clone()
                };
                Box::pin(async move { Ok(response) })
            });
        repo.expect_semantic_search_product_documents()
            .times(1)
            .return_once(move |_, embedding, k| {
                assert_eq!(embedding[0], 1.0);
                assert!(k >= HybridSearchParams::MIN_CANDIDATE_K);
                Box::pin(async move { Ok(semantic_candidates) })
            });

        let search = mk_search();
        let embedding = one_hot_embedding(0);
        let outcome = hybrid_search(&repo, &search, &embedding, &None, &[search.language])
            .await
            .unwrap();
        assert_eq!(outcome.items.items.len(), 2);
        assert!(outcome.params.vector_weight <= 1.0 - HybridSearchParams::MIN_BM25_WEIGHT);
        assert!(outcome.params.candidate_k >= HybridSearchParams::MIN_CANDIDATE_K);
        assert!(outcome.params.candidate_k <= HybridSearchParams::MAX_CANDIDATE_K);
        assert!(outcome.items.total.is_none());
    }

    #[tokio::test]
    async fn should_err_when_bm25_probe_times_out() {
        let mut repo = MockProductOpenSearchRepository::default();
        repo.expect_search_product_documents()
            .times(1)
            .return_once(|_, _, _| Box::pin(async move { Ok(timed_out_response()) }));

        let search = mk_search();
        let result = hybrid_search(
            &repo,
            &search,
            &one_hot_embedding(0),
            &None,
            &[search.language],
        )
        .await;

        assert!(matches!(
            result,
            Err(HybridSearchError::OpenSearchTimedOut(_))
        ));
    }

    #[tokio::test]
    async fn should_err_when_semantic_candidate_query_times_out() {
        let mut repo = MockProductOpenSearchRepository::default();
        let probe = flat_probe();
        let bm25_candidates = mk_response(vec![]);

        repo.expect_search_product_documents()
            .times(2)
            .returning(move |_, _, cursor| {
                let response = if cursor.as_ref().map(|c| c.size) == Some(HYBRID_BM25_PROBE_SIZE) {
                    probe.clone()
                } else {
                    bm25_candidates.clone()
                };
                Box::pin(async move { Ok(response) })
            });
        repo.expect_semantic_search_product_documents()
            .times(1)
            .return_once(|_, _, _| Box::pin(async move { Ok(timed_out_response()) }));

        let search = mk_search();
        let result = hybrid_search(
            &repo,
            &search,
            &one_hot_embedding(0),
            &None,
            &[search.language],
        )
        .await;

        assert!(matches!(
            result,
            Err(HybridSearchError::OpenSearchTimedOut(_))
        ));
    }

    #[tokio::test]
    async fn should_route_precision_query_to_lexical_search_when_probe_is_sharply_peaked() {
        let mut repo = MockProductOpenSearchRepository::default();
        let mut seq = Sequence::new();

        let probe = mk_response(vec![
            mk_hit(mk_doc("Rolex Submariner 1965"), 12.0, json!([12.0]), vec![]),
            mk_hit(mk_doc("Rolex Submariner 1964"), 0.6, json!([0.6]), vec![]),
            mk_hit(mk_doc("Rolex Submariner 1963"), 0.5, json!([0.5]), vec![]),
        ]);
        let exact = mk_doc("Rolex Submariner 1965");
        let lexical = mk_response(vec![mk_hit(
            exact.clone(),
            12.0,
            json!([12.0, exact.product_id]),
            vec![],
        )]);

        repo.expect_search_product_documents()
            .times(1)
            .in_sequence(&mut seq)
            .return_once(move |_, _, _| Box::pin(async move { Ok(probe) }));
        repo.expect_search_product_documents()
            .times(1)
            .in_sequence(&mut seq)
            .return_once(move |_, _, _| Box::pin(async move { Ok(lexical) }));
        repo.expect_semantic_search_product_documents().times(0);

        let mut search = mk_search();
        search.product_query = Some("Rolex Submariner 1965".try_into().unwrap());
        let embedding = one_hot_embedding(0);
        let outcome = hybrid_search(&repo, &search, &embedding, &None, &[search.language])
            .await
            .unwrap();

        assert_eq!(outcome.items.items.len(), 1);
        assert_eq!(outcome.items.items[0].product_id, exact.product_id);
        assert_eq!(outcome.items.total, Some(1));
    }

    #[tokio::test]
    async fn should_drop_low_similarity_vector_only_candidates() {
        let mut repo = MockProductOpenSearchRepository::default();
        let probe = flat_probe();
        let bm25_anchor = mk_doc("blue ceramic ornate vase");
        let bm25_candidates = mk_response(vec![mk_hit(
            bm25_anchor.clone(),
            1.0,
            json!([1.0, bm25_anchor.product_id]),
            vec![],
        )]);

        let mut vector_target = mk_doc("blue ceramic ornate vase");
        vector_target.embedding = Some(one_hot_embedding(0));
        let mut vector_noise = mk_doc("unrelated text");
        vector_noise.embedding = Some(one_hot_embedding(1));
        let semantic_candidates = mk_response(vec![
            mk_hit(vector_target.clone(), 1.0, json!([1.0]), vec![]),
            mk_hit(vector_noise.clone(), 0.9, json!([0.9]), vec![]),
        ]);

        repo.expect_search_product_documents()
            .times(2)
            .returning(move |_, _, cursor| {
                let response = if cursor.as_ref().map(|c| c.size) == Some(HYBRID_BM25_PROBE_SIZE) {
                    probe.clone()
                } else {
                    bm25_candidates.clone()
                };
                Box::pin(async move { Ok(response) })
            });
        repo.expect_semantic_search_product_documents()
            .times(1)
            .return_once(move |_, _, _| Box::pin(async move { Ok(semantic_candidates) }));

        let mut search = mk_search();
        search.product_query = Some("blue ceramic ornate vase".try_into().unwrap());
        let outcome = hybrid_search(
            &repo,
            &search,
            &one_hot_embedding(0),
            &None,
            &[search.language],
        )
        .await
        .unwrap();

        let returned_ids = outcome
            .items
            .items
            .iter()
            .map(|item| item.product_id)
            .collect::<HashSet<_>>();
        assert!(returned_ids.contains(&bm25_anchor.product_id));
        assert!(returned_ids.contains(&vector_target.product_id));
        assert!(!returned_ids.contains(&vector_noise.product_id));
    }

    #[tokio::test]
    async fn should_drop_semantic_tail_after_significant_similarity_gap() {
        let mut repo = MockProductOpenSearchRepository::default();
        let probe = flat_probe();
        let bm25_candidates = mk_response(vec![]);

        let mut vector_target = mk_doc("art deco lamp");
        vector_target.embedding = Some(one_hot_embedding(0));
        let mut semantic_tail = mk_doc("decorative object");
        semantic_tail.embedding = Some(embedding_with_cosine(0.65));
        let semantic_candidates = mk_response(vec![
            mk_hit(vector_target.clone(), 1.0, json!([1.0]), vec![]),
            mk_hit(semantic_tail.clone(), 0.9, json!([0.9]), vec![]),
        ]);

        repo.expect_search_product_documents()
            .times(2)
            .returning(move |_, _, cursor| {
                let response = if cursor.as_ref().map(|c| c.size) == Some(HYBRID_BM25_PROBE_SIZE) {
                    probe.clone()
                } else {
                    bm25_candidates.clone()
                };
                Box::pin(async move { Ok(response) })
            });
        repo.expect_semantic_search_product_documents()
            .times(1)
            .return_once(move |_, _, _| Box::pin(async move { Ok(semantic_candidates) }));

        let search = mk_search();
        let outcome = hybrid_search(
            &repo,
            &search,
            &one_hot_embedding(0),
            &None,
            &[search.language],
        )
        .await
        .unwrap();

        let returned_ids = outcome
            .items
            .items
            .iter()
            .map(|item| item.product_id)
            .collect::<HashSet<_>>();
        assert!(returned_ids.contains(&vector_target.product_id));
        assert!(!returned_ids.contains(&semantic_tail.product_id));
    }

    #[test]
    fn should_ignore_low_precision_semantic_anchor_tokens_across_supported_languages() {
        assert_eq!(
            vec!["silber".to_string(), "löffel".to_string()],
            semantic_required_anchor_tokens("antiker seltener silber löffel")
        );
        assert_eq!(
            vec![
                "montre".to_string(),
                "poche".to_string(),
                "argent".to_string()
            ],
            semantic_required_anchor_tokens("ancienne paire montre de poche argent")
        );
        assert_eq!(
            vec!["silla".to_string(), "art".to_string(), "déco".to_string()],
            semantic_required_anchor_tokens("antigua silla estilo art déco")
        );
        assert_eq!(
            vec![
                "lampada".to_string(),
                "art".to_string(),
                "nouveau".to_string()
            ],
            semantic_required_anchor_tokens("antica coppia lampada stile art nouveau")
        );
    }

    #[tokio::test]
    async fn should_drop_medium_semantic_candidate_without_required_object_anchor() {
        let mut repo = MockProductOpenSearchRepository::default();
        let probe = flat_probe();
        let bm25_candidates = mk_response(vec![]);

        let mut anchored = mk_doc("Art Deco bronze table lamp");
        anchored.embedding = Some(embedding_with_cosine(0.70));
        let mut missing_object = mk_doc("Art Deco gilt bronze herons");
        missing_object.embedding = Some(embedding_with_cosine(0.69));
        let semantic_candidates = mk_response(vec![
            mk_hit(anchored.clone(), 1.0, json!([1.0]), vec![]),
            mk_hit(missing_object.clone(), 0.99, json!([0.99]), vec![]),
        ]);

        repo.expect_search_product_documents()
            .times(2)
            .returning(move |_, _, cursor| {
                let response = if cursor.as_ref().map(|c| c.size) == Some(HYBRID_BM25_PROBE_SIZE) {
                    probe.clone()
                } else {
                    bm25_candidates.clone()
                };
                Box::pin(async move { Ok(response) })
            });
        repo.expect_semantic_search_product_documents()
            .times(1)
            .return_once(move |_, _, _| Box::pin(async move { Ok(semantic_candidates) }));

        let search = mk_search();
        let outcome = hybrid_search(
            &repo,
            &search,
            &one_hot_embedding(0),
            &None,
            &[search.language],
        )
        .await
        .unwrap();

        let returned_ids = outcome
            .items
            .items
            .iter()
            .map(|item| item.product_id)
            .collect::<HashSet<_>>();
        assert!(returned_ids.contains(&anchored.product_id));
        assert!(!returned_ids.contains(&missing_object.product_id));
    }

    #[tokio::test]
    async fn should_drop_weak_bm25_tail_candidates() {
        let mut repo = MockProductOpenSearchRepository::default();
        let probe = flat_probe();
        let strong = mk_doc("art deco lamp");
        let weak_tail = mk_doc("art deco lamp");
        let bm25_candidates = mk_response(vec![
            mk_hit(strong.clone(), 10.0, json!([10.0]), vec![]),
            mk_hit(weak_tail.clone(), 1.0, json!([1.0]), vec![]),
        ]);
        let semantic_candidates = mk_response(vec![]);

        repo.expect_search_product_documents()
            .times(2)
            .returning(move |_, _, cursor| {
                let response = if cursor.as_ref().map(|c| c.size) == Some(HYBRID_BM25_PROBE_SIZE) {
                    probe.clone()
                } else {
                    bm25_candidates.clone()
                };
                Box::pin(async move { Ok(response) })
            });
        repo.expect_semantic_search_product_documents()
            .times(1)
            .return_once(move |_, _, _| Box::pin(async move { Ok(semantic_candidates) }));

        let search = mk_search();
        let outcome = hybrid_search(
            &repo,
            &search,
            &one_hot_embedding(0),
            &None,
            &[search.language],
        )
        .await
        .unwrap();

        let returned_ids = outcome
            .items
            .items
            .iter()
            .map(|item| item.product_id)
            .collect::<HashSet<_>>();
        assert!(returned_ids.contains(&strong.product_id));
        assert!(!returned_ids.contains(&weak_tail.product_id));
    }

    #[tokio::test]
    async fn should_rank_dual_branch_candidate_above_single_branch_candidates() {
        let mut repo = MockProductOpenSearchRepository::default();
        let probe = flat_probe();

        let bm25_only = mk_doc("blue ceramic ornate vase");
        let mut dual = mk_doc("blue ceramic ornate vase");
        dual.embedding = Some(one_hot_embedding(0));
        let bm25_candidates = mk_response(vec![
            mk_hit(bm25_only.clone(), 3.0, json!([3.0]), vec![]),
            mk_hit(dual.clone(), 2.0, json!([2.0]), vec![]),
        ]);

        let mut vector_only = mk_doc("unrelated text");
        vector_only.embedding = Some(one_hot_embedding(0));
        let semantic_candidates = mk_response(vec![
            mk_hit(dual.clone(), 1.0, json!([1.0]), vec![]),
            mk_hit(vector_only, 0.99, json!([0.99]), vec![]),
        ]);

        repo.expect_search_product_documents()
            .times(2)
            .returning(move |_, _, cursor| {
                let response = if cursor.as_ref().map(|c| c.size) == Some(HYBRID_BM25_PROBE_SIZE) {
                    probe.clone()
                } else {
                    bm25_candidates.clone()
                };
                Box::pin(async move { Ok(response) })
            });
        repo.expect_semantic_search_product_documents()
            .times(1)
            .return_once(move |_, _, _| Box::pin(async move { Ok(semantic_candidates) }));

        let mut search = mk_search();
        search.product_query = Some("blue ceramic ornate vase".try_into().unwrap());
        let outcome = hybrid_search(
            &repo,
            &search,
            &one_hot_embedding(0),
            &None,
            &[search.language],
        )
        .await
        .unwrap();

        assert_eq!(outcome.items.items[0].product_id, dual.product_id);
    }

    #[tokio::test]
    async fn should_page_fused_results_with_deterministic_cursor_without_duplicates() {
        let mut repo = MockProductOpenSearchRepository::default();
        let probe = flat_probe();
        let docs = [
            mk_doc("art deco lamp"),
            mk_doc("art deco floor lamp"),
            mk_doc("art deco table lamp"),
        ];
        let bm25_candidates = mk_response(vec![
            mk_hit(docs[0].clone(), 3.0, json!([3.0]), vec![]),
            mk_hit(docs[1].clone(), 2.0, json!([2.0]), vec![]),
            mk_hit(docs[2].clone(), 1.0, json!([1.0]), vec![]),
        ]);
        let semantic_candidates = mk_response(vec![]);

        repo.expect_search_product_documents()
            .times(4)
            .returning(move |_, _, cursor| {
                let response = if cursor.as_ref().map(|c| c.size) == Some(HYBRID_BM25_PROBE_SIZE) {
                    probe.clone()
                } else {
                    bm25_candidates.clone()
                };
                Box::pin(async move { Ok(response) })
            });
        repo.expect_semantic_search_product_documents()
            .times(2)
            .returning(move |_, _, _| {
                let response = semantic_candidates.clone();
                Box::pin(async move { Ok(response) })
            });

        let search = mk_search();
        let embedding = one_hot_embedding(0);
        let first = hybrid_search(
            &repo,
            &search,
            &embedding,
            &Some(Cursor {
                size: 2,
                search_after: None,
            }),
            &[search.language],
        )
        .await
        .unwrap();
        assert_eq!(first.items.items.len(), 2);
        assert_eq!(
            first
                .items
                .cursor
                .search_after
                .as_ref()
                .and_then(|value| value.get("strategy"))
                .and_then(Value::as_str),
            Some(HYBRID_CURSOR_STRATEGY)
        );

        let second = hybrid_search(
            &repo,
            &search,
            &embedding,
            &Some(first.items.cursor.clone()),
            &[search.language],
        )
        .await
        .unwrap();

        assert_eq!(second.items.items.len(), 1);
        let first_ids = first
            .items
            .items
            .iter()
            .map(|item| item.product_id)
            .collect::<HashSet<_>>();
        assert!(!first_ids.contains(&second.items.items[0].product_id));
        assert_eq!(second.items.items[0].product_id, docs[2].product_id);
    }
}
