use crate::google_llm::GeminiRateLimiter;
use crate::logging::llm_metrics;
use crate::scraper::css_selector::product_schema::{
    ApplySchemaError, ProductCssSelectorSchema, ShopsProductSchema,
};
use crate::scraper::css_selector::product_schema_repository::ShopsProductSchemaRepository;
use common::logging::{GeminiServiceTier, LlmModel, LlmOperation, LlmProvider, log_llm_invocation};
use common::shop_id::ShopId;
use kuchiki::traits::*;
use kuchiki::{NodeRef, parse_html};
use llm::{
    chat::{ChatMessage, ChatProvider},
    error::LLMError,
};
use schemars::schema_for;
use std::sync::Arc;
use std::time::Instant;
use time::OffsetDateTime;
use tracing::{debug, info};

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
    ) -> Result<Vec<ProductCssSelectorSchema>, ProductSchemaServiceError>;

    /// Generate a single schema from a single HTML page and append it to the
    /// cached schema set. Used when a runtime schema-variant match fails to
    /// dynamically expand the schema set without full regeneration.
    async fn append_single_schema(
        &self,
        html: &str,
        failed_schema: Option<&ProductCssSelectorSchema>,
        last_error: Option<&ApplySchemaError>,
    ) -> Result<ProductCssSelectorSchema, ProductSchemaServiceError>;

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
        let schema = serde_json::to_string_pretty(&schema_for!(ProductCssSelectorSchema))
            .unwrap_or_else(|_| "Failed to generate schema".to_string());
        let system_prompt = format!(
            "You are an e-commerce scraper-assistant for antiques creating extraction-schemas for HTML given product-pages.
            Return only JSON. The output may be either:
            - a single object matching this schema, or
            - an array of such objects.
            Schema:\n\n {schema}",
        );
        let llm = llm
            .resilient(true)
            .resilient_attempts(3)
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
        Ok(Self {
            llm,
            rate_limiter,
            service_tier,
            repository,
        })
    }
}

fn parse_product_schemas_response(
    raw: &str,
) -> Result<Vec<ProductCssSelectorSchema>, serde_json::Error> {
    match serde_json::from_str::<Vec<ProductCssSelectorSchema>>(raw) {
        Ok(list) => Ok(list),
        Err(_) => serde_json::from_str::<ProductCssSelectorSchema>(raw).map(|single| vec![single]),
    }
}

#[async_trait::async_trait]
impl ProductSchemaService for ProductSchemaServiceImpl {
    async fn create_product_schema(
        &self,
        html_pages: &[String],
    ) -> Result<ProductCssSelectorSchema, ProductSchemaServiceError> {
        let schemas = self.create_product_schemas(html_pages).await?;
        schemas.first().cloned().ok_or_else(|| {
            ProductSchemaServiceError::NoTextResponse("LLM produced zero schemas".to_string())
        })
    }

    #[tracing::instrument(skip(self, html_pages), fields(html_pages = html_pages.len()))]
    async fn create_product_schemas(
        &self,
        html_pages: &[String],
    ) -> Result<Vec<ProductCssSelectorSchema>, ProductSchemaServiceError> {
        let instruction = build_create_schemas_instruction(html_pages);
        let message = ChatMessage::user().content(instruction).build();
        let messages = vec![message];

        let started_at = Instant::now();
        let permit = match &self.rate_limiter {
            Some(limiter) => Some(limiter.acquire().await?),
            None => None,
        };
        let response = self.llm.chat(&messages).await?;
        drop(permit);
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
        let schemas = parse_product_schemas_response(parsed)
            .map_err(ProductSchemaServiceError::JsonParsingTargetSchemaError)?;
        if schemas.is_empty() {
            return Err(ProductSchemaServiceError::NoTextResponse(
                "LLM produced zero schemas".to_string(),
            ));
        }

        debug!(
            schemas_count = schemas.len(),
            "LLM created product CSS selector schemas"
        );
        Ok(schemas)
    }

    #[tracing::instrument(
        name = "scraper_append_single_schema",
        skip(self, html, failed_schema, last_error)
    )]
    async fn append_single_schema(
        &self,
        html: &str,
        failed_schema: Option<&ProductCssSelectorSchema>,
        last_error: Option<&ApplySchemaError>,
    ) -> Result<ProductCssSelectorSchema, ProductSchemaServiceError> {
        // Generate single schema for this HTML page
        let instruction = build_append_schema_instruction(html, failed_schema, last_error);
        let message = ChatMessage::user().content(instruction).build();
        let messages = vec![message];

        let started_at = Instant::now();
        let permit = match &self.rate_limiter {
            Some(limiter) => Some(limiter.acquire().await?),
            None => None,
        };
        let response = self.llm.chat(&messages).await?;
        drop(permit);
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
        let new_schemas = parse_product_schemas_response(parsed)
            .map_err(ProductSchemaServiceError::JsonParsingTargetSchemaError)?;

        if new_schemas.is_empty() {
            return Err(ProductSchemaServiceError::NoTextResponse(
                "LLM produced zero schemas when appending".to_string(),
            ));
        }

        let schema = new_schemas.first().cloned().ok_or_else(|| {
            ProductSchemaServiceError::NoTextResponse(
                "LLM produced zero schemas when appending".to_string(),
            )
        })?;

        debug!("Generated single schema for append-and-retry");
        Ok(schema)
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
                info!("Updating existing product schema");
                self.repository
                    .update_product_schema(shop_id, &product_schemas)
                    .await
                    .map_err(ProductSchemaServiceError::DatabaseError)
            }
            None => {
                info!("Inserting new product schema");
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
        let product_schemas = self.create_product_schemas(html_pages).await?;
        self.save_product_schemas(shop_id, product_schemas).await
    }
}

fn build_create_schemas_instruction(html_pages: &[String]) -> String {
    let cleaned_pages: Vec<String> = if html_pages.is_empty() {
        Vec::new()
    } else {
        html_pages
            .iter()
            .map(|html| clean_html_for_schema_generation(html))
            .collect()
    };

    if cleaned_pages.is_empty() {
        return String::from(
            "Generate one or more robust Extraction-Schemas for the given HTML product pages.",
        );
    }

    let mut samples = String::new();
    for (idx, cleaned) in cleaned_pages.iter().enumerate() {
        let _ = std::fmt::Write::write_fmt(
            &mut samples,
            format_args!(
                "\n--- SAMPLE {sample_idx} ---\n{html}\n",
                sample_idx = idx + 1,
                html = cleaned
            ),
        );
    }

    format!(
        "Generate one or more robust Extraction-Schemas that together cover these product page HTML samples from the same shop.\n\
         Shops may have multiple templates/layouts. If a single schema cannot generalize to all samples, return multiple schemas where each schema works for a subset of samples.\n\
         Prefer high-precision selectors that represent semantic fields rather than layout wrappers.\n\
         Return ONLY JSON as an array of ProductCssSelectorSchema objects.\n\
         Optional fields may remain null if not confidently present.\n\
         Here are the cleaned HTML samples:{samples}"
    )
}

fn build_append_schema_instruction(
    html: &str,
    failed_schema: Option<&ProductCssSelectorSchema>,
    last_error: Option<&ApplySchemaError>,
) -> String {
    let cleaned = clean_html_for_schema_generation(html);
    let failure_context = match (failed_schema, last_error) {
        (Some(schema), Some(error)) => {
            let schema_json = serde_json::to_string_pretty(schema)
                .unwrap_or_else(|_| "<failed to serialize previous schema>".to_string());
            format!(
                "\nPrevious attempt failed. Here is the schema that just failed:\n{schema_json}\n\
                 Extraction failure observed:\n{error}\n\
                 Improve/fix the failed schema for this page instead of repeating the same selectors."
            )
        }
        _ => String::new(),
    };

    format!(
        "Generate a single robust Extraction-Schema for the following product page HTML.\n\
          This schema will be appended to a set of existing schemas from the same shop.\n\
          Focus on this specific layout and make the selectors resilient.\n\
          Return ONLY JSON as a single ProductCssSelectorSchema object (not an array).\n\
          Optional fields may remain null if not confidently present.\n\
          {failure_context}\n\
          Here is the cleaned HTML:\n\
          {cleaned}"
    )
}

pub fn strip_markdown_json_embedding(s: &str) -> &str {
    s.trim()
        .strip_prefix("```json")
        .unwrap_or(s)
        .strip_suffix("```")
        .unwrap_or(s)
}

pub fn clean_html_for_schema_generation(input: &str) -> String {
    let document = parse_html().one(input);

    // Tags to remove entirely
    let remove_selectors = [
        "script", "style", "noscript", "svg", "canvas", "header", "footer", "nav", "aside",
    ];

    for selector in &remove_selectors {
        if let Ok(nodes) = document.select(selector) {
            for node in nodes {
                node.as_node().detach();
            }
        }
    }

    remove_comments(&document);
    strip_attributes(&document);
    let mut cleaned = Vec::new();
    document.serialize(&mut cleaned).unwrap();
    String::from_utf8(cleaned).unwrap_or_default()
}

fn remove_comments(node: &NodeRef) {
    for child in node.children() {
        if child.as_comment().is_some() {
            child.detach();
        } else {
            remove_comments(&child);
        }
    }
}

fn strip_attributes(document: &NodeRef) {
    let deny_prefixes = ["on"]; // onclick, onload, etc.

    let deny_exact = [
        "style",
        "integrity",
        "crossorigin",
        "referrerpolicy",
        "nonce",
        "tabindex",
        "width",
        "height",
        "loading",
        "decoding",
    ];

    for css_match in document.select("*").unwrap() {
        let mut attributes = css_match.attributes.borrow_mut();

        attributes.map.retain(|key, _| {
            let name = key.local.as_ref();

            // Remove JS event handlers
            if deny_prefixes.iter().any(|prefix| name.starts_with(prefix)) {
                return false;
            }

            // Remove known useless attributes
            if deny_exact.contains(&name) {
                return false;
            }

            true
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scraper::css_selector::product_schema_repository::MockShopsProductSchemaRepository;
    use crate::scraper::css_selector::rule::{
        ExtractionCardinality, ExtractionKind, ExtractionRule,
    };
    use common::shop_id::ShopId;
    use time::OffsetDateTime;

    fn sample_css_schema() -> ProductCssSelectorSchema {
        ProductCssSelectorSchema {
            shops_product_id: ExtractionRule {
                selector: "span.product-id".into(),
                additional_selectors: vec![],
                extract: ExtractionKind::Text,
                cardinality: ExtractionCardinality::First,
            },
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
        let instruction = build_create_schemas_instruction(&html_pages);
        assert!(instruction.contains("--- SAMPLE 1 ---"));
        assert!(instruction.contains("--- SAMPLE 2 ---"));
        assert!(
            instruction.contains(
                "return multiple schemas where each schema works for a subset of samples"
            )
        );
    }

    #[test]
    fn should_parse_array_of_schemas_response() {
        let payload = format!("[{}]", serde_json::to_string(&sample_css_schema()).unwrap());
        let parsed = parse_product_schemas_response(&payload).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0], sample_css_schema());
    }

    #[test]
    fn should_parse_single_schema_response_as_singleton_vec() {
        let payload = serde_json::to_string(&sample_css_schema()).unwrap();
        let parsed = parse_product_schemas_response(&payload).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0], sample_css_schema());
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
            let json = serde_json::to_string(&self.0).expect("schema should serialize");
            Ok(Box::new(FakeChatResponse(Some(json))))
        }
    }
}
