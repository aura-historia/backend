//! The synchronous Vertex AI Gemini Embedding adapter.

use crate::{
    EMBEDDING_DIMENSIONS, EmbeddingError, EmbeddingGenerator, EmbeddingImageUrl, EmbeddingText,
    EmbeddingVector, SEARCH_QUERY_EMBEDDING_DEADLINE, VertexAiEmbeddingConfig,
};
use google_cloud_auth::credentials::AccessTokenCredentials;
use image_fetcher::{FetchedImage, ImageFetcher};
use lru::LruCache;
use serde::{Deserialize, Serialize};
use std::{num::NonZeroUsize, time::Duration};
use tokio::sync::Mutex;

pub(crate) const GEMINI_EMBEDDING_MODEL_RESOURCE: &str =
    "publishers/google/models/gemini-embedding-2";
const QUERY_CACHE_CAPACITY: usize = 4096;
pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// Product embeddings run in the background worker, after its separately bounded image fetch.
const PRODUCT_EMBEDDING_DEADLINE: Duration = Duration::from_secs(30);

/// Vertex AI Gemini Embedding implementation of [`EmbeddingGenerator`].
///
/// Callers supply semantic product or search fields. This adapter owns Google prompt
/// format, provider protocol, image retrieval, response validation, query cache, and
/// deadlines. Each provider exchange, including credential acquisition and the response body,
/// is bounded by [`SEARCH_QUERY_EMBEDDING_DEADLINE`] for search queries and by a longer
/// background deadline for products; expiry yields [`EmbeddingError::DeadlineExceeded`].
pub struct VertexAiEmbeddingGenerator {
    embed_content_url: String,
    client: reqwest::Client,
    image_fetcher: ImageFetcher,
    credentials: AccessTokenCredentials,
    query_cache: Mutex<LruCache<EmbeddingText, EmbeddingVector>>,
    search_query_deadline: Duration,
    product_deadline: Duration,
}

impl VertexAiEmbeddingGenerator {
    pub fn new(
        config: VertexAiEmbeddingConfig,
        credentials: AccessTokenCredentials,
    ) -> Result<Self, EmbeddingError> {
        let client = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .map_err(|source| EmbeddingError::ClientInitializationFailed { source })?;
        Ok(Self {
            embed_content_url: build_embed_content_url(&config),
            client,
            image_fetcher: ImageFetcher::new(),
            credentials,
            query_cache: Mutex::new(LruCache::new(
                NonZeroUsize::new(QUERY_CACHE_CAPACITY).unwrap_or(NonZeroUsize::MIN),
            )),
            search_query_deadline: SEARCH_QUERY_EMBEDDING_DEADLINE,
            product_deadline: PRODUCT_EMBEDDING_DEADLINE,
        })
    }

    async fn embed_search_query(
        &self,
        query: &EmbeddingText,
    ) -> Result<EmbeddingVector, EmbeddingError> {
        if let Some(vector) = self.query_cache.lock().await.get(query).cloned() {
            return Ok(vector);
        }

        let vector = self
            .request_embedding(
                EmbedContentRequest::for_search_query(query),
                self.search_query_deadline,
            )
            .await?;
        self.query_cache
            .lock()
            .await
            .put(query.clone(), vector.clone());
        Ok(vector)
    }

    async fn embed_product(
        &self,
        title: &EmbeddingText,
        additional_text: Option<&EmbeddingText>,
        image_url: Option<&EmbeddingImageUrl>,
    ) -> Result<EmbeddingVector, EmbeddingError> {
        let image = match image_url {
            Some(image_url) => self.fetch_image(image_url).await,
            None => None,
        };
        self.request_embedding(
            EmbedContentRequest::for_product(title, additional_text, image),
            self.product_deadline,
        )
        .await
    }

    async fn request_embedding(
        &self,
        request: EmbedContentRequest,
        deadline: Duration,
    ) -> Result<EmbeddingVector, EmbeddingError> {
        tokio::time::timeout(deadline, self.call_embed_content(request))
            .await
            .map_err(|_| EmbeddingError::DeadlineExceeded { deadline })?
    }

    async fn call_embed_content(
        &self,
        request: EmbedContentRequest,
    ) -> Result<EmbeddingVector, EmbeddingError> {
        let access_token = self.credentials.access_token().await.map_err(|source| {
            EmbeddingError::AuthenticationFailed {
                source: Box::new(source),
            }
        })?;
        let response = self
            .client
            .post(&self.embed_content_url)
            .bearer_auth(access_token.token)
            .json(&request)
            .send()
            .await
            .map_err(|source| EmbeddingError::RequestFailed { source })?;

        if !response.status().is_success() {
            return Err(EmbeddingError::ApiFailure {
                status: response.status(),
            });
        }

        response
            .json::<EmbedContentResponse>()
            .await
            .map_err(|source| EmbeddingError::RequestFailed { source })?
            .into_embedding_vector()
    }

    async fn fetch_image(&self, image_url: &EmbeddingImageUrl) -> Option<FetchedImage> {
        fetch_image(&self.image_fetcher, image_url).await
    }
}

/// An image that cannot be fetched is left out of the embedding; its bounded cause is logged.
pub(crate) async fn fetch_image(
    fetcher: &ImageFetcher,
    image_url: &EmbeddingImageUrl,
) -> Option<FetchedImage> {
    fetcher
        .fetch(image_url.as_url())
        .await
        .inspect_err(|error| {
            let fetch_failure = error.diagnostics();
            tracing::info!(
                eventType = "EMBEDDING_IMAGE_OMITTED",
                imageOmissionReason = "fetch_failed",
                imageFetchFailure = fetch_failure.category,
                imageFetchHttpStatus = fetch_failure.http_status,
                imageFetchAttempts = fetch_failure.attempts,
                "Embedding image omitted."
            );
        })
        .ok()
}

#[async_trait::async_trait]
impl EmbeddingGenerator for VertexAiEmbeddingGenerator {
    async fn embed_product(
        &self,
        title: &EmbeddingText,
        additional_text: Option<&EmbeddingText>,
        image_url: Option<&EmbeddingImageUrl>,
    ) -> Result<EmbeddingVector, EmbeddingError> {
        self.embed_product(title, additional_text, image_url).await
    }

    async fn embed_search_query(
        &self,
        query: &EmbeddingText,
    ) -> Result<EmbeddingVector, EmbeddingError> {
        self.embed_search_query(query).await
    }
}

fn build_embed_content_url(config: &VertexAiEmbeddingConfig) -> String {
    format!(
        "{}/v1/{}/{GEMINI_EMBEDDING_MODEL_RESOURCE}:embedContent",
        api_endpoint(config),
        location_resource(config),
    )
}

/// The Vertex AI host serving `config`'s location: the `us` and `eu` multi-regions have regional
/// endpoints of their own.
pub(crate) fn api_endpoint(config: &VertexAiEmbeddingConfig) -> String {
    match config.location() {
        "us" | "eu" => format!(
            "https://aiplatform.{}.rep.googleapis.com",
            config.location()
        ),
        "global" => "https://aiplatform.googleapis.com".to_owned(),
        location => format!("https://{location}-aiplatform.googleapis.com"),
    }
}

/// `projects/<project>/locations/<location>`, the parent of every Vertex AI resource used here.
pub(crate) fn location_resource(config: &VertexAiEmbeddingConfig) -> String {
    format!(
        "projects/{}/locations/{}",
        config.project_id(),
        config.location()
    )
}

/// The product document text. Batch and synchronous requests share it, so both embed a listing
/// identically.
pub(crate) fn product_text(
    title: &EmbeddingText,
    additional_text: Option<&EmbeddingText>,
) -> String {
    let additional_text = additional_text.map(EmbeddingText::as_str).unwrap_or("none");
    format!("title: {} | text: {additional_text}", title.as_str())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EmbedContentRequest {
    content: Content,
    output_dimensionality: usize,
}

impl EmbedContentRequest {
    fn for_search_query(query: &EmbeddingText) -> Self {
        Self::for_parts(vec![ContentPart::Text {
            text: format!("task: search result | query: {}", query.as_str()),
        }])
    }

    fn for_product(
        title: &EmbeddingText,
        additional_text: Option<&EmbeddingText>,
        image: Option<FetchedImage>,
    ) -> Self {
        let mut parts = vec![ContentPart::Text {
            text: product_text(title, additional_text),
        }];
        if let Some(image) = image {
            parts.push(ContentPart::InlineData {
                inline_data: InlineData {
                    mime_type: image.mime_type(),
                    data: image.base64_data().to_owned(),
                },
            });
        }
        Self::for_parts(parts)
    }

    fn for_parts(parts: Vec<ContentPart>) -> Self {
        Self {
            content: Content { parts },
            output_dimensionality: EMBEDDING_DIMENSIONS,
        }
    }
}

#[derive(Debug, Serialize)]
struct Content {
    parts: Vec<ContentPart>,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum ContentPart {
    Text {
        text: String,
    },
    InlineData {
        #[serde(rename = "inlineData")]
        inline_data: InlineData,
    },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct InlineData {
    mime_type: &'static str,
    data: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct EmbedContentResponse {
    #[serde(default)]
    embedding: Option<ProviderEmbedding>,
    #[serde(default)]
    embeddings: Vec<ProviderEmbedding>,
}

impl EmbedContentResponse {
    pub(crate) fn into_embedding_vector(self) -> Result<EmbeddingVector, EmbeddingError> {
        let values = self
            .embedding
            .or_else(|| self.embeddings.into_iter().next())
            .ok_or(EmbeddingError::InvalidResponse {
                reason: "embedding is missing",
            })?
            .values;
        EmbeddingVector::try_new(values)
    }
}

#[derive(Debug, Deserialize)]
struct ProviderEmbedding {
    values: Vec<f32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestCredentials;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
    };

    const TEST_SEARCH_QUERY_DEADLINE: Duration = Duration::from_millis(500);
    const TEST_PRODUCT_DEADLINE: Duration = Duration::from_millis(1000);
    const PROVIDER_STALL: Duration = Duration::from_secs(30);
    /// Fails an unbounded embedding instead of hanging the test on the stalled fixture.
    const TEST_GUARD: Duration = Duration::from_secs(5);

    #[derive(Debug, Clone, Copy)]
    enum ProviderBehavior {
        Respond,
        StallHeaders,
        StallBody,
    }

    /// Local Vertex stand-in counting the requests it has read completely.
    struct LocalProvider {
        url: String,
        requests: Arc<AtomicUsize>,
    }

    impl LocalProvider {
        async fn start(behavior: ProviderBehavior) -> std::io::Result<Self> {
            let listener = TcpListener::bind("127.0.0.1:0").await?;
            let url = format!("http://{}/v1/embedContent", listener.local_addr()?);
            let requests = Arc::new(AtomicUsize::new(0));
            let served = Arc::clone(&requests);
            tokio::spawn(async move {
                while let Ok((stream, _)) = listener.accept().await {
                    let served = Arc::clone(&served);
                    tokio::spawn(async move {
                        let _ = respond(stream, behavior, &served).await;
                    });
                }
            });
            Ok(Self { url, requests })
        }

        fn requests(&self) -> usize {
            self.requests.load(Ordering::SeqCst)
        }
    }

    async fn respond(
        mut stream: TcpStream,
        behavior: ProviderBehavior,
        requests: &AtomicUsize,
    ) -> std::io::Result<()> {
        read_request(&mut stream).await?;
        requests.fetch_add(1, Ordering::SeqCst);
        let body =
            serde_json::json!({"embedding": {"values": vec![2.0_f32; EMBEDDING_DIMENSIONS]}})
                .to_string();
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        if matches!(behavior, ProviderBehavior::StallHeaders) {
            tokio::time::sleep(PROVIDER_STALL).await;
        }
        stream.write_all(head.as_bytes()).await?;
        stream.flush().await?;
        if matches!(behavior, ProviderBehavior::StallBody) {
            tokio::time::sleep(PROVIDER_STALL).await;
        }
        stream.write_all(body.as_bytes()).await
    }

    async fn read_request(stream: &mut TcpStream) -> std::io::Result<()> {
        let mut bytes = Vec::new();
        let mut buffer = [0; 4096];
        loop {
            let count = stream.read(&mut buffer).await?;
            if count == 0 {
                return Err(std::io::ErrorKind::UnexpectedEof.into());
            }
            bytes.extend_from_slice(&buffer[..count]);
            let Some(header_end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") else {
                continue;
            };
            let content_length = String::from_utf8_lossy(&bytes[..header_end])
                .to_ascii_lowercase()
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .and_then(|length| length.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if bytes.len() >= header_end + 4 + content_length {
                return Ok(());
            }
        }
    }

    fn vertex_generator(
        provider: &LocalProvider,
        credentials: TestCredentials,
    ) -> Result<VertexAiEmbeddingGenerator, Box<dyn std::error::Error>> {
        let mut generator = VertexAiEmbeddingGenerator::new(
            VertexAiEmbeddingConfig::new("test-project", "eu"),
            AccessTokenCredentials::from(credentials),
        )?;
        // Reach the loopback fixture even when the developer environment configures a proxy.
        generator.client = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .no_proxy()
            .build()?;
        generator.embed_content_url = provider.url.clone();
        generator.search_query_deadline = TEST_SEARCH_QUERY_DEADLINE;
        generator.product_deadline = TEST_PRODUCT_DEADLINE;
        Ok(generator)
    }

    async fn assert_deadline_exceeded(
        embedding: impl Future<Output = Result<EmbeddingVector, EmbeddingError>>,
        expected_deadline: Duration,
    ) {
        let result = tokio::time::timeout(TEST_GUARD, embedding).await;

        assert!(
            matches!(
                result,
                Ok(Err(EmbeddingError::DeadlineExceeded { deadline })) if deadline == expected_deadline
            ),
            "expected the {expected_deadline:?} deadline to expire, got {result:?}"
        );
    }

    #[tokio::test]
    async fn should_bound_query_and_product_embeddings_when_provider_stalls_headers_or_body()
    -> Result<(), Box<dyn std::error::Error>> {
        for behavior in [ProviderBehavior::StallHeaders, ProviderBehavior::StallBody] {
            let provider = LocalProvider::start(behavior).await?;
            let generator = vertex_generator(&provider, TestCredentials::Issued)?;
            let text = EmbeddingText::new("vintage brass lamp")?;

            assert_deadline_exceeded(
                generator.embed_search_query(&text),
                TEST_SEARCH_QUERY_DEADLINE,
            )
            .await;
            assert_deadline_exceeded(
                generator.embed_product(&text, None, None),
                TEST_PRODUCT_DEADLINE,
            )
            .await;

            assert_eq!(2, provider.requests(), "{behavior:?}");
            assert!(generator.query_cache.lock().await.is_empty());
        }
        Ok(())
    }

    #[tokio::test]
    async fn should_bound_credential_acquisition_without_contacting_the_provider()
    -> Result<(), Box<dyn std::error::Error>> {
        let provider = LocalProvider::start(ProviderBehavior::Respond).await?;
        let generator = vertex_generator(&provider, TestCredentials::Stalled)?;

        assert_deadline_exceeded(
            generator.embed_search_query(&EmbeddingText::new("vintage brass lamp")?),
            TEST_SEARCH_QUERY_DEADLINE,
        )
        .await;

        assert_eq!(0, provider.requests());
        Ok(())
    }

    #[tokio::test]
    async fn should_serve_repeated_search_queries_from_the_cache_after_one_provider_request()
    -> Result<(), Box<dyn std::error::Error>> {
        let provider = LocalProvider::start(ProviderBehavior::Respond).await?;
        let generator = vertex_generator(&provider, TestCredentials::Issued)?;
        let query = EmbeddingText::new("vintage brass lamp")?;

        let first = generator.embed_search_query(&query).await?;
        let second = generator.embed_search_query(&query).await?;

        assert_eq!(first, second);
        assert_eq!(1, provider.requests());
        Ok(())
    }

    #[test]
    fn should_build_vertex_endpoint_for_regional_and_global_locations() {
        assert_eq!(
            build_embed_content_url(&VertexAiEmbeddingConfig::new("test-project", "eu")),
            "https://aiplatform.eu.rep.googleapis.com/v1/projects/test-project/locations/eu/publishers/google/models/gemini-embedding-2:embedContent"
        );
        assert_eq!(
            build_embed_content_url(&VertexAiEmbeddingConfig::new("test-project", "global")),
            "https://aiplatform.googleapis.com/v1/projects/test-project/locations/global/publishers/google/models/gemini-embedding-2:embedContent"
        );
    }

    #[test]
    fn should_serialize_search_query_vertex_request() -> Result<(), EmbeddingError> {
        let query =
            EmbedContentRequest::for_search_query(&EmbeddingText::new("vintage brass lamp")?);

        assert_eq!(
            serde_json::to_value(query).map_err(|_| EmbeddingError::InvalidResponse {
                reason: "query request serialization failed"
            })?,
            serde_json::json!({"content":{"parts":[{"text":"task: search result | query: vintage brass lamp"}]},"outputDimensionality":768})
        );
        Ok(())
    }

    #[test]
    fn should_serialize_product_vertex_request_with_additional_text_or_none()
    -> Result<(), EmbeddingError> {
        let title = EmbeddingText::new("vintage brass lamp")?;
        let additional_text = EmbeddingText::new("adjustable arm")?;

        assert_eq!(
            serde_json::to_value(EmbedContentRequest::for_product(
                &title,
                Some(&additional_text),
                None,
            ))
            .map_err(|_| EmbeddingError::InvalidResponse {
                reason: "product request serialization failed"
            })?,
            serde_json::json!({"content":{"parts":[{"text":"title: vintage brass lamp | text: adjustable arm"}]},"outputDimensionality":768})
        );
        assert_eq!(
            serde_json::to_value(EmbedContentRequest::for_product(&title, None, None)).map_err(
                |_| EmbeddingError::InvalidResponse {
                    reason: "product request serialization failed"
                }
            )?,
            serde_json::json!({"content":{"parts":[{"text":"title: vintage brass lamp | text: none"}]},"outputDimensionality":768})
        );
        Ok(())
    }

    #[test]
    fn should_validate_and_normalize_both_vertex_response_shapes() -> Result<(), EmbeddingError> {
        for response in [
            serde_json::json!({"embedding":{"values":vec![2.0_f32; EMBEDDING_DIMENSIONS]}}),
            serde_json::json!({"embeddings":[{"values":vec![2.0_f32; EMBEDDING_DIMENSIONS]}]}),
        ] {
            let response =
                serde_json::from_value::<EmbedContentResponse>(response).map_err(|_| {
                    EmbeddingError::InvalidResponse {
                        reason: "response deserialization failed",
                    }
                })?;
            let vector = response.into_embedding_vector()?;
            let norm = vector
                .values()
                .iter()
                .map(|value| value * value)
                .sum::<f32>()
                .sqrt();
            assert!((norm - 1.0).abs() < 0.000_01);
        }
        Ok(())
    }
}
