use crate::google_llm::{GeminiRateLimiter, run_with_gemini_rate_limiter};
use crate::logging::llm_metrics;
use crate::scraper::css_selector::product_schema::{
    ApplySchemaError, ProductCssSelectorSchema, ShopsProductSchema,
};
use crate::scraper::css_selector::product_schema_repository::ShopsProductSchemaRepository;
use common::logging::{GeminiServiceTier, LlmModel, LlmOperation, LlmProvider, log_llm_invocation};
use common::shop_id::ShopId;
use llm::{
    chat::{ChatMessage, ChatProvider},
    error::LLMError,
};
use prompt::{build_append_schema_instruction, build_create_schemas_instruction};
use response::{parse_product_schemas_response, product_schema_generation_response_schema_json};
use schemars::schema_for;
use std::sync::Arc;
use std::time::Instant;
use time::OffsetDateTime;
use tracing::{debug, info};

#[path = "schema_generation/projection.rs"]
mod projection;
#[path = "schema_generation/prompt.rs"]
mod prompt;
#[path = "schema_generation/response.rs"]
mod response;

pub use projection::{clean_html_for_schema_generation, html_to_schema_prompt_dsl};
pub use prompt::SchemaPromptSource;
pub use response::{
    GeneratedProductSchemas, SchemaLlmEvaluation, SchemaLlmEvaluationConfidence,
    SchemaLlmEvaluationDecision, strip_markdown_json_embedding,
};

#[derive(Debug, thiserror::Error)]
pub enum ProductSchemaServiceError {
    #[error("LLM error: {0}")]
    LLMError(#[from] LLMError),

    #[error("NoTextResponse: {0}")]
    NoTextResponse(String),

    #[error("JsonParsingTargetSchemaError: {0}")]
    JsonParsingTargetSchemaError(serde_json::Error),

    #[error("Database error: {0}")]
    DatabaseError(#[from] sqlx::Error),
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait ProductSchemaService {
    async fn create_product_schema(
        &self,
        html_pages: &[String],
    ) -> Result<ProductCssSelectorSchema, ProductSchemaServiceError>;

    async fn create_product_schemas(
        &self,
        html_pages: &[String],
    ) -> Result<GeneratedProductSchemas, ProductSchemaServiceError>;

    async fn create_product_schemas_with_source(
        &self,
        html_pages: &[String],
        prompt_source: SchemaPromptSource,
    ) -> Result<GeneratedProductSchemas, ProductSchemaServiceError>;

    /// Generate a single schema from a single HTML page and append it to the
    /// cached schema set. Used when a runtime schema-variant match fails to
    /// dynamically expand the schema set without full regeneration.
    async fn append_single_schema(
        &self,
        html: &str,
        prompt_source: SchemaPromptSource,
        failed_schema: Option<&ProductCssSelectorSchema>,
        last_error: Option<&ApplySchemaError>,
    ) -> Result<GeneratedProductSchemas, ProductSchemaServiceError>;

    async fn find_product_schema(
        &self,
        shop_id: &ShopId,
    ) -> Result<Option<ShopsProductSchema>, ProductSchemaServiceError>;

    async fn save_product_schema(
        &self,
        shop_id: &ShopId,
        product_schema: ProductCssSelectorSchema,
    ) -> Result<ShopsProductSchema, ProductSchemaServiceError>;

    async fn save_product_schemas(
        &self,
        shop_id: &ShopId,
        product_schemas: Vec<ProductCssSelectorSchema>,
    ) -> Result<ShopsProductSchema, ProductSchemaServiceError>;

    async fn get_product_schema(
        &self,
        shop_id: &ShopId,
        html_pages: &[String],
    ) -> Result<ShopsProductSchema, ProductSchemaServiceError>;
}

pub struct ProductSchemaServiceImpl {
    llm: Box<dyn ChatProvider>,
    rate_limiter: Option<Arc<GeminiRateLimiter>>,
    service_tier: Option<GeminiServiceTier>,
    repository: Box<dyn ShopsProductSchemaRepository + Send + Sync>,
}

impl ProductSchemaServiceImpl {
    pub fn new(
        llm: llm::builder::LLMBuilder,
        service_tier: Option<GeminiServiceTier>,
        repository: Box<dyn ShopsProductSchemaRepository + Send + Sync>,
        rate_limiter: Option<Arc<GeminiRateLimiter>>,
    ) -> Result<Self, LLMError> {
        let llm = build_schema_generation_llm(llm)?;
        Ok(Self {
            llm,
            rate_limiter,
            service_tier,
            repository,
        })
    }
}

fn build_schema_generation_llm(
    llm: llm::builder::LLMBuilder,
) -> Result<Box<dyn ChatProvider>, LLMError> {
    let schema = serde_json::to_string_pretty(&schema_for!(ProductCssSelectorSchema))
        .unwrap_or_else(|_| "Failed to generate schema".to_string());
    let response_schema = product_schema_generation_response_schema_json();
    let system_prompt = format!(
        "You are an e-commerce scraper-assistant for antiques creating extraction-schemas for HTML given product-pages.
            Return only JSON matching ProductSchemaGenerationResponse.
            The response must include schemas plus confidence LOW, MEDIUM, or HIGH.
            HIGH means the selectors are product-specific, deterministic, and safe to auto-approve when local validation passes.
            MEDIUM means the schema is plausible but needs human review. LOW means uncertain or weak selectors and needs human review.
            ProductCssSelectorSchema schema:\n\n {schema}\n\n
            ProductSchemaGenerationResponse schema:\n\n {response_schema}",
    );
    let llm = llm
        .system(system_prompt)
        .openai_enable_web_search(false)
        .reasoning(true)
        .timeout_seconds(180)
        .validator(|res| {
            parse_product_schemas_response(strip_markdown_json_embedding(res))
                .map(|_| ())
                .map_err(|err| err.to_string())
        })
        .validator_attempts(3)
        .build()?;
    let llm: Box<dyn ChatProvider> = llm;
    Ok(llm)
}

#[async_trait::async_trait]
impl ProductSchemaService for ProductSchemaServiceImpl {
    async fn create_product_schema(
        &self,
        html_pages: &[String],
    ) -> Result<ProductCssSelectorSchema, ProductSchemaServiceError> {
        let generated = self.create_product_schemas(html_pages).await?;
        generated.schemas.first().cloned().ok_or_else(|| {
            ProductSchemaServiceError::NoTextResponse("LLM produced zero schemas".to_string())
        })
    }

    #[tracing::instrument(skip(self, html_pages), fields(html_pages = html_pages.len()))]
    async fn create_product_schemas(
        &self,
        html_pages: &[String],
    ) -> Result<GeneratedProductSchemas, ProductSchemaServiceError> {
        self.create_product_schemas_with_source(html_pages, SchemaPromptSource::YamlProjection)
            .await
    }

    #[tracing::instrument(
        skip(self, html_pages),
        fields(html_pages = html_pages.len(), prompt_source = prompt_source.as_str())
    )]
    async fn create_product_schemas_with_source(
        &self,
        html_pages: &[String],
        prompt_source: SchemaPromptSource,
    ) -> Result<GeneratedProductSchemas, ProductSchemaServiceError> {
        let instruction = build_create_schemas_instruction(html_pages, prompt_source);
        let message = ChatMessage::user().content(instruction).build();
        let messages = vec![message];

        let started_at = Instant::now();
        let response =
            run_with_gemini_rate_limiter(&*self.llm, self.rate_limiter.as_deref(), &messages)
                .await?;
        log_llm_invocation(
            LlmOperation::CrawlerProductSchemaGeneration,
            LlmProvider::Google,
            LlmModel::Configured,
            started_at.elapsed(),
            llm_metrics(response.usage(), Some(html_pages.len()), self.service_tier),
        );
        let res = response.text().ok_or_else(|| {
            ProductSchemaServiceError::NoTextResponse("Expected text response".to_string())
        })?;

        let parsed = strip_markdown_json_embedding(&res);
        let generated = parse_product_schemas_response(parsed)
            .map_err(ProductSchemaServiceError::JsonParsingTargetSchemaError)?;
        if generated.schemas.is_empty() {
            return Err(ProductSchemaServiceError::NoTextResponse(
                "LLM produced zero schemas".to_string(),
            ));
        }

        debug!(
            schemas_count = generated.schemas.len(),
            html_pages = html_pages.len(),
            confidence = ?generated.evaluation.confidence,
            prompt_source = prompt_source.as_str(),
            "LLM created product CSS selector schemas"
        );
        Ok(generated)
    }

    #[tracing::instrument(
        name = "scraper_append_single_schema",
        skip(self, html, failed_schema, last_error),
        fields(prompt_source = prompt_source.as_str())
    )]
    async fn append_single_schema(
        &self,
        html: &str,
        prompt_source: SchemaPromptSource,
        failed_schema: Option<&ProductCssSelectorSchema>,
        last_error: Option<&ApplySchemaError>,
    ) -> Result<GeneratedProductSchemas, ProductSchemaServiceError> {
        let instruction =
            build_append_schema_instruction(html, prompt_source, failed_schema, last_error);
        let message = ChatMessage::user().content(instruction).build();
        let messages = vec![message];

        let started_at = Instant::now();
        let response =
            run_with_gemini_rate_limiter(&*self.llm, self.rate_limiter.as_deref(), &messages)
                .await?;
        log_llm_invocation(
            LlmOperation::CrawlerProductSchemaRepair,
            LlmProvider::Google,
            LlmModel::Configured,
            started_at.elapsed(),
            llm_metrics(response.usage(), Some(1), self.service_tier),
        );
        let res = response.text().ok_or_else(|| {
            ProductSchemaServiceError::NoTextResponse("Expected text response".to_string())
        })?;

        let parsed = strip_markdown_json_embedding(&res);
        let generated = parse_product_schemas_response(parsed)
            .map_err(ProductSchemaServiceError::JsonParsingTargetSchemaError)?;
        if generated.schemas.is_empty() {
            return Err(ProductSchemaServiceError::NoTextResponse(
                "LLM produced zero schemas".to_string(),
            ));
        }
        if generated.schemas.len() != 1 {
            return Err(ProductSchemaServiceError::NoTextResponse(format!(
                "Expected exactly one schema for append generation, got {}",
                generated.schemas.len()
            )));
        }

        info!(
            schema_count = generated.schemas.len(),
            confidence = ?generated.evaluation.confidence,
            prompt_source = prompt_source.as_str(),
            "Generated schema response for append-and-retry"
        );
        Ok(generated)
    }

    async fn find_product_schema(
        &self,
        shop_id: &ShopId,
    ) -> Result<Option<ShopsProductSchema>, ProductSchemaServiceError> {
        self.repository
            .find_product_schema(shop_id)
            .await
            .map_err(ProductSchemaServiceError::DatabaseError)
    }

    async fn save_product_schema(
        &self,
        shop_id: &ShopId,
        product_schema: ProductCssSelectorSchema,
    ) -> Result<ShopsProductSchema, ProductSchemaServiceError> {
        self.save_product_schemas(shop_id, vec![product_schema])
            .await
    }

    #[tracing::instrument(
        name = "scraper_save_product_schemas",
        skip(self, product_schemas),
        fields(shop_id = %shop_id, schema_count = product_schemas.len())
    )]
    async fn save_product_schemas(
        &self,
        shop_id: &ShopId,
        product_schemas: Vec<ProductCssSelectorSchema>,
    ) -> Result<ShopsProductSchema, ProductSchemaServiceError> {
        if product_schemas.is_empty() {
            return Err(ProductSchemaServiceError::NoTextResponse(
                "LLM produced zero schemas".to_string(),
            ));
        }

        let existing = self.repository.find_product_schema(shop_id).await?;

        match existing {
            Some(_) => {
                debug!("Updating existing product schema");
                self.repository
                    .update_product_schema(shop_id, &product_schemas)
                    .await
                    .map_err(ProductSchemaServiceError::DatabaseError)
            }
            None => {
                debug!("Inserting new product schema");
                let now = OffsetDateTime::now_utc();
                let schema = ShopsProductSchema {
                    shop_id: *shop_id,
                    product_schemas,
                    created: now,
                    updated: now,
                };
                self.repository
                    .insert_product_schema(shop_id, &schema)
                    .await
                    .map_err(ProductSchemaServiceError::DatabaseError)
            }
        }
    }

    #[tracing::instrument(
        skip(self, html_pages),
        fields(shop_id = %shop_id, html_pages = html_pages.len())
    )]
    async fn get_product_schema(
        &self,
        shop_id: &ShopId,
        html_pages: &[String],
    ) -> Result<ShopsProductSchema, ProductSchemaServiceError> {
        if let Some(existing) = self.find_product_schema(shop_id).await? {
            debug!("Found existing product schema");
            return Ok(existing);
        }

        info!("No product schema found for shop, creating via LLM");
        let generated = self.create_product_schemas(html_pages).await?;
        self.save_product_schemas(shop_id, generated.schemas).await
    }
}

#[cfg(test)]
mod tests {
    use super::prompt::{build_append_schema_instruction, build_create_schemas_instruction};
    use super::response::parse_product_schemas_response;
    use super::*;
    use crate::scraper::css_selector::product_schema_repository::MockShopsProductSchemaRepository;
    use crate::scraper::css_selector::rule::{
        ExtractionCardinality, ExtractionKind, ExtractionRule,
    };
    use common::shop_id::ShopId;
    use time::OffsetDateTime;

    fn sample_css_schema() -> ProductCssSelectorSchema {
        ProductCssSelectorSchema {
            shops_product_id: Some(ExtractionRule {
                selector: "span.product-id".into(),
                additional_selectors: vec![],
                extract: ExtractionKind::Text,
                cardinality: ExtractionCardinality::First,
            }),
            title: ExtractionRule {
                selector: "h1.title".into(),
                additional_selectors: vec![],
                extract: ExtractionKind::Text,
                cardinality: ExtractionCardinality::First,
            },
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            seller_name: None,
            state: ExtractionRule {
                selector: "span.state".into(),
                additional_selectors: vec![],
                extract: ExtractionKind::Text,
                cardinality: ExtractionCardinality::First,
            },
            images: ExtractionRule {
                selector: "img.product-image".into(),
                additional_selectors: vec![],
                extract: ExtractionKind::Attribute { name: "src".into() },
                cardinality: ExtractionCardinality::All,
            },
            auction_start: None,
            auction_end: None,
            default_currency: None,
        }
    }

    fn sample_shops_product_schema(shop_id: ShopId) -> ShopsProductSchema {
        let now = OffsetDateTime::now_utc();
        ShopsProductSchema {
            shop_id,
            product_schemas: vec![sample_css_schema()],
            created: now,
            updated: now,
        }
    }

    fn generated_response_json(schemas: Vec<ProductCssSelectorSchema>) -> String {
        serde_json::to_string(&serde_json::json!({
            "schemas": schemas,
            "confidence": "HIGH",
            "summary": "Selectors are product-specific.",
            "risks": [],
        }))
        .expect("generated response should serialize")
    }

    #[test]
    fn should_project_extensionless_image_like_href_but_not_product_href() {
        let html = r#"
            <main>
              <a class="full-image" href="/photos/51996"><img src="/thumbs/51996"></a>
              <a href="/products/foo">Product</a>
            </main>
        "#;

        let dsl = html_to_schema_prompt_dsl(html);

        assert!(dsl.contains("href: /photos/51996"));
        assert!(!dsl.contains("href: /products/foo"));
    }

    #[test]
    fn should_not_project_product_json_ld_scripts() {
        let html = r#"
            <script type="application/ld+json">
            {
              "@context": "https://schema.org",
              "@type": "Product",
              "sku": "SKU-42",
              "name": "Biedermeier Chair",
              "image": ["https://cdn.example.com/image/abc123"],
              "brand": {"@type": "Brand", "name": "Antique House"},
              "offers": {
                "@type": "Offer",
                "price": "1200",
                "priceCurrency": "EUR",
                "availability": "https://schema.org/InStock",
                "seller": {"name": "Dealer"}
              }
            }
            </script>
            <script>window.noise = true;</script>
        "#;

        let dsl = html_to_schema_prompt_dsl(html);

        assert!(!dsl.contains("tag: json_ld_product"));
        assert!(!dsl.contains("SKU-42"));
        assert!(!dsl.contains("Biedermeier Chair"));
        assert!(!dsl.contains("offers"));
        assert!(!dsl.contains("window.noise"));
        assert!(!dsl.contains("tag: script"));
    }

    #[test]
    fn should_preserve_product_specific_class_only_wrapper_context() {
        let html = r#"
            <main>
              <div class="product-info"><span class="price">EUR 42</span></div>
              <div class="layout-row"><span class="state">Available</span></div>
            </main>
        "#;

        let dsl = html_to_schema_prompt_dsl(html);

        assert!(dsl.contains("class: product-info"));
        assert!(!dsl.contains("class: layout-row"));
        assert!(dsl.contains("class: price"));
        assert!(dsl.contains("class: state"));
    }

    #[test]
    fn should_project_additional_ecommerce_data_attributes() {
        let html = r#"
            <div itemscope itemtype="https://schema.org/Product"
                 data-id="42"
                 data-variant-id="v1"
                 data-product="payload"
                 data-srcset="/image/abc 1200w"
                 data-gallery="main"
                 data-lightbox="product"
                 data-fancybox="gallery">
            </div>
        "#;

        let dsl = html_to_schema_prompt_dsl(html);

        for needle in [
            "itemscope:",
            "itemtype: https://schema.org/Product",
            "data-id: '42'",
            "data-variant-id: v1",
            "data-product: payload",
            "data-srcset: /image/abc 1200w",
            "data-gallery: main",
            "data-lightbox: product",
            "data-fancybox: gallery",
        ] {
            assert!(dsl.contains(needle), "DSL missing {needle}");
        }
    }

    // -----------------------------------------------------------------------
    // find_product_schema
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn should_return_schema_when_found_in_repository_for_find() {
        let shop_id = ShopId::new();
        let expected = sample_shops_product_schema(shop_id);
        let expected_clone = expected.clone();

        let mut repository = MockShopsProductSchemaRepository::new();
        repository
            .expect_find_product_schema()
            .withf(move |id| *id == shop_id)
            .return_once(move |_| Box::pin(async move { Ok(Some(expected_clone)) }));

        let service = ProductSchemaServiceImpl {
            llm: Box::new(MockLlmProvider),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repository),
        };

        let result = service.find_product_schema(&shop_id).await.unwrap();
        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.shop_id, expected.shop_id);
        assert_eq!(result.product_schemas, expected.product_schemas);
    }

    #[tokio::test]
    async fn should_return_none_when_not_found_in_repository_for_find() {
        let shop_id = ShopId::new();

        let mut repository = MockShopsProductSchemaRepository::new();
        repository
            .expect_find_product_schema()
            .return_once(|_| Box::pin(async { Ok(None) }));

        let service = ProductSchemaServiceImpl {
            llm: Box::new(MockLlmProvider),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repository),
        };

        let result = service.find_product_schema(&shop_id).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn should_propagate_database_error_for_find() {
        let shop_id = ShopId::new();

        let mut repository = MockShopsProductSchemaRepository::new();
        repository
            .expect_find_product_schema()
            .return_once(|_| Box::pin(async { Err(sqlx::Error::RowNotFound) }));

        let service = ProductSchemaServiceImpl {
            llm: Box::new(MockLlmProvider),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repository),
        };

        let result = service.find_product_schema(&shop_id).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ProductSchemaServiceError::DatabaseError(_)
        ));
    }

    // -----------------------------------------------------------------------
    // save_product_schema
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn should_insert_schema_when_not_existing_for_save() {
        let shop_id = ShopId::new();
        let css_schema = sample_css_schema();
        let expected = sample_shops_product_schema(shop_id);
        let expected_clone = expected.clone();

        let mut repository = MockShopsProductSchemaRepository::new();
        repository
            .expect_find_product_schema()
            .return_once(|_| Box::pin(async { Ok(None) }));
        repository
            .expect_insert_product_schema()
            .withf(move |id, _schema| *id == shop_id)
            .return_once(move |_, _| Box::pin(async move { Ok(expected_clone) }));

        let service = ProductSchemaServiceImpl {
            llm: Box::new(MockLlmProvider),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repository),
        };

        let result = service
            .save_product_schema(&shop_id, css_schema)
            .await
            .unwrap();
        assert_eq!(result.shop_id, expected.shop_id);
        assert_eq!(result.product_schemas, expected.product_schemas);
    }

    #[tokio::test]
    async fn should_update_schema_when_already_existing_for_save() {
        let shop_id = ShopId::new();
        let existing = sample_shops_product_schema(shop_id);
        let existing_clone = existing.clone();
        let css_schema = sample_css_schema();
        let updated = sample_shops_product_schema(shop_id);
        let updated_clone = updated.clone();

        let mut repository = MockShopsProductSchemaRepository::new();
        repository
            .expect_find_product_schema()
            .return_once(move |_| Box::pin(async move { Ok(Some(existing_clone)) }));
        repository
            .expect_update_product_schema()
            .withf(move |id, _schema| *id == shop_id)
            .return_once(move |_, _| Box::pin(async move { Ok(updated_clone) }));

        let service = ProductSchemaServiceImpl {
            llm: Box::new(MockLlmProvider),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repository),
        };

        let result = service
            .save_product_schema(&shop_id, css_schema)
            .await
            .unwrap();
        assert_eq!(result.shop_id, updated.shop_id);
    }

    #[tokio::test]
    async fn should_propagate_database_error_when_find_fails_for_save() {
        let shop_id = ShopId::new();
        let css_schema = sample_css_schema();

        let mut repository = MockShopsProductSchemaRepository::new();
        repository
            .expect_find_product_schema()
            .return_once(|_| Box::pin(async { Err(sqlx::Error::RowNotFound) }));

        let service = ProductSchemaServiceImpl {
            llm: Box::new(MockLlmProvider),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repository),
        };

        let result = service.save_product_schema(&shop_id, css_schema).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ProductSchemaServiceError::DatabaseError(_)
        ));
    }

    #[tokio::test]
    async fn should_propagate_database_error_when_insert_fails_for_save() {
        let shop_id = ShopId::new();
        let css_schema = sample_css_schema();

        let mut repository = MockShopsProductSchemaRepository::new();
        repository
            .expect_find_product_schema()
            .return_once(|_| Box::pin(async { Ok(None) }));
        repository
            .expect_insert_product_schema()
            .return_once(|_, _| Box::pin(async { Err(sqlx::Error::RowNotFound) }));

        let service = ProductSchemaServiceImpl {
            llm: Box::new(MockLlmProvider),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repository),
        };

        let result = service.save_product_schema(&shop_id, css_schema).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ProductSchemaServiceError::DatabaseError(_)
        ));
    }

    // -----------------------------------------------------------------------
    // get_product_schema
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn should_return_existing_schema_without_llm_call_for_get() {
        let shop_id = ShopId::new();
        let existing = sample_shops_product_schema(shop_id);
        let existing_clone = existing.clone();

        let mut repository = MockShopsProductSchemaRepository::new();
        repository
            .expect_find_product_schema()
            .return_once(move |_| Box::pin(async move { Ok(Some(existing_clone)) }));
        // insert and update should NOT be called
        repository.expect_insert_product_schema().never();
        repository.expect_update_product_schema().never();

        let service = ProductSchemaServiceImpl {
            llm: Box::new(MockLlmProvider),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repository),
        };

        let html_pages = vec!["<html></html>".to_string()];
        let result = service
            .get_product_schema(&shop_id, &html_pages)
            .await
            .unwrap();
        assert_eq!(result.shop_id, existing.shop_id);
        assert_eq!(result.product_schemas, existing.product_schemas);
    }

    #[tokio::test]
    async fn should_create_and_save_schema_when_not_found_for_get() {
        let shop_id = ShopId::new();
        let css_schema = sample_css_schema();
        let saved = sample_shops_product_schema(shop_id);
        let saved_clone = saved.clone();

        let mut repository = MockShopsProductSchemaRepository::new();

        // First call from get_product_schema -> find_product_schema
        repository
            .expect_find_product_schema()
            .times(1)
            .return_once(|_| Box::pin(async { Ok(None) }));

        // Second call from save_product_schema -> find_product_schema (upsert check)
        repository
            .expect_find_product_schema()
            .times(1)
            .return_once(|_| Box::pin(async { Ok(None) }));

        repository
            .expect_insert_product_schema()
            .return_once(move |_, _| Box::pin(async move { Ok(saved_clone) }));

        let service = ProductSchemaServiceImpl {
            llm: Box::new(MockLlmProviderReturning(css_schema)),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repository),
        };

        let html_pages = vec!["<html></html>".to_string()];
        let result = service
            .get_product_schema(&shop_id, &html_pages)
            .await
            .unwrap();
        assert_eq!(result.shop_id, saved.shop_id);
    }

    #[tokio::test]
    async fn should_propagate_database_error_when_find_fails_for_get() {
        let shop_id = ShopId::new();

        let mut repository = MockShopsProductSchemaRepository::new();
        repository
            .expect_find_product_schema()
            .return_once(|_| Box::pin(async { Err(sqlx::Error::RowNotFound) }));

        let service = ProductSchemaServiceImpl {
            llm: Box::new(MockLlmProvider),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repository),
        };

        let html_pages = vec!["<html></html>".to_string()];
        let result = service.get_product_schema(&shop_id, &html_pages).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ProductSchemaServiceError::DatabaseError(_)
        ));
    }

    #[test]
    fn should_include_all_html_samples_in_create_instruction() {
        let html_pages = vec![
            "<html><body><h1>A</h1></body></html>".to_string(),
            "<html><body><h1>B</h1></body></html>".to_string(),
        ];
        let instruction =
            build_create_schemas_instruction(&html_pages, SchemaPromptSource::YamlProjection);
        assert!(instruction.contains("--- SAMPLE 1 YAML ---"));
        assert!(instruction.contains("--- SAMPLE 2 YAML ---"));
        assert!(instruction.contains("page YAML samples"));
        assert!(instruction.contains("Derive CSS selectors"));
        assert!(instruction.contains("Return one schema per distinct template"));
        assert!(instruction.contains("not one schema per page"));
        assert!(instruction.contains("The schemas field contains one ProductCssSelectorSchema"));
        assert!(instruction.contains("multiple schemas ordered as described"));
    }

    #[test]
    fn should_build_cleaned_html_fallback_create_instruction() {
        let html_pages =
            vec!["<html><body><script>noise</script><h1>A</h1></body></html>".to_string()];

        let instruction =
            build_create_schemas_instruction(&html_pages, SchemaPromptSource::CleanedHtmlFallback);

        assert!(instruction.contains("--- SAMPLE 1 CLEANED HTML ---"));
        assert!(instruction.contains("cleaned HTML from the original pages"));
        assert!(instruction.contains("<h1>A</h1>"));
        assert!(!instruction.contains("noise"));
    }

    #[test]
    fn should_build_cleaned_html_fallback_append_instruction() {
        let html = "<html><body><script>noise</script><h1>A</h1></body></html>";

        let instruction = build_append_schema_instruction(
            html,
            SchemaPromptSource::CleanedHtmlFallback,
            None,
            None,
        );

        assert!(instruction.contains("page CLEANED HTML"));
        assert!(instruction.contains("cleaned HTML from the original pages"));
        assert!(instruction.contains("<h1>A</h1>"));
        assert!(!instruction.contains("page_dsl"));
        assert!(!instruction.contains("noise"));
    }

    #[test]
    fn should_build_schema_prompt_dsl_with_generic_tree_nodes() {
        let html = r#"
            <html>
              <head>
                <meta property="og:title" content="Example: Vase">
                <script>window.noisy = true;</script>
              </head>
              <body>
                <nav><a href="/noise">Noise</a></nav>
                <input id="ProductId" name="ProductId" type="hidden" value="SKU-42">
                <section class="product">
                  <h1 class="title">Example "Vase"</h1>
                  <a class="full-image" href="/large-link.jpg" rel="gallery">
                    <img src="/image.jpg" data-large_image="/large.jpg">
                  </a>
                  <button role="button" aria-label="Add to basket" data-product-id="SKU-42" data-price="10.00" data-currency="EUR" data-availability="available">Buy</button>
                </section>
              </body>
            </html>
        "#;

        let dsl = html_to_schema_prompt_dsl(html);

        assert!(dsl.contains("tag: meta"));
        assert!(dsl.contains("property: og:title"));
        assert!(dsl.contains("content: 'Example: Vase'"));
        assert!(dsl.contains("tag: input"));
        assert!(dsl.contains("id: ProductId"));
        assert!(dsl.contains("value: SKU-42"));
        assert!(dsl.contains("tag: h1"));
        assert!(dsl.contains("class: title"));
        assert!(dsl.contains("tag: img"));
        assert!(dsl.contains("href: /large-link.jpg"));
        assert!(dsl.contains("rel: gallery"));
        assert!(dsl.contains("role: button"));
        assert!(dsl.contains("aria-label: Add to basket"));
        assert!(dsl.contains("src: /image.jpg"));
        assert!(dsl.contains("data-large_image: /large.jpg"));
        assert!(dsl.contains("data-product-id: SKU-42"));
        assert!(dsl.contains("data-price: '10.00'"));
        assert!(dsl.contains("data-currency: EUR"));
        assert!(dsl.contains("data-availability: available"));
        assert!(dsl.contains("Example \"Vase\""));
        assert!(!dsl.contains("<script"));
        assert!(!dsl.contains("<nav"));
    }

    #[test]
    fn should_build_schema_prompt_dsl_deterministically() {
        let html = r#"
            <html><body>
              <div class="product"><span class="price">10 EUR</span></div>
              <input id="State" value="available">
            </body></html>
        "#;

        assert_eq!(
            html_to_schema_prompt_dsl(html),
            html_to_schema_prompt_dsl(html)
        );
    }

    #[test]
    fn should_parse_generated_schemas_response() {
        let payload = generated_response_json(vec![sample_css_schema()]);
        let parsed = parse_product_schemas_response(&payload).unwrap();
        assert_eq!(parsed.schemas.len(), 1);
        assert_eq!(parsed.schemas[0], sample_css_schema());
        assert_eq!(
            parsed.evaluation.confidence,
            SchemaLlmEvaluationConfidence::High
        );
        assert!(parsed.evaluation.is_high_confidence_approval());
    }

    #[test]
    fn should_reject_legacy_single_schema_response_without_confidence() {
        let payload = serde_json::to_string(&sample_css_schema()).unwrap();
        let parsed = parse_product_schemas_response(&payload);
        assert!(parsed.is_err());
    }

    #[test]
    fn should_reject_empty_generated_schema_array() {
        let payload = generated_response_json(Vec::new());
        let parsed = parse_product_schemas_response(&payload);
        assert!(parsed.is_err());
    }

    #[test]
    fn should_reject_generated_response_without_confidence() {
        let payload = serde_json::to_string(&serde_json::json!({
            "schemas": [sample_css_schema()],
            "summary": "missing confidence"
        }))
        .unwrap();
        let parsed = parse_product_schemas_response(&payload);
        assert!(parsed.is_err());
    }

    #[test]
    fn should_parse_high_confidence_generation_response() {
        let payload = serde_json::to_string(&serde_json::json!({
            "schemas": [sample_css_schema()],
            "confidence": "HIGH",
            "summary": "Selectors are product-specific.",
            "risks": [],
        }))
        .unwrap();

        let generated = parse_product_schemas_response(&payload).unwrap();
        let evaluation = generated.evaluation;

        assert!(evaluation.is_high_confidence_approval());
        assert!(!evaluation.approved_by_llm);
        assert_eq!(evaluation.summary, "Selectors are product-specific.");
    }

    #[test]
    fn should_mark_unavailable_schema_evaluation_as_low_confidence_human_review() {
        let evaluation = SchemaLlmEvaluation::unavailable("budget exhausted");

        assert_eq!(
            evaluation.decision,
            SchemaLlmEvaluationDecision::NeedsHumanReview
        );
        assert_eq!(evaluation.confidence, SchemaLlmEvaluationConfidence::Low);
        assert!(!evaluation.approved_by_llm);
        assert!(!evaluation.is_high_confidence_approval());
    }

    // -----------------------------------------------------------------------
    // Helpers: Mock LLM providers
    // -----------------------------------------------------------------------

    /// A concrete [`llm::chat::ChatResponse`] for test doubles.
    #[derive(Debug)]
    struct FakeChatResponse(Option<String>);

    impl std::fmt::Display for FakeChatResponse {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match &self.0 {
                Some(text) => write!(f, "{text}"),
                None => write!(f, ""),
            }
        }
    }

    impl llm::chat::ChatResponse for FakeChatResponse {
        fn text(&self) -> Option<String> {
            self.0.clone()
        }

        fn tool_calls(&self) -> Option<Vec<llm::ToolCall>> {
            None
        }
    }

    /// A mock LLM provider that panics if called — used when we expect no LLM
    /// interaction.
    struct MockLlmProvider;

    #[async_trait::async_trait]
    impl llm::chat::ChatProvider for MockLlmProvider {
        async fn chat_with_tools(
            &self,
            _messages: &[ChatMessage],
            _tools: Option<&[llm::chat::Tool]>,
        ) -> Result<Box<dyn llm::chat::ChatResponse>, LLMError> {
            panic!("LLM should not be called in this test")
        }
    }

    /// A mock LLM provider that returns a fixed `ProductCssSelectorSchema`.
    struct MockLlmProviderReturning(ProductCssSelectorSchema);

    #[async_trait::async_trait]
    impl llm::chat::ChatProvider for MockLlmProviderReturning {
        async fn chat_with_tools(
            &self,
            _messages: &[ChatMessage],
            _tools: Option<&[llm::chat::Tool]>,
        ) -> Result<Box<dyn llm::chat::ChatResponse>, LLMError> {
            let json = generated_response_json(vec![self.0.clone()]);
            Ok(Box::new(FakeChatResponse(Some(json))))
        }
    }
}
