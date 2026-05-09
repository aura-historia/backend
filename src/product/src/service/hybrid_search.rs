//! Adaptive product retrieval that combines a lexical BM25 probe with native OpenSearch
//! hybrid search.
//!
//! The flow is:
//! 1. Run a small BM25 probe to measure how sharp the lexical intent already is.
//! 2. Derive soft intent signals from query text, embedding, and the probe scores.
//! 3. Route clearly precision-dominant queries to the regular BM25 path.
//! 4. Otherwise run the native hybrid query and trim low-confidence vector-only tail hits
//!    client-side using an adaptive semantic floor.

use crate::core::product::{LocalizedProductView, Product};
use crate::core::product_search::ProductSearch;
use crate::core::sort_product_field::SortProductField;
use crate::opensearch::intent::{
    HybridSearchParams, IntentSignals, compute_intent_signals, intent_centroids,
    semantic_dropout_floor, should_prefer_lexical_search,
};
use crate::opensearch::product_document::ProductDocument;
use crate::opensearch::repository::{HYBRID_BM25_QUERY_NAME, ProductOpenSearchRepository};
use common::language::domain::Language;
use common::opensearch::search_response::{OpenSearchTimedOutError, SearchHit, SearchResponse};
use common::pagination::cursor::{Cursor, CursoredResult};
use common::sort::{Sort, SortOrder};
use std::collections::HashSet;

const DEFAULT_PAGE_SIZE: u64 = 20;
const HYBRID_BM25_PROBE_SIZE: u64 = 8;
const HYBRID_MAX_SCAN_MULTIPLIER: u64 = 5;

struct HybridFilterContext<'a> {
    query_text: &'a str,
    embedding: &'a [f32],
    languages: &'a [Language],
    params: HybridSearchParams,
    min_semantic_cosine: f32,
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
/// exploratory queries continue to use the native OpenSearch hybrid query, but low-quality
/// vector-only tail hits are dropped before the page is returned to the caller.
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
    let items = fetch_filtered_hybrid_page(repository, search, page, &filter_context).await?;

    Ok(HybridSearchOutcome {
        items,
        intent,
        params,
    })
}

async fn fetch_filtered_hybrid_page(
    repository: &(dyn ProductOpenSearchRepository + Sync),
    search: &ProductSearch,
    page: &Option<Cursor<serde_json::Value>>,
    context: &HybridFilterContext<'_>,
) -> Result<CursoredResult<LocalizedProductView, serde_json::Value>, HybridSearchError> {
    let requested_size = requested_page_size(page);
    let max_scanned_hits = requested_size
        .saturating_mul(HYBRID_MAX_SCAN_MULTIPLIER)
        .max(requested_size);

    let mut accepted = Vec::with_capacity(requested_size as usize);
    let mut raw_total = None;
    let mut dropped_any = false;
    let mut fetched_pages = 0u64;
    let mut scanned_hits = 0u64;
    let mut next_search_after = page.as_ref().and_then(|cursor| cursor.search_after.clone());
    let mut last_processed_sort = None;

    loop {
        let current_cursor = Some(Cursor {
            size: requested_size,
            search_after: next_search_after.clone(),
        });
        let response = repository
            .hybrid_search_product_documents(
                search,
                context.embedding,
                context.params,
                &current_cursor,
            )
            .await?
            .into_non_timed_out("product hybrid search")?;
        fetched_pages += 1;

        if raw_total.is_none() {
            raw_total = Some(response.hits.total.value);
        }

        let page_hits = response.hits.hits;
        let page_hit_count = page_hits.len();
        let page_last_sort = page_hits.iter().rev().find_map(|hit| hit.sort.clone());

        if page_hit_count == 0 {
            break;
        }

        for hit in page_hits {
            if hit.sort.is_some() {
                last_processed_sort = hit.sort.clone();
            }
            scanned_hits += 1;

            if should_keep_hybrid_hit(
                search,
                context.query_text,
                context.embedding,
                context.min_semantic_cosine,
                &hit,
            ) {
                accepted
                    .push(Product::from(hit.source).localized(&search.currency, context.languages));
                if accepted.len() as u64 == requested_size {
                    return Ok(CursoredResult {
                        items: accepted,
                        cursor: Cursor {
                            size: requested_size,
                            search_after: last_processed_sort,
                        },
                        total: if dropped_any || fetched_pages > 1 {
                            None
                        } else {
                            raw_total
                        },
                    });
                }
            } else {
                dropped_any = true;
            }

            if scanned_hits >= max_scanned_hits {
                let size = accepted.len() as u64;
                return Ok(CursoredResult {
                    items: accepted,
                    cursor: Cursor {
                        size,
                        search_after: last_processed_sort,
                    },
                    total: None,
                });
            }
        }

        if page_hit_count < requested_size as usize || page_last_sort.is_none() {
            break;
        }
        if page_last_sort == next_search_after {
            break;
        }
        next_search_after = page_last_sort;
    }

    let size = accepted.len() as u64;
    Ok(CursoredResult {
        items: accepted,
        cursor: Cursor {
            size,
            search_after: last_processed_sort,
        },
        total: if dropped_any || fetched_pages > 1 {
            None
        } else {
            raw_total
        },
    })
}

fn should_keep_hybrid_hit(
    search: &ProductSearch,
    query_text: &str,
    query_embedding: &[f32],
    min_semantic_cosine: f32,
    hit: &SearchHit<ProductDocument>,
) -> bool {
    if hit
        .matched_queries
        .iter()
        .any(|name| name == HYBRID_BM25_QUERY_NAME)
    {
        return true;
    }

    if hit_has_text_anchor(search, query_text, &hit.source) {
        return true;
    }

    hit.source
        .embedding
        .as_deref()
        .and_then(|doc_embedding| cosine_similarity(query_embedding, doc_embedding))
        .is_some_and(|cosine| cosine + 1e-6 >= min_semantic_cosine)
}

fn hit_has_text_anchor(search: &ProductSearch, query_text: &str, doc: &ProductDocument) -> bool {
    let mut titles = Vec::with_capacity(2);
    if let Some(title) = localized_title_for_search(search, doc) {
        titles.push(title);
    }
    titles.push(doc.title_native.text.as_str());

    titles
        .into_iter()
        .any(|title| title_has_query_anchor(title, query_text))
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
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|token| {
            !token.is_empty() && (token.len() >= 3 || token.chars().all(|ch| ch.is_ascii_digit()))
        })
        .map(ToOwned::to_owned)
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

    #[tokio::test]
    async fn should_dispatch_native_hybrid_query_for_text_search() {
        let mut repo = MockProductOpenSearchRepository::default();
        let probe = mk_response(vec![mk_hit(
            mk_doc("art deco lamp"),
            1.0,
            json!([1.0]),
            vec![],
        )]);
        let hybrid_response = mk_response(vec![mk_hit(
            mk_doc("art deco lamp"),
            1.0,
            json!([1.0]),
            vec![HYBRID_BM25_QUERY_NAME],
        )]);

        repo.expect_search_product_documents()
            .times(1)
            .return_once(move |_, _, _| Box::pin(async move { Ok(probe) }));
        repo.expect_hybrid_search_product_documents()
            .times(1)
            .return_once(move |_, _, _, _| Box::pin(async move { Ok(hybrid_response) }));

        let search = mk_search();
        let embedding = one_hot_embedding(0);
        let outcome = hybrid_search(&repo, &search, &embedding, &None, &[search.language])
            .await
            .unwrap();
        assert_eq!(outcome.items.items.len(), 1);
        assert!(outcome.params.vector_weight <= 1.0 - HybridSearchParams::MIN_BM25_WEIGHT);
        assert!(outcome.params.candidate_k >= HybridSearchParams::MIN_CANDIDATE_K);
        assert!(outcome.params.candidate_k <= HybridSearchParams::MAX_CANDIDATE_K);
    }

    #[tokio::test]
    async fn should_err_when_bm25_probe_times_out() {
        let mut repo = MockProductOpenSearchRepository::default();
        repo.expect_search_product_documents()
            .times(1)
            .return_once(|_, _, _| {
                Box::pin(async move {
                    Ok(SearchResponse {
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
                    })
                })
            });

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

        let mut search = mk_search();
        search.product_query = Some("Rolex Submariner 1965".try_into().unwrap());
        let embedding = one_hot_embedding(0);
        let outcome = hybrid_search(&repo, &search, &embedding, &None, &[search.language])
            .await
            .unwrap();

        assert_eq!(outcome.items.items.len(), 1);
        assert_eq!(outcome.items.items[0].product_id, exact.product_id);
    }

    #[tokio::test]
    async fn should_propagate_pagination_cursor_from_opensearch_response() {
        let mut repo = MockProductOpenSearchRepository::default();
        let probe = mk_response(vec![mk_hit(
            mk_doc("art deco lamp"),
            1.0,
            json!([1.0]),
            vec![],
        )]);
        let hybrid = mk_response(vec![
            mk_hit(
                mk_doc("art deco lamp"),
                1.0,
                json!([1.0]),
                vec![HYBRID_BM25_QUERY_NAME],
            ),
            mk_hit(
                mk_doc("art deco floor lamp"),
                0.9,
                json!([0.9]),
                vec![HYBRID_BM25_QUERY_NAME],
            ),
        ]);
        repo.expect_search_product_documents()
            .times(1)
            .return_once(move |_, _, _| Box::pin(async move { Ok(probe) }));
        repo.expect_hybrid_search_product_documents()
            .times(1)
            .return_once(move |_, _, _, _| Box::pin(async move { Ok(hybrid) }));

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
        assert!(outcome.items.cursor.search_after.is_some());
    }

    #[tokio::test]
    async fn should_scan_additional_hybrid_pages_when_initial_page_contains_dropouts() {
        let mut repo = MockProductOpenSearchRepository::default();
        let mut seq = Sequence::new();

        let probe = mk_response(vec![
            mk_hit(mk_doc("blue ceramic vase"), 1.0, json!([1.0]), vec![]),
            mk_hit(mk_doc("blue ceramic jar"), 0.99, json!([0.99]), vec![]),
            mk_hit(mk_doc("blue glass vase"), 0.98, json!([0.98]), vec![]),
        ]);
        repo.expect_search_product_documents()
            .times(1)
            .in_sequence(&mut seq)
            .return_once(move |_, _, _| Box::pin(async move { Ok(probe) }));

        let bm25_hit = mk_hit(
            mk_doc("blue ceramic vase"),
            1.0,
            json!([1.0]),
            vec![HYBRID_BM25_QUERY_NAME],
        );
        let mut junk_doc = mk_doc("totally unrelated text");
        junk_doc.embedding = Some(one_hot_embedding(1));
        let junk_hit = mk_hit(junk_doc, 0.9, json!([0.9]), vec![]);
        let page_one = mk_response(vec![bm25_hit, junk_hit]);

        let mut good_vector_doc = mk_doc("totally unrelated text");
        good_vector_doc.embedding = Some(one_hot_embedding(0));
        let page_two = mk_response(vec![mk_hit(good_vector_doc, 0.8, json!([0.8]), vec![])]);

        repo.expect_hybrid_search_product_documents()
            .times(1)
            .in_sequence(&mut seq)
            .return_once(move |_, _, _, cursor| {
                assert!(
                    cursor
                        .as_ref()
                        .and_then(|page| page.search_after.clone())
                        .is_none()
                );
                Box::pin(async move { Ok(page_one) })
            });
        repo.expect_hybrid_search_product_documents()
            .times(1)
            .in_sequence(&mut seq)
            .return_once(move |_, _, _, cursor| {
                assert_eq!(
                    cursor.as_ref().and_then(|page| page.search_after.clone()),
                    Some(json!([0.9]))
                );
                Box::pin(async move { Ok(page_two) })
            });

        let mut search = mk_search();
        search.product_query = Some("blue ceramic vase".try_into().unwrap());
        let outcome = hybrid_search(
            &repo,
            &search,
            &one_hot_embedding(0),
            &Some(Cursor {
                size: 2,
                search_after: None,
            }),
            &[search.language],
        )
        .await
        .unwrap();

        assert_eq!(outcome.items.items.len(), 2);
        assert_eq!(outcome.items.cursor.search_after, Some(json!([0.8])));
    }
}
