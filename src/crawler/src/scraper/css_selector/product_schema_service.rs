use crate::google_llm::{GeminiRateLimiter, run_with_gemini_rate_limiter};
use crate::logging::llm_metrics;
use crate::scraper::css_selector::product_schema::{ProductCssSelectorSchema, ShopsProductSchema};
use crate::scraper::css_selector::product_schema_repository::ShopsProductSchemaRepository;
use common::logging::{GeminiServiceTier, LlmModel, LlmOperation, LlmProvider, log_llm_invocation};
use common::shop_id::ShopId;
use llm::{
    chat::{ChatMessage, ChatProvider},
    error::LLMError,
};
use prompt::{build_append_schema_instruction, build_create_schemas_instruction};
use response::{
    append_schema_generation_response_schema_json, parse_append_schema_response,
    parse_product_schemas_response, product_schema_generation_response_schema_json,
};
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

pub use projection::html_to_schema_prompt_dsl;
pub use response::{
    GeneratedAppendSchema, GeneratedProductSchemas, SchemaLlmEvaluation,
    SchemaLlmEvaluationConfidence, SchemaLlmEvaluationDecision, strip_markdown_json_embedding,
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

    /// Generate a single schema from a single HTML page and append it to the
    /// cached schema set. Used when a runtime schema-variant match fails to
    /// dynamically expand the schema set without full regeneration.
    async fn append_single_schema(
        &self,
        html: &str,
    ) -> Result<GeneratedAppendSchema, ProductSchemaServiceError>;

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
    create_llm: Box<dyn ChatProvider>,
    append_llm: Box<dyn ChatProvider>,
    rate_limiter: Option<Arc<GeminiRateLimiter>>,
    service_tier: Option<GeminiServiceTier>,
    repository: Box<dyn ShopsProductSchemaRepository + Send + Sync>,
}

impl ProductSchemaServiceImpl {
    pub fn new(
        create_llm: llm::builder::LLMBuilder,
        append_llm: llm::builder::LLMBuilder,
        service_tier: Option<GeminiServiceTier>,
        repository: Box<dyn ShopsProductSchemaRepository + Send + Sync>,
        rate_limiter: Option<Arc<GeminiRateLimiter>>,
    ) -> Result<Self, LLMError> {
        let create_llm = build_create_schema_generation_llm(create_llm)?;
        let append_llm = build_append_schema_generation_llm(append_llm)?;
        Ok(Self {
            create_llm,
            append_llm,
            rate_limiter,
            service_tier,
            repository,
        })
    }
}

fn build_create_schema_generation_llm(
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
            Never guess default_currency. Never use EUR or another arbitrary currency as a generic fallback. If currency evidence is unreliable, the response must not be high confidence.
            MEDIUM means the schema is plausible but needs human review. LOW means uncertain or weak selectors and needs human review.
            ProductCssSelectorSchema schema:\n\n {schema}\n\n
            ProductSchemaGenerationResponse schema:\n\n {response_schema}",
    );
    let llm = llm
        .system(system_prompt)
        .openai_enable_web_search(false)
        .reasoning(true)
        .timeout_seconds(180)
        .validator(validate_create_schema_response)
        .validator_attempts(3)
        .build()?;
    let llm: Box<dyn ChatProvider> = llm;
    Ok(llm)
}

fn build_append_schema_generation_llm(
    llm: llm::builder::LLMBuilder,
) -> Result<Box<dyn ChatProvider>, LLMError> {
    let schema = serde_json::to_string_pretty(&schema_for!(ProductCssSelectorSchema))
        .unwrap_or_else(|_| "Failed to generate schema".to_string());
    let append_response_schema = append_schema_generation_response_schema_json();
    let system_prompt = format!(
        "You are an e-commerce scraper-assistant for antiques repairing extraction-schemas for one HTML page.
            Return only JSON matching Append ProductSchemaGenerationResponse.
            The response may classify the page as product, removed, or not_product.
            Never guess default_currency. Never use EUR or another arbitrary currency as a generic fallback. If currency evidence is unreliable, the response must not be high confidence.
            ProductCssSelectorSchema schema:\n\n {schema}\n\n
            Append ProductSchemaGenerationResponse schema:\n\n {append_response_schema}",
    );
    let llm = llm
        .system(system_prompt)
        .openai_enable_web_search(false)
        .reasoning(true)
        .timeout_seconds(180)
        .validator(validate_append_schema_response)
        .validator_attempts(3)
        .build()?;
    let llm: Box<dyn ChatProvider> = llm;
    Ok(llm)
}

fn validate_create_schema_response(res: &str) -> Result<(), String> {
    let stripped = strip_markdown_json_embedding(res);
    parse_product_schemas_response(stripped)
        .map(|_| ())
        .map_err(|_| "response did not match product schema response".to_string())
}

fn validate_append_schema_response(res: &str) -> Result<(), String> {
    let stripped = strip_markdown_json_embedding(res);
    parse_append_schema_response(stripped)
        .map(|_| ())
        .map_err(|_| "response did not match append schema response".to_string())
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
        let instruction = build_create_schemas_instruction(html_pages);
        let message = ChatMessage::user().content(instruction).build();
        let messages = vec![message];

        let started_at = Instant::now();
        let response = run_with_gemini_rate_limiter(
            &*self.create_llm,
            self.rate_limiter.as_deref(),
            &messages,
        )
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
            "LLM created product CSS selector schemas"
        );
        Ok(generated)
    }

    #[tracing::instrument(name = "scraper_append_single_schema", skip(self, html))]
    async fn append_single_schema(
        &self,
        html: &str,
    ) -> Result<GeneratedAppendSchema, ProductSchemaServiceError> {
        let instruction = build_append_schema_instruction(html);
        let message = ChatMessage::user().content(instruction).build();
        let messages = vec![message];

        let started_at = Instant::now();
        let response = run_with_gemini_rate_limiter(
            &*self.append_llm,
            self.rate_limiter.as_deref(),
            &messages,
        )
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
        let generated = parse_append_schema_response(parsed)
            .map_err(ProductSchemaServiceError::JsonParsingTargetSchemaError)?;

        debug!(
            page_kind = ?generated,
            confidence = ?generated.evaluation().confidence,
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
    use super::prompt::build_append_schema_instruction;
    use super::prompt::build_create_schemas_instruction;
    use super::response::parse_append_schema_response;
    use super::response::parse_product_schemas_response;
    use super::*;
    use crate::scraper::css_selector::currency_dto::CurrencyDto;
    use crate::scraper::css_selector::product_schema_repository::MockShopsProductSchemaRepository;
    use crate::scraper::css_selector::removed_page_schema::RemovedPageSchema;
    use crate::scraper::css_selector::rule::{
        ExtractionCardinality, ExtractionKind, ExtractionRule,
    };
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
            default_currency: CurrencyDto::Eur,
            raw_attributes: Default::default(),
        }
    }

    fn sample_zar_css_schema() -> ProductCssSelectorSchema {
        ProductCssSelectorSchema {
            default_currency: CurrencyDto::Zar,
            ..sample_css_schema()
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

    fn sample_schema_with_raw_attributes() -> ProductCssSelectorSchema {
        let raw_attributes = [
            ("rawShipment", ".shipping"),
            ("rawCondition", ".condition"),
            ("rawMaterial", ".material"),
            ("rawYear", ".year"),
            ("rawPeriod", ".period"),
            ("rawCategory", ".category"),
            ("rawTags", ".tags"),
            ("rawMeasurements", ".measurements"),
            ("rawOrigin", ".origin"),
            ("rawArtistName", ".artist"),
            ("rawMakerName", ".maker"),
            ("rawDesignerName", ".designer"),
            ("rawBrandName", ".brand"),
            ("rawSignature", ".signature"),
            ("rawCreatorNote", ".creator-note"),
        ]
        .into_iter()
        .map(|(key, selector)| {
            (
                key.to_string(),
                ExtractionRule {
                    selector: selector.into(),
                    additional_selectors: vec![],
                    extract: ExtractionKind::Text,
                    cardinality: ExtractionCardinality::All,
                },
            )
        })
        .collect();

        ProductCssSelectorSchema {
            raw_attributes,
            ..sample_css_schema()
        }
    }

    fn removed_append_response_json() -> String {
        serde_json::to_string(&serde_json::json!({
            "page_kind": "removed",
            "schemas": [],
            "removed_schema": {
                "selector": "#mainCatCol h1",
                "text": "Sorry, the page you're looking for couldn't be found"
            },
            "confidence": "HIGH",
            "summary": "Soft 404 page.",
            "risks": [],
        }))
        .expect("removed append response should serialize")
    }

    fn removed_regex_append_response_json(pattern: &str) -> String {
        serde_json::to_string(&serde_json::json!({
            "page_kind": "removed",
            "schemas": [],
            "removed_schema": {
                "selector": "#mainCatCol h1",
                "regex": pattern
            },
            "confidence": "HIGH",
            "summary": "Soft 404 page.",
            "risks": [],
        }))
        .expect("removed regex append response should serialize")
    }

    fn not_product_append_response_json() -> String {
        serde_json::to_string(&serde_json::json!({
            "page_kind": "not_product",
            "schemas": [],
            "reason": "category page",
            "confidence": "HIGH",
            "summary": "Category page.",
            "risks": [],
        }))
        .expect("not-product append response should serialize")
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
            create_llm: Box::new(MockLlmProvider),
            append_llm: Box::new(MockLlmProvider),
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
            create_llm: Box::new(MockLlmProvider),
            append_llm: Box::new(MockLlmProvider),
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
            create_llm: Box::new(MockLlmProvider),
            append_llm: Box::new(MockLlmProvider),
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
            .withf(move |id, _| *id == shop_id)
            .return_once(move |_, _| Box::pin(async move { Ok(expected_clone) }));

        let service = ProductSchemaServiceImpl {
            create_llm: Box::new(MockLlmProvider),
            append_llm: Box::new(MockLlmProvider),
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
            .withf(move |id, _| *id == shop_id)
            .return_once(move |_, _| Box::pin(async move { Ok(updated_clone) }));

        let service = ProductSchemaServiceImpl {
            create_llm: Box::new(MockLlmProvider),
            append_llm: Box::new(MockLlmProvider),
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
            create_llm: Box::new(MockLlmProvider),
            append_llm: Box::new(MockLlmProvider),
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
            create_llm: Box::new(MockLlmProvider),
            append_llm: Box::new(MockLlmProvider),
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
            create_llm: Box::new(MockLlmProvider),
            append_llm: Box::new(MockLlmProvider),
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
            create_llm: Box::new(MockLlmProviderReturning(css_schema)),
            append_llm: Box::new(MockLlmProvider),
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
    async fn should_preserve_raw_attribute_schema_from_mocked_llm_response() {
        let css_schema = sample_schema_with_raw_attributes();
        let service = ProductSchemaServiceImpl {
            create_llm: Box::new(MockLlmProviderReturning(css_schema)),
            append_llm: Box::new(MockLlmProvider),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(MockShopsProductSchemaRepository::new()),
        };

        let result = service
            .create_product_schemas(&[
                "<html><body><p class=\"shipping\">Ships in 2 weeks</p></body></html>".to_string(),
            ])
            .await
            .unwrap();

        assert_eq!(
            result.schemas[0]
                .raw_attributes
                .get("rawMaterial")
                .map(|rule| rule.selector.to_string()),
            Some(".material".to_string())
        );
        assert_eq!(
            result.schemas[0]
                .raw_attributes
                .get("rawPeriod")
                .map(|rule| rule.selector.to_string()),
            Some(".period".to_string())
        );
        assert_eq!(
            result.schemas[0]
                .raw_attributes
                .get("rawArtistName")
                .map(|rule| rule.selector.to_string()),
            Some(".artist".to_string())
        );
    }

    fn assert_raw_attribute_instruction(instruction: &str) {
        assert!(instruction.contains("configured raw attribute groups"));
        assert!(instruction.contains("Do not generate arbitrary raw attribute keys"));
        assert!(instruction.contains("Prefer the broad group key"));
        assert!(instruction.contains("specific measurement or origin keys only"));
        assert!(instruction.contains("rawShipment"));
        assert!(instruction.contains("rawShipmentNote"));
        assert!(instruction.contains("rawShipmentMin"));
        assert!(instruction.contains("rawShipmentMax"));
        assert!(instruction.contains("rawCondition"));
        assert!(instruction.contains("rawConditionNote"));
        assert!(instruction.contains("rawMaterial"));
        assert!(instruction.contains("rawMaterialNote"));
        assert!(instruction.contains("rawYear"));
        assert!(instruction.contains("rawPeriod"));
        assert!(instruction.contains("rawYearNote"));
        assert!(instruction.contains("rawCategory"));
        assert!(instruction.contains("rawCategoryPath"));
        assert!(instruction.contains("rawTags"));
        assert!(instruction.contains("rawMeasurements"));
        assert!(instruction.contains("rawHeight"));
        assert!(instruction.contains("rawWidth"));
        assert!(instruction.contains("rawDepth"));
        assert!(instruction.contains("rawDiameter"));
        assert!(instruction.contains("rawWeight"));
        assert!(instruction.contains("rawMeasurementNote"));
        assert!(instruction.contains("rawOrigin"));
        assert!(instruction.contains("rawCountry"));
        assert!(instruction.contains("rawRegion"));
        assert!(instruction.contains("rawOriginNote"));
        assert!(instruction.contains("rawArtistName"));
        assert!(instruction.contains("rawMakerName"));
        assert!(instruction.contains("rawDesignerName"));
        assert!(instruction.contains("rawBrandName"));
        assert!(instruction.contains("rawSignature"));
        assert!(instruction.contains("rawCreatorNote"));
        assert!(instruction.contains("specific creator keys"));
        assert!(instruction.contains("rawCreatorNote for combined"));
        assert!(instruction.contains("Do not infer artist"));
        assert!(instruction.contains("meta author"));
        assert!(instruction.contains("seller"));
    }

    #[tokio::test]
    async fn should_propagate_database_error_when_find_fails_for_get() {
        let shop_id = ShopId::new();

        let mut repository = MockShopsProductSchemaRepository::new();
        repository
            .expect_find_product_schema()
            .return_once(|_| Box::pin(async { Err(sqlx::Error::RowNotFound) }));

        let service = ProductSchemaServiceImpl {
            create_llm: Box::new(MockLlmProvider),
            append_llm: Box::new(MockLlmProvider),
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
        let instruction = build_create_schemas_instruction(&html_pages);
        assert!(instruction.contains("--- SAMPLE 1 YAML ---"));
        assert!(instruction.contains("--- SAMPLE 2 YAML ---"));
        assert!(instruction.contains("page YAML samples"));
        assert!(instruction.contains("Derive CSS selectors"));
        assert!(instruction.contains("Return one schema per distinct template"));
        assert!(instruction.contains("not one schema per page"));
        assert!(instruction.contains("The schemas field contains one ProductCssSelectorSchema"));
        assert!(instruction.contains("multiple schemas ordered as described"));
        assert_raw_attribute_instruction(&instruction);
    }

    #[test]
    fn should_include_page_classification_in_append_instruction() {
        let instruction =
            build_append_schema_instruction("<html><body><h1>Missing</h1></body></html>");

        assert!(instruction.contains("page_kind = product"));
        assert!(instruction.contains("page_kind = removed"));
        assert!(instruction.contains("page_kind = not_product"));
        assert!(instruction.contains("Return no schemas and include a short reason"));
        assert!(instruction.contains("removed_schema must include selector"));
        assert!(instruction.contains("exactly one of text or regex"));
        assert_raw_attribute_instruction(&instruction);
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
    fn should_accept_generated_product_schema_with_zar_default_currency() {
        let payload = generated_response_json(vec![sample_zar_css_schema()]);

        let parsed = parse_product_schemas_response(&payload).unwrap();

        assert_eq!(parsed.schemas[0].default_currency, CurrencyDto::Zar);
        assert!(validate_create_schema_response(&payload).is_ok());
    }

    #[test]
    fn should_parse_generated_schemas_response_with_raw_attributes() {
        let payload = generated_response_json(vec![sample_schema_with_raw_attributes()]);

        let parsed = parse_product_schemas_response(&payload).unwrap();

        assert_eq!(
            parsed.schemas[0]
                .raw_attributes
                .get("rawOrigin")
                .map(|rule| rule.selector.to_string()),
            Some(".origin".to_string())
        );
        assert_eq!(
            parsed.schemas[0]
                .raw_attributes
                .get("rawPeriod")
                .map(|rule| rule.selector.to_string()),
            Some(".period".to_string())
        );
        assert_eq!(
            parsed.schemas[0]
                .raw_attributes
                .get("rawCreatorNote")
                .map(|rule| rule.selector.to_string()),
            Some(".creator-note".to_string())
        );
    }

    #[test]
    fn should_parse_product_append_response() {
        let payload = generated_response_json(vec![sample_css_schema()]);

        let parsed = parse_append_schema_response(&payload).unwrap();

        assert!(matches!(parsed, GeneratedAppendSchema::Product { .. }));
    }

    #[test]
    fn should_parse_removed_append_response() {
        let parsed = parse_append_schema_response(&removed_append_response_json()).unwrap();

        assert_eq!(
            parsed,
            GeneratedAppendSchema::Removed {
                schema: RemovedPageSchema {
                    selector: "#mainCatCol h1".into(),
                    text: Some("Sorry, the page you're looking for couldn't be found".to_string()),
                    regex: None,
                },
                evaluation: SchemaLlmEvaluation {
                    decision: SchemaLlmEvaluationDecision::Approve,
                    confidence: SchemaLlmEvaluationConfidence::High,
                    approved_by_llm: false,
                    summary: "Soft 404 page.".to_string(),
                    risks: vec![],
                },
            }
        );
    }

    #[test]
    fn should_accept_removed_regex_for_append_validator() {
        let parsed = parse_append_schema_response(&removed_regex_append_response_json(
            r"the .+ is not available anymore",
        ))
        .unwrap();

        assert_eq!(
            parsed,
            GeneratedAppendSchema::Removed {
                schema: RemovedPageSchema {
                    selector: "#mainCatCol h1".into(),
                    text: None,
                    regex: Some(r"the .+ is not available anymore".to_string()),
                },
                evaluation: SchemaLlmEvaluation {
                    decision: SchemaLlmEvaluationDecision::Approve,
                    confidence: SchemaLlmEvaluationConfidence::High,
                    approved_by_llm: false,
                    summary: "Soft 404 page.".to_string(),
                    risks: vec![],
                },
            }
        );
    }

    #[test]
    fn should_reject_removed_invalid_regex_for_append_validator() {
        let payload = removed_regex_append_response_json("[unclosed");

        assert!(parse_append_schema_response(&payload).is_err());
        assert!(validate_append_schema_response(&payload).is_err());
    }

    #[test]
    fn should_reject_removed_text_and_regex_for_append_validator() {
        let payload = serde_json::to_string(&serde_json::json!({
            "page_kind": "removed",
            "schemas": [],
            "removed_schema": {
                "selector": "#mainCatCol h1",
                "text": "Product removed",
                "regex": "product removed"
            },
            "confidence": "HIGH",
            "summary": "ambiguous evidence"
        }))
        .unwrap();

        assert!(parse_append_schema_response(&payload).is_err());
        assert!(validate_append_schema_response(&payload).is_err());
    }

    #[test]
    fn should_reject_removed_without_text_or_regex_for_append_validator() {
        let payload = serde_json::to_string(&serde_json::json!({
            "page_kind": "removed",
            "schemas": [],
            "removed_schema": {
                "selector": "#mainCatCol h1"
            },
            "confidence": "HIGH",
            "summary": "missing evidence"
        }))
        .unwrap();

        assert!(parse_append_schema_response(&payload).is_err());
    }

    #[test]
    fn should_parse_not_product_append_response() {
        let parsed = parse_append_schema_response(&not_product_append_response_json()).unwrap();

        assert_eq!(
            parsed,
            GeneratedAppendSchema::NotProduct {
                reason: "category page".to_string(),
                evaluation: SchemaLlmEvaluation {
                    decision: SchemaLlmEvaluationDecision::Approve,
                    confidence: SchemaLlmEvaluationConfidence::High,
                    approved_by_llm: false,
                    summary: "Category page.".to_string(),
                    risks: vec![],
                },
            }
        );
    }

    #[test]
    fn should_reject_append_classification_for_create_validator() {
        let payload = not_product_append_response_json();

        let result = validate_create_schema_response(&payload);

        assert!(result.is_err());
    }

    #[test]
    fn should_reject_not_product_page_kind_with_schema_for_create_validator() {
        let payload = serde_json::to_string(&serde_json::json!({
            "page_kind": "not_product",
            "schemas": [sample_css_schema()],
            "reason": "category page",
            "confidence": "HIGH",
            "summary": "wrong page kind"
        }))
        .unwrap();

        let result = validate_create_schema_response(&payload);

        assert!(result.is_err());
    }

    #[test]
    fn should_reject_removed_page_kind_with_schema_for_create_validator() {
        let payload = serde_json::to_string(&serde_json::json!({
            "page_kind": "removed",
            "schemas": [sample_css_schema()],
            "removed_schema": {
                "selector": "#main h1",
                "text": "Product removed"
            },
            "confidence": "HIGH",
            "summary": "wrong page kind"
        }))
        .unwrap();

        let result = validate_create_schema_response(&payload);

        assert!(result.is_err());
    }

    #[test]
    fn should_reject_removed_schema_for_create_validator() {
        let payload = serde_json::to_string(&serde_json::json!({
            "schemas": [sample_css_schema()],
            "removed_schema": {
                "selector": "#main h1",
                "text": "Product removed"
            },
            "confidence": "HIGH",
            "summary": "append-only metadata"
        }))
        .unwrap();

        let result = validate_create_schema_response(&payload);

        assert!(result.is_err());
    }

    #[test]
    fn should_reject_classification_reason_for_create_validator() {
        let payload = serde_json::to_string(&serde_json::json!({
            "schemas": [sample_css_schema()],
            "reason": "category page",
            "confidence": "HIGH",
            "summary": "append-only metadata"
        }))
        .unwrap();

        let result = validate_create_schema_response(&payload);

        assert!(result.is_err());
    }

    #[test]
    fn should_accept_append_classification_for_append_validator() {
        let payload = not_product_append_response_json();

        let result = validate_append_schema_response(&payload);

        assert!(result.is_ok());
    }

    #[test]
    fn should_accept_product_schema_for_create_validator() {
        let payload = generated_response_json(vec![sample_css_schema()]);

        let result = validate_create_schema_response(&payload);

        assert!(result.is_ok());
    }

    #[test]
    fn should_require_reliable_currency_language_in_create_prompt() {
        let instruction = build_create_schemas_instruction(&[
            r#"<html><body><h1>Chair</h1></body></html>"#.to_string(),
        ]);

        assert!(instruction.contains("Every product schema must include default_currency."));
        assert!(instruction.contains("Determine default_currency only from reliable evidence"));
        assert!(instruction.contains("Never guess a currency"));
        assert!(
            instruction
                .contains("Explicit currency contained in an extracted price is authoritative")
        );
        assert!(instruction.contains("do not claim a valid high-confidence product schema"));
    }

    #[test]
    fn should_require_reliable_currency_language_in_append_prompt() {
        let instruction =
            build_append_schema_instruction(r#"<html><body><h1>Chair</h1></body></html>"#);

        assert!(instruction.contains("Every product schema must include default_currency."));
        assert!(instruction.contains("Never guess a currency"));
        assert!(instruction.contains("do not claim a valid high-confidence product schema"));
    }

    #[test]
    fn should_accept_explicit_product_page_kind_for_create_validator() {
        let payload = serde_json::to_string(&serde_json::json!({
            "page_kind": "product",
            "schemas": [sample_css_schema()],
            "confidence": "HIGH",
            "summary": "Selectors are product-specific.",
            "risks": [],
        }))
        .unwrap();

        let result = validate_create_schema_response(&payload);

        assert!(result.is_ok());
    }

    #[test]
    fn should_reject_generated_product_schema_missing_default_currency_for_create_validator() {
        let payload = serde_json::to_string(&serde_json::json!({
            "schemas": [{
                "shops_product_id": null,
                "title": {"selector": "h1", "additional_selectors": [], "type": "text", "cardinality": "first"},
                "description": null,
                "price": null,
                "price_estimate_min": null,
                "price_estimate_max": null,
                "seller_name": null,
                "state": {"selector": "#state", "additional_selectors": [], "type": "text", "cardinality": "first"},
                "images": {"selector": "img", "additional_selectors": [], "type": "attribute", "name": "src", "cardinality": "all"},
                "auction_start": null,
                "auction_end": null,
                "raw_attributes": {}
            }],
            "confidence": "HIGH",
            "summary": "missing currency"
        }))
        .unwrap();

        assert!(parse_product_schemas_response(&payload).is_err());
        assert!(validate_create_schema_response(&payload).is_err());
    }

    #[test]
    fn should_reject_generated_product_schema_null_default_currency_for_create_validator() {
        let payload = serde_json::to_string(&serde_json::json!({
            "schemas": [{
                "shops_product_id": null,
                "title": {"selector": "h1", "additional_selectors": [], "type": "text", "cardinality": "first"},
                "description": null,
                "price": null,
                "price_estimate_min": null,
                "price_estimate_max": null,
                "seller_name": null,
                "state": {"selector": "#state", "additional_selectors": [], "type": "text", "cardinality": "first"},
                "images": {"selector": "img", "additional_selectors": [], "type": "attribute", "name": "src", "cardinality": "all"},
                "auction_start": null,
                "auction_end": null,
                "default_currency": null,
                "raw_attributes": {}
            }],
            "confidence": "HIGH",
            "summary": "null currency"
        }))
        .unwrap();

        assert!(parse_product_schemas_response(&payload).is_err());
        assert!(validate_create_schema_response(&payload).is_err());
    }

    #[test]
    fn should_reject_removed_append_response_without_schema() {
        let payload = serde_json::to_string(&serde_json::json!({
            "page_kind": "removed",
            "schemas": [],
            "confidence": "HIGH",
            "summary": "missing schema"
        }))
        .unwrap();

        assert!(parse_append_schema_response(&payload).is_err());
    }

    #[test]
    fn should_parse_not_product_append_response_without_schema() {
        let payload = serde_json::to_string(&serde_json::json!({
            "page_kind": "not_product",
            "schemas": [],
            "reason": "category page",
            "confidence": "HIGH",
            "summary": "missing schema"
        }))
        .unwrap();

        let parsed = parse_append_schema_response(&payload).unwrap();
        assert!(matches!(parsed, GeneratedAppendSchema::NotProduct { .. }));
    }

    #[test]
    fn should_accept_removed_append_response_without_schemas_field() {
        let payload = serde_json::to_string(&serde_json::json!({
            "page_kind": "removed",
            "removed_schema": {
                "selector": "#mainCatCol h1",
                "text": "Sorry, the page you're looking for couldn't be found"
            },
            "confidence": "HIGH",
            "summary": "Soft 404 page."
        }))
        .unwrap();

        assert!(validate_append_schema_response(&payload).is_ok());
        assert!(matches!(
            parse_append_schema_response(&payload).unwrap(),
            GeneratedAppendSchema::Removed { .. }
        ));
    }

    #[test]
    fn should_accept_not_product_append_response_without_schemas_field() {
        let payload = serde_json::to_string(&serde_json::json!({
            "page_kind": "not_product",
            "reason": "category page",
            "confidence": "HIGH",
            "summary": "Category page."
        }))
        .unwrap();

        assert!(validate_append_schema_response(&payload).is_ok());
        assert!(matches!(
            parse_append_schema_response(&payload).unwrap(),
            GeneratedAppendSchema::NotProduct { .. }
        ));
    }

    #[test]
    fn should_reject_not_product_append_response_with_product_schema() {
        let payload = serde_json::to_string(&serde_json::json!({
            "page_kind": "not_product",
            "schemas": [sample_css_schema()],
            "reason": "category page",
            "confidence": "HIGH",
            "summary": "wrong schema"
        }))
        .unwrap();

        assert!(parse_append_schema_response(&payload).is_err());
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
            _: &[ChatMessage],
            _: Option<&[llm::chat::Tool]>,
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
            _: &[ChatMessage],
            _: Option<&[llm::chat::Tool]>,
        ) -> Result<Box<dyn llm::chat::ChatResponse>, LLMError> {
            let json = generated_response_json(vec![self.0.clone()]);
            Ok(Box::new(FakeChatResponse(Some(json))))
        }
    }
}
