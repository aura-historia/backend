use google_cloud_auth::credentials::AccessTokenCredentials;
use image_fetcher::{FetchedImage, ImageFetcher};
use lru::LruCache;
use serde::{Deserialize, Serialize};
use std::{num::NonZeroUsize, sync::Arc, time::Duration};
use tokio::sync::Mutex;
use url::Url;

const GEMINI_EMBEDDING_MODEL: &str = "gemini-embedding-2";
const QUERY_CACHE_CAPACITY: usize = 4096;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// Product embeddings run in the background worker, after its separately bounded image fetch.
const PRODUCT_EMBEDDING_DEADLINE: Duration = Duration::from_secs(30);
pub const EMBEDDING_DIMENSIONS: usize = 768;

/// Upper bound for one uncached search-query embedding, from credential acquisition to the
/// decoded response body.
///
/// Query embeddings serve interactive requests. The bound stays well below the shortest public
/// request budget so callers can still answer without the embedding, for example by falling
/// back to lexical search on an initial search page.
pub const SEARCH_QUERY_EMBEDDING_DEADLINE: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VertexAiEmbeddingConfig {
    project_id: String,
    location: String,
}

impl VertexAiEmbeddingConfig {
    pub fn new(project_id: impl Into<String>, location: impl Into<String>) -> Self {
        Self {
            project_id: project_id.into(),
            location: location.into(),
        }
    }

    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    pub fn location(&self) -> &str {
        &self.location
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EmbeddingError {
    #[error("embedding input is invalid: {reason}")]
    InvalidInput { reason: &'static str },
    #[error("embedding HTTP client could not be initialized")]
    ClientInitializationFailed {
        #[source]
        source: reqwest::Error,
    },
    #[error("embedding authentication failed")]
    AuthenticationFailed {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("embedding request failed")]
    RequestFailed {
        #[source]
        source: reqwest::Error,
    },
    #[error("embedding request did not complete within {deadline:?}")]
    DeadlineExceeded { deadline: Duration },
    #[error("embedding provider returned HTTP {status}")]
    ApiFailure { status: reqwest::StatusCode },
    #[error("embedding provider response is invalid: {reason}")]
    InvalidResponse { reason: &'static str },
}

impl EmbeddingError {
    /// An allowlisted label for terminal diagnostics. Unlike the error text, it never includes
    /// request URLs, credentials or provider response content.
    pub fn diagnostic_category(&self) -> &'static str {
        match self {
            Self::InvalidInput { .. } => "input_invalid",
            Self::ClientInitializationFailed { .. } => "configuration",
            Self::AuthenticationFailed { .. } => "authentication",
            Self::DeadlineExceeded { .. } => "timeout",
            Self::RequestFailed { source } if source.is_timeout() => "timeout",
            Self::RequestFailed { source } if source.is_decode() => "response_invalid",
            Self::RequestFailed { .. } => "request",
            Self::ApiFailure { .. } => "http_status",
            Self::InvalidResponse { .. } => "response_invalid",
        }
    }

    pub fn http_status(&self) -> Option<u16> {
        match self {
            Self::ApiFailure { status } => Some(status.as_u16()),
            Self::RequestFailed { source } => source.status().map(|status| status.as_u16()),
            Self::InvalidInput { .. }
            | Self::ClientInitializationFailed { .. }
            | Self::AuthenticationFailed { .. }
            | Self::DeadlineExceeded { .. }
            | Self::InvalidResponse { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EmbeddingText(String);

impl EmbeddingText {
    pub fn new(value: impl Into<String>) -> Result<Self, EmbeddingError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(EmbeddingError::InvalidInput {
                reason: "embedding text is empty",
            });
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingImageUrl(Url);

impl EmbeddingImageUrl {
    pub fn new(url: Url) -> Result<Self, EmbeddingError> {
        if matches!(url.scheme(), "http" | "https") {
            Ok(Self(url))
        } else {
            Err(EmbeddingError::InvalidInput {
                reason: "embedding image URL must use HTTP or HTTPS",
            })
        }
    }

    pub fn as_url(&self) -> &Url {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddingVector(Vec<f32>);

impl EmbeddingVector {
    pub fn try_new(mut values: Vec<f32>) -> Result<Self, EmbeddingError> {
        normalize_embedding(&mut values)?;
        Ok(Self(values))
    }

    pub fn values(&self) -> &[f32] {
        &self.0
    }

    pub fn into_values(self) -> Vec<f32> {
        self.0
    }
}

#[async_trait::async_trait]
pub trait EmbeddingGenerator: Send + Sync {
    async fn embed_product(
        &self,
        title: &EmbeddingText,
        additional_text: Option<&EmbeddingText>,
        image_url: Option<&EmbeddingImageUrl>,
    ) -> Result<EmbeddingVector, EmbeddingError>;

    /// Serves interactive requests: implementations must return, with an error if necessary,
    /// within [`SEARCH_QUERY_EMBEDDING_DEADLINE`] so callers can still answer without it.
    async fn embed_search_query(
        &self,
        query: &EmbeddingText,
    ) -> Result<EmbeddingVector, EmbeddingError>;
}

#[async_trait::async_trait]
impl<G> EmbeddingGenerator for Arc<G>
where
    G: EmbeddingGenerator + ?Sized,
{
    async fn embed_product(
        &self,
        title: &EmbeddingText,
        additional_text: Option<&EmbeddingText>,
        image_url: Option<&EmbeddingImageUrl>,
    ) -> Result<EmbeddingVector, EmbeddingError> {
        self.as_ref()
            .embed_product(title, additional_text, image_url)
            .await
    }

    async fn embed_search_query(
        &self,
        query: &EmbeddingText,
    ) -> Result<EmbeddingVector, EmbeddingError> {
        self.as_ref().embed_search_query(query).await
    }
}

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
        self.image_fetcher.fetch(image_url.as_url()).await
    }
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
    let endpoint = match config.location() {
        "us" | "eu" => format!(
            "https://aiplatform.{}.rep.googleapis.com",
            config.location()
        ),
        "global" => "https://aiplatform.googleapis.com".to_owned(),
        location => format!("https://{location}-aiplatform.googleapis.com"),
    };
    format!(
        "{endpoint}/v1/projects/{}/locations/{}/publishers/google/models/{GEMINI_EMBEDDING_MODEL}:embedContent",
        config.project_id(),
        config.location(),
    )
}

fn normalize_embedding(values: &mut [f32]) -> Result<(), EmbeddingError> {
    if values.len() != EMBEDDING_DIMENSIONS {
        return Err(EmbeddingError::InvalidResponse {
            reason: "embedding has an unexpected dimension",
        });
    }
    if values.iter().any(|value| !value.is_finite()) {
        return Err(EmbeddingError::InvalidResponse {
            reason: "embedding contains a non-finite value",
        });
    }
    let norm = values.iter().map(|value| value * value).sum::<f32>().sqrt();
    if !norm.is_finite() || norm == 0.0 {
        return Err(EmbeddingError::InvalidResponse {
            reason: "embedding has zero norm",
        });
    }
    for value in values {
        *value /= norm;
    }
    Ok(())
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
        let additional_text = additional_text.map(EmbeddingText::as_str).unwrap_or("none");
        let mut parts = vec![ContentPart::Text {
            text: format!("title: {} | text: {additional_text}", title.as_str()),
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
struct EmbedContentResponse {
    #[serde(default)]
    embedding: Option<ProviderEmbedding>,
    #[serde(default)]
    embeddings: Vec<ProviderEmbedding>,
}

impl EmbedContentResponse {
    fn into_embedding_vector(self) -> Result<EmbeddingVector, EmbeddingError> {
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
    use google_cloud_auth::credentials::{
        AccessToken, AccessTokenCredentialsProvider, CacheableResource, CredentialsProvider,
    };
    use google_cloud_auth::errors::CredentialsError;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
    };

    const TEST_SEARCH_QUERY_DEADLINE: Duration = Duration::from_millis(500);
    const TEST_PRODUCT_DEADLINE: Duration = Duration::from_millis(1000);
    const PROVIDER_STALL: Duration = Duration::from_secs(30);
    /// Fails an unbounded embedding instead of hanging the test on the stalled fixture.
    const TEST_GUARD: Duration = Duration::from_secs(5);

    #[derive(Debug)]
    enum TestCredentials {
        Issued,
        Stalled,
    }

    impl CredentialsProvider for TestCredentials {
        async fn headers(
            &self,
            _: http::Extensions,
        ) -> Result<CacheableResource<http::HeaderMap>, CredentialsError> {
            Err(CredentialsError::from_msg(
                false,
                "test credentials issue access tokens only",
            ))
        }

        async fn universe_domain(&self) -> Option<String> {
            None
        }
    }

    impl AccessTokenCredentialsProvider for TestCredentials {
        async fn access_token(&self) -> Result<AccessToken, CredentialsError> {
            match self {
                Self::Issued => Ok(AccessToken {
                    token: "test-access-token".to_owned(),
                }),
                Self::Stalled => std::future::pending().await,
            }
        }
    }

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
    fn should_classify_provider_failures_by_variant() {
        for (error, category, status) in [
            (
                EmbeddingError::AuthenticationFailed {
                    source: Box::new(std::io::Error::other("token refresh failed")),
                },
                "authentication",
                None,
            ),
            (
                EmbeddingError::ApiFailure {
                    status: reqwest::StatusCode::TOO_MANY_REQUESTS,
                },
                "http_status",
                Some(429),
            ),
            (
                EmbeddingError::InvalidResponse {
                    reason: "missing embedding values",
                },
                "response_invalid",
                None,
            ),
        ] {
            assert_eq!(category, error.diagnostic_category());
            assert_eq!(status, error.http_status());
        }
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

    #[test]
    fn should_reject_invalid_embedding_vectors() {
        assert!(EmbeddingVector::try_new(vec![1.0; EMBEDDING_DIMENSIONS - 1]).is_err());
        assert!(EmbeddingVector::try_new(vec![0.0; EMBEDDING_DIMENSIONS]).is_err());
        let mut non_finite = vec![1.0; EMBEDDING_DIMENSIONS];
        non_finite[0] = f32::NAN;
        assert!(EmbeddingVector::try_new(non_finite).is_err());
    }
}
