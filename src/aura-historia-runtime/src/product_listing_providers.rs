//! Reusable listing-provider startup. ADC is initialized only when embeddings are requested.
use crate::{ProductListingConfig, ProductListingDependencies};
use application::error::BoxError;
use async_trait::async_trait;
use embedding::{
    EmbeddingError, EmbeddingGenerator, EmbeddingImageUrl, EmbeddingText, EmbeddingVector,
    VertexAiEmbeddingConfig, VertexAiEmbeddingGenerator,
};
use google_cloud_auth::credentials::{AccessTokenCredentials, Builder as GoogleCredentialsBuilder};
use opensearch::{
    OpenSearch,
    auth::Credentials,
    http::transport::{SingleNodeConnectionPool, TransportBuilder},
};
use std::sync::Arc;
use tokio::sync::OnceCell;

const GOOGLE_CLOUD_PLATFORM_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";

impl ProductListingDependencies {
    pub fn from_env(config: &ProductListingConfig) -> Result<Self, ProductListingProviderError> {
        Ok(Self {
            opensearch: opensearch_client(|name| std::env::var(name).ok())?,
            embeddings: Arc::new(LazyVertexAiEmbeddingGenerator::new(
                config.vertex_ai_embedding().clone(),
            )),
            fx_cache: config.fx_cache(),
            source_cache: config.source_cache(),
            read_execution_policy: config.read_execution_policy(),
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProductListingProviderError {
    #[error("missing required environment variable {name}")]
    MissingEnv { name: &'static str },
    #[error("failed to configure OpenSearch")]
    OpenSearch {
        #[source]
        source: BoxError,
    },
}

fn opensearch_client<F>(mut get: F) -> Result<OpenSearch, ProductListingProviderError>
where
    F: FnMut(&'static str) -> Option<String>,
{
    let mut required = |name| get(name).ok_or(ProductListingProviderError::MissingEnv { name });
    let endpoint = url::Url::parse(&required("OPENSEARCH_ENDPOINT_URL")?).map_err(|source| {
        ProductListingProviderError::OpenSearch {
            source: Box::new(source),
        }
    })?;
    let stage = get("STAGE").unwrap_or_else(|| "prod".to_owned());
    let transport = if stage == "ephemeral" {
        TransportBuilder::new(SingleNodeConnectionPool::new(endpoint)).build()
    } else {
        let mut required = |name| get(name).ok_or(ProductListingProviderError::MissingEnv { name });
        TransportBuilder::new(SingleNodeConnectionPool::new(endpoint))
            .auth(Credentials::Basic(
                required("OPENSEARCH_USERNAME")?,
                required("OPENSEARCH_PASSWORD")?,
            ))
            .build()
    }
    .map_err(|source| ProductListingProviderError::OpenSearch {
        source: Box::new(source),
    })?;
    Ok(OpenSearch::new(transport))
}

trait GoogleAdcProvider: Send + Sync {
    fn credentials(&self) -> Result<AccessTokenCredentials, EmbeddingError>;
}

struct DefaultGoogleAdcProvider;

impl GoogleAdcProvider for DefaultGoogleAdcProvider {
    fn credentials(&self) -> Result<AccessTokenCredentials, EmbeddingError> {
        google_application_default_credentials()
    }
}

struct LazyVertexAiEmbeddingGenerator {
    config: VertexAiEmbeddingConfig,
    adc_provider: Arc<dyn GoogleAdcProvider>,
    generator: OnceCell<VertexAiEmbeddingGenerator>,
}

impl LazyVertexAiEmbeddingGenerator {
    fn new(config: VertexAiEmbeddingConfig) -> Self {
        Self::with_adc_provider(config, Arc::new(DefaultGoogleAdcProvider))
    }

    fn with_adc_provider(
        config: VertexAiEmbeddingConfig,
        adc_provider: Arc<dyn GoogleAdcProvider>,
    ) -> Self {
        Self {
            config,
            adc_provider,
            generator: OnceCell::new(),
        }
    }

    async fn generator(&self) -> Result<&VertexAiEmbeddingGenerator, EmbeddingError> {
        self.generator
            .get_or_try_init(|| async {
                VertexAiEmbeddingGenerator::new(
                    self.config.clone(),
                    self.adc_provider.credentials()?,
                )
            })
            .await
    }
}

#[async_trait]
impl EmbeddingGenerator for LazyVertexAiEmbeddingGenerator {
    async fn embed_product(
        &self,
        title: &EmbeddingText,
        additional_text: Option<&EmbeddingText>,
        image_url: Option<&EmbeddingImageUrl>,
    ) -> Result<EmbeddingVector, EmbeddingError> {
        self.generator()
            .await?
            .embed_product(title, additional_text, image_url)
            .await
    }

    async fn embed_search_query(
        &self,
        query: &EmbeddingText,
    ) -> Result<EmbeddingVector, EmbeddingError> {
        self.generator().await?.embed_search_query(query).await
    }
}

fn google_application_default_credentials() -> Result<AccessTokenCredentials, EmbeddingError> {
    GoogleCredentialsBuilder::default()
        .with_scopes([GOOGLE_CLOUD_PLATFORM_SCOPE])
        .build_access_token_credentials()
        .map_err(|source| EmbeddingError::AuthenticationFailed {
            source: Box::new(source),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[test]
    fn should_require_reader_credentials_outside_the_ephemeral_stage() {
        let result = opensearch_client(|name| match name {
            "OPENSEARCH_ENDPOINT_URL" => Some("https://search.example.test".to_owned()),
            _ => None,
        });
        assert!(matches!(
            result,
            Err(ProductListingProviderError::MissingEnv {
                name: "OPENSEARCH_USERNAME"
            })
        ));
        let result = opensearch_client(|name| match name {
            "OPENSEARCH_ENDPOINT_URL" => Some("https://search.example.test".to_owned()),
            "OPENSEARCH_USERNAME" => Some("fixture-reader".to_owned()),
            _ => None,
        });
        assert!(matches!(
            result,
            Err(ProductListingProviderError::MissingEnv {
                name: "OPENSEARCH_PASSWORD"
            })
        ));
    }

    #[test]
    fn should_compose_ephemeral_search_without_loading_google_or_other_provider_configuration() {
        let client = opensearch_client(|name| match name {
            "OPENSEARCH_ENDPOINT_URL" => Some("http://127.0.0.1:9".to_owned()),
            "STAGE" => Some("ephemeral".to_owned()),
            _ => panic!("unexpected configuration read: {name}"),
        });
        assert!(client.is_ok());
    }

    #[test]
    fn should_preserve_the_endpoint_parse_cause_without_disclosing_configuration() {
        use std::error::Error;
        let error = match opensearch_client(|_| Some("invalid-private-endpoint".to_owned())) {
            Err(error) => error,
            Ok(_) => panic!("invalid endpoint accepted"),
        };
        assert!(error.source().unwrap().is::<url::ParseError>());
        assert!(!error.to_string().contains("invalid-private-endpoint"));
    }

    struct UnavailableGoogleAdcProvider {
        calls: AtomicUsize,
    }

    impl GoogleAdcProvider for UnavailableGoogleAdcProvider {
        fn credentials(&self) -> Result<AccessTokenCredentials, EmbeddingError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(EmbeddingError::AuthenticationFailed {
                source: Box::new(std::io::Error::other("test Google ADC is unavailable")),
            })
        }
    }

    #[tokio::test]
    async fn should_initialize_vertex_adapter_and_adc_only_when_embedding_needs_it() {
        let adc_provider = Arc::new(UnavailableGoogleAdcProvider {
            calls: AtomicUsize::new(0),
        });
        let embeddings = LazyVertexAiEmbeddingGenerator::with_adc_provider(
            VertexAiEmbeddingConfig::new("test-project", "eu"),
            Arc::clone(&adc_provider) as Arc<dyn GoogleAdcProvider>,
        );

        assert!(embeddings.generator.get().is_none());
        assert_eq!(adc_provider.calls.load(Ordering::SeqCst), 0);

        let result = embeddings.generator().await;

        assert!(matches!(
            result,
            Err(EmbeddingError::AuthenticationFailed { .. })
        ));
        assert!(embeddings.generator.get().is_none());
        assert_eq!(adc_provider.calls.load(Ordering::SeqCst), 1);
    }
}
