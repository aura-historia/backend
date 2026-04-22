use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use thiserror::Error;
use tracing::debug;

#[derive(Debug, Error)]
pub enum QueryEmbeddingError {
    #[error("Gemini API request failed: {0}")]
    RequestFailed(#[from] reqwest::Error),
    #[error("Gemini API returned an empty embedding")]
    EmptyResponse,
}

/// Service that embeds a free-text product search query into a dense vector.
///
/// Conceptually a sibling of `MultimodalEmbeddingService` from `product-pipeline-embed-text`,
/// but specialised for *queries* (uses the `RETRIEVAL_QUERY` task type per
/// <https://ai.google.dev/gemini-api/docs/embeddings#task-types-embeddings-2>) and usable from
/// the `product` crate without pulling in the pipeline binary's heavy dependency surface.
#[async_trait]
#[mockall::automock]
pub trait QueryEmbeddingService: Send + Sync {
    async fn embed_query(&self, query: &str) -> Result<Vec<f32>, QueryEmbeddingError>;
}

pub struct GeminiQueryEmbeddingService {
    api_key: String,
    client: reqwest::Client,
}

impl GeminiQueryEmbeddingService {
    pub fn new(api_key: &str) -> Self {
        Self {
            api_key: api_key.to_string(),
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl QueryEmbeddingService for GeminiQueryEmbeddingService {
    async fn embed_query(&self, query: &str) -> Result<Vec<f32>, QueryEmbeddingError> {
        debug!("Requesting query embedding from Gemini API.");
        let request = EmbedContentRequest {
            model: "models/gemini-embedding-2-preview-03-25",
            content: Content {
                parts: vec![ContentPart::Text {
                    text: query.to_string(),
                }],
            },
            // Per Gemini guidance, queries are embedded with the RETRIEVAL_QUERY task type
            // so that they sit in the same vector space as documents embedded with
            // RETRIEVAL_DOCUMENT (or its multimodal equivalent used by the ingestion pipeline).
            task_type: "RETRIEVAL_QUERY",
        };

        let response = self
            .client
            .post(
                "https://generativelanguage.googleapis.com/v1beta/models/\
                 gemini-embedding-2-preview:embedContent",
            )
            .header("x-goog-api-key", &self.api_key)
            .query(&[("output_dimensionality", "768")])
            .json(&request)
            .send()
            .await?
            .error_for_status()?;

        let body: EmbedContentResponse = response.json().await?;
        let mut values = body.embedding.values;
        if values.is_empty() {
            return Err(QueryEmbeddingError::EmptyResponse);
        }
        // Normalise to unit length so cosine similarity equals dot product, mirroring the
        // ingestion-time normalisation done by `MultimodalEmbeddingServiceImpl`.
        let norm: f32 = values.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm == 0.0 {
            return Err(QueryEmbeddingError::EmptyResponse);
        }
        for v in &mut values {
            *v /= norm;
        }
        Ok(values)
    }
}

/// Lightweight TTL cache decorator around any [`QueryEmbeddingService`].
///
/// Keeps the Lambda lightweight: at most `capacity` entries are retained and each entry
/// expires after `ttl`. This is intended for short-lived in-memory caching across the
/// hot warm-period of a Lambda container, where the same query is frequently re-embedded
/// (e.g. paginated requests).
pub struct CachedQueryEmbeddingService<S> {
    inner: S,
    state: Mutex<CacheState>,
    ttl: Duration,
    capacity: usize,
}

struct CacheState {
    entries: HashMap<String, CacheEntry>,
}

struct CacheEntry {
    inserted_at: Instant,
    embedding: Vec<f32>,
}

impl<S> CachedQueryEmbeddingService<S> {
    pub fn new(inner: S, ttl: Duration, capacity: usize) -> Self {
        Self {
            inner,
            state: Mutex::new(CacheState {
                entries: HashMap::with_capacity(capacity.min(64)),
            }),
            ttl,
            capacity,
        }
    }

    fn get_fresh(&self, query: &str) -> Option<Vec<f32>> {
        let state = self.state.lock().ok()?;
        let entry = state.entries.get(query)?;
        if entry.inserted_at.elapsed() <= self.ttl {
            Some(entry.embedding.clone())
        } else {
            None
        }
    }

    fn insert(&self, query: String, embedding: Vec<f32>) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        // Evict expired entries first.
        let ttl = self.ttl;
        state.entries.retain(|_, e| e.inserted_at.elapsed() <= ttl);
        // If still over capacity, drop the oldest entry to bound memory.
        if state.entries.len() >= self.capacity
            && let Some(oldest_key) = state
                .entries
                .iter()
                .min_by_key(|(_, e)| e.inserted_at)
                .map(|(k, _)| k.clone())
        {
            state.entries.remove(&oldest_key);
        }
        state.entries.insert(
            query,
            CacheEntry {
                inserted_at: Instant::now(),
                embedding,
            },
        );
    }
}

#[async_trait]
impl<S: QueryEmbeddingService + Send + Sync> QueryEmbeddingService
    for CachedQueryEmbeddingService<S>
{
    async fn embed_query(&self, query: &str) -> Result<Vec<f32>, QueryEmbeddingError> {
        if let Some(cached) = self.get_fresh(query) {
            return Ok(cached);
        }
        let embedding = self.inner.embed_query(query).await?;
        self.insert(query.to_string(), embedding.clone());
        Ok(embedding)
    }
}

// ---- Gemini wire types ----

#[derive(Debug, Serialize)]
struct EmbedContentRequest<'a> {
    model: &'a str,
    content: Content,
    #[serde(rename = "taskType")]
    task_type: &'a str,
}

#[derive(Debug, Serialize)]
struct Content {
    parts: Vec<ContentPart>,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum ContentPart {
    Text { text: String },
}

#[derive(Debug, Deserialize)]
struct EmbedContentResponse {
    embedding: Embedding,
}

#[derive(Debug, Deserialize)]
struct Embedding {
    values: Vec<f32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    struct StubService {
        calls: Mutex<u32>,
    }

    #[async_trait]
    impl QueryEmbeddingService for StubService {
        async fn embed_query(&self, query: &str) -> Result<Vec<f32>, QueryEmbeddingError> {
            *self.calls.lock().unwrap() += 1;
            Ok(vec![query.len() as f32; 4])
        }
    }

    #[tokio::test]
    async fn should_cache_embedding_when_called_twice_for_same_query() {
        let stub = StubService {
            calls: Mutex::new(0),
        };
        let cached = CachedQueryEmbeddingService::new(stub, Duration::from_secs(60), 8);

        let first = cached.embed_query("antique chair").await.unwrap();
        let second = cached.embed_query("antique chair").await.unwrap();

        assert_eq!(first, second);
        assert_eq!(*cached.inner.calls.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn should_recompute_embedding_when_ttl_expired() {
        let stub = StubService {
            calls: Mutex::new(0),
        };
        let cached = CachedQueryEmbeddingService::new(stub, Duration::from_millis(1), 8);
        let _ = cached.embed_query("vase").await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        let _ = cached.embed_query("vase").await.unwrap();
        assert_eq!(*cached.inner.calls.lock().unwrap(), 2);
    }

    #[tokio::test]
    async fn should_evict_oldest_entry_when_capacity_exceeded() {
        let stub = StubService {
            calls: Mutex::new(0),
        };
        let cached = CachedQueryEmbeddingService::new(stub, Duration::from_secs(60), 2);
        let _ = cached.embed_query("a").await.unwrap();
        let _ = cached.embed_query("bb").await.unwrap();
        let _ = cached.embed_query("ccc").await.unwrap();

        // "a" should have been evicted to keep size bounded.
        let state = cached.state.lock().unwrap();
        assert_eq!(state.entries.len(), 2);
        assert!(!state.entries.contains_key("a"));
    }
}
