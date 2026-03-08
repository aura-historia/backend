use crate::css_selector::product_schema::{
    ApplySchemaError, ProductCssSelectorSchema, ShopsProductSchema,
};
use crate::css_selector::product_schema_repository::ShopsProductSchemaRepository;
use common::shop_id::ShopId;
use kuchiki::traits::*;
use kuchiki::{NodeRef, parse_html};
use llm::{LLMProvider, chat::ChatMessage, error::LLMError};
use schemars::schema_for;
use time::OffsetDateTime;
use tracing::{debug, info, warn};

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
        html: &str,
    ) -> Result<ProductCssSelectorSchema, ProductSchemaServiceError>;

    async fn fix_product_schema(
        &self,
        schema: &ProductCssSelectorSchema,
        err: &ApplySchemaError,
        html: &str,
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

    async fn get_product_schema(
        &self,
        shop_id: &ShopId,
        html: &str,
    ) -> Result<ShopsProductSchema, ProductSchemaServiceError>;
}

pub struct ProductSchemaServiceImpl {
    llm: Box<dyn LLMProvider>,
    repository: Box<dyn ShopsProductSchemaRepository + Send + Sync>,
}

impl ProductSchemaServiceImpl {
    pub fn new(
        llm: llm::builder::LLMBuilder,
        repository: Box<dyn ShopsProductSchemaRepository + Send + Sync>,
    ) -> Result<Self, LLMError> {
        let schema = serde_json::to_string_pretty(&schema_for!(ProductCssSelectorSchema))
            .unwrap_or_else(|_| "Failed to generate schema".to_string());
        let system_prompt = format!(
            "You are an e-commerce scraper-assistant for antiques creating extraction-schemas for HTML given product-pages.
            Only answer with JSON for the following schema: \n\n {schema}",
        );
        let llm = llm
            .resilient(true)
            .resilient_attempts(3)
            .system(system_prompt)
            .openai_enable_web_search(false)
            .reasoning(true)
            .timeout_seconds(180)
            .validator(|res| {
                serde_json::from_str::<ProductCssSelectorSchema>(strip_markdown_json_embedding(res))
                    .map(|_| ())
                    .map_err(|err| err.to_string())
            })
            .validator_attempts(3)
            .build()?;
        Ok(Self { llm, repository })
    }
}

#[async_trait::async_trait]
impl ProductSchemaService for ProductSchemaServiceImpl {
    async fn create_product_schema(
        &self,
        html: &str,
    ) -> Result<ProductCssSelectorSchema, ProductSchemaServiceError> {
        let instruction = format!(
            "Generate a robust Extraction-Schema for given HTML. Here is the HTML: \n\n {}",
            clean_html_for_schema_generation(html),
        );
        let message = ChatMessage::user().content(instruction).build();
        let messages = vec![message];

        let res = self.llm.chat(&messages).await?.text().ok_or_else(|| {
            ProductSchemaServiceError::NoTextResponse("Expected text response".to_string())
        })?;

        let schema = serde_json::from_str(strip_markdown_json_embedding(&res))
            .map_err(ProductSchemaServiceError::JsonParsingTargetSchemaError)?;
        info!("LLM created new product CSS selector schema");
        Ok(schema)
    }

    async fn fix_product_schema(
        &self,
        schema: &ProductCssSelectorSchema,
        err: &ApplySchemaError,
        html: &str,
    ) -> Result<ProductCssSelectorSchema, ProductSchemaServiceError> {
        const MAX_RETRIES: u32 = 3;

        let current_schema_json =
            serde_json::to_string_pretty(schema).unwrap_or_else(|_| "<serialization error>".into());

        let instruction = format!(
            "The following CSS-selector extraction schema failed to extract data from the given HTML page.\n\
             \n\
             Error: {err}\n\
             \n\
             Current schema:\n\
             ```json\n\
             {current_schema_json}\n\
             ```\n\
             \n\
             HTML:\n\
             {cleaned_html}\n\
             \n\
             Instructions:\n\
             1. PREFER fixing only the failing rule by adding one or more entries to its \
                `additional_selectors` array so that the correct element is still found.\n\
             2. Only rewrite the entire schema if the page structure makes a targeted fix impossible.\n\
             3. Return the corrected schema as JSON and nothing else.",
            err = err,
            current_schema_json = current_schema_json,
            cleaned_html = clean_html_for_schema_generation(html),
        );

        let mut last_err: Option<ProductSchemaServiceError> = None;

        for attempt in 1..=MAX_RETRIES {
            debug!(
                attempt,
                maxRetries = MAX_RETRIES,
                "Attempting to fix product schema via LLM"
            );

            let message = ChatMessage::user().content(instruction.clone()).build();
            let res = match self.llm.chat(&[message]).await {
                Ok(response) => response,
                Err(e) => {
                    warn!(attempt, error = %e, "LLM call failed while fixing product schema");
                    last_err = Some(ProductSchemaServiceError::LLMError(e));
                    continue;
                }
            };

            let text = match res.text() {
                Some(t) => t,
                None => {
                    warn!(attempt, "LLM returned no text while fixing product schema");
                    last_err = Some(ProductSchemaServiceError::NoTextResponse(
                        "Expected text response while fixing schema".to_string(),
                    ));
                    continue;
                }
            };

            match serde_json::from_str::<ProductCssSelectorSchema>(strip_markdown_json_embedding(
                &text,
            )) {
                Ok(fixed_schema) => {
                    info!(attempt, "LLM successfully produced a fixed product schema");
                    return Ok(fixed_schema);
                }
                Err(parse_err) => {
                    warn!(
                        attempt,
                        error = %parse_err,
                        "LLM returned unparseable schema while fixing product schema"
                    );
                    last_err = Some(ProductSchemaServiceError::JsonParsingTargetSchemaError(
                        parse_err,
                    ));
                }
            }
        }

        warn!(
            maxRetries = MAX_RETRIES,
            "Exhausted all retries fixing product schema, giving up"
        );
        Err(last_err.unwrap_or_else(|| {
            ProductSchemaServiceError::NoTextResponse(
                "Exhausted all retries fixing schema without a usable response".to_string(),
            )
        }))
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
        let existing = self.repository.find_product_schema(shop_id).await?;

        match existing {
            Some(_) => {
                info!(shopId = %shop_id, "Updating existing product schema");
                self.repository
                    .update_product_schema(shop_id, &product_schema)
                    .await
                    .map_err(ProductSchemaServiceError::DatabaseError)
            }
            None => {
                info!(shopId = %shop_id, "Inserting new product schema");
                let now = OffsetDateTime::now_utc();
                let schema = ShopsProductSchema {
                    shop_id: *shop_id,
                    product_schema,
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

    async fn get_product_schema(
        &self,
        shop_id: &ShopId,
        html: &str,
    ) -> Result<ShopsProductSchema, ProductSchemaServiceError> {
        if let Some(existing) = self.find_product_schema(shop_id).await? {
            debug!(shopId = %shop_id, "Found existing product schema");
            return Ok(existing);
        }

        info!(shopId = %shop_id, "No product schema found for shop, creating via LLM");
        let product_schema = self.create_product_schema(html).await?;
        self.save_product_schema(shop_id, product_schema).await
    }
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
    use crate::css_selector::product_schema_repository::MockShopsProductSchemaRepository;
    use crate::css_selector::rule::{ExtractionCardinality, ExtractionKind, ExtractionRule};
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
        }
    }

    fn sample_shops_product_schema(shop_id: ShopId) -> ShopsProductSchema {
        let now = OffsetDateTime::now_utc();
        ShopsProductSchema {
            shop_id,
            product_schema: sample_css_schema(),
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
            repository: Box::new(repository),
        };

        let result = service.find_product_schema(&shop_id).await.unwrap();
        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.shop_id, expected.shop_id);
        assert_eq!(result.product_schema, expected.product_schema);
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
            repository: Box::new(repository),
        };

        let result = service
            .save_product_schema(&shop_id, css_schema)
            .await
            .unwrap();
        assert_eq!(result.shop_id, expected.shop_id);
        assert_eq!(result.product_schema, expected.product_schema);
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
            repository: Box::new(repository),
        };

        let result = service
            .get_product_schema(&shop_id, "<html></html>")
            .await
            .unwrap();
        assert_eq!(result.shop_id, existing.shop_id);
        assert_eq!(result.product_schema, existing.product_schema);
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
            repository: Box::new(repository),
        };

        let result = service
            .get_product_schema(&shop_id, "<html></html>")
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
            repository: Box::new(repository),
        };

        let result = service.get_product_schema(&shop_id, "<html></html>").await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ProductSchemaServiceError::DatabaseError(_)
        ));
    }

    // -----------------------------------------------------------------------
    // fix_product_schema
    // -----------------------------------------------------------------------

    fn apply_schema_error_title() -> ApplySchemaError {
        use crate::css_selector::rule::ExtractionError;
        ApplySchemaError::Title(ExtractionError::NoElementMatched {
            selector: "h1.title".to_string(),
        })
    }

    #[tokio::test]
    async fn should_return_fixed_schema_on_first_attempt_for_fix() {
        let fixed_schema = sample_css_schema();
        let fixed_clone = fixed_schema.clone();

        let service = ProductSchemaServiceImpl {
            llm: Box::new(MockLlmProviderReturning(fixed_clone)),
            repository: Box::new(MockShopsProductSchemaRepository::new()),
        };

        let original = sample_css_schema();
        let err = apply_schema_error_title();
        let result = service
            .fix_product_schema(&original, &err, "<html><body></body></html>")
            .await
            .unwrap();

        assert_eq!(result, fixed_schema);
    }

    #[tokio::test]
    async fn should_return_fixed_schema_after_multiple_attempts_for_fix() {
        // The static counter must be reset between test runs when tests are
        // executed sequentially in the same process. We use a dedicated atomic
        // per-test by inlining the provider here.
        use std::sync::atomic::{AtomicU32, Ordering};

        static CALL_COUNT: AtomicU32 = AtomicU32::new(0);
        // Reset in case a previous test run left it dirty.
        CALL_COUNT.store(0, Ordering::SeqCst);

        let fixed_schema = sample_css_schema();
        let fixed_json = serde_json::to_string(&fixed_schema).unwrap();

        struct TwoFailsThenSucceed(String);

        #[async_trait::async_trait]
        impl llm::chat::ChatProvider for TwoFailsThenSucceed {
            async fn chat_with_tools(
                &self,
                _messages: &[ChatMessage],
                _tools: Option<&[llm::chat::Tool]>,
            ) -> Result<Box<dyn llm::chat::ChatResponse>, LLMError> {
                use std::sync::atomic::{AtomicU32, Ordering};
                static COUNT: AtomicU32 = AtomicU32::new(0);
                let n = COUNT.fetch_add(1, Ordering::SeqCst);
                let text = if n < 2 {
                    "not valid json".to_string()
                } else {
                    self.0.clone()
                };
                Ok(Box::new(FakeChatResponse(Some(text))))
            }
        }

        #[async_trait::async_trait]
        impl llm::completion::CompletionProvider for TwoFailsThenSucceed {
            async fn complete(
                &self,
                _req: &llm::completion::CompletionRequest,
            ) -> Result<llm::completion::CompletionResponse, LLMError> {
                unimplemented!()
            }
        }

        #[async_trait::async_trait]
        impl llm::embedding::EmbeddingProvider for TwoFailsThenSucceed {
            async fn embed(&self, _input: Vec<String>) -> Result<Vec<Vec<f32>>, LLMError> {
                unimplemented!()
            }
        }

        #[async_trait::async_trait]
        impl llm::stt::SpeechToTextProvider for TwoFailsThenSucceed {
            async fn transcribe(&self, _audio: Vec<u8>) -> Result<String, LLMError> {
                unimplemented!()
            }
        }

        #[async_trait::async_trait]
        impl llm::tts::TextToSpeechProvider for TwoFailsThenSucceed {}

        #[async_trait::async_trait]
        impl llm::models::ModelsProvider for TwoFailsThenSucceed {}

        impl LLMProvider for TwoFailsThenSucceed {}

        let service = ProductSchemaServiceImpl {
            llm: Box::new(TwoFailsThenSucceed(fixed_json)),
            repository: Box::new(MockShopsProductSchemaRepository::new()),
        };

        let original = sample_css_schema();
        let err = apply_schema_error_title();
        let result = service
            .fix_product_schema(&original, &err, "<html><body></body></html>")
            .await
            .unwrap();

        assert_eq!(result, fixed_schema);
    }

    #[tokio::test]
    async fn should_return_json_parsing_error_when_all_attempts_return_invalid_json_for_fix() {
        struct AlwaysInvalidJson;

        #[async_trait::async_trait]
        impl llm::chat::ChatProvider for AlwaysInvalidJson {
            async fn chat_with_tools(
                &self,
                _messages: &[ChatMessage],
                _tools: Option<&[llm::chat::Tool]>,
            ) -> Result<Box<dyn llm::chat::ChatResponse>, LLMError> {
                Ok(Box::new(FakeChatResponse(Some(
                    "not valid json at all".to_string(),
                ))))
            }
        }

        #[async_trait::async_trait]
        impl llm::completion::CompletionProvider for AlwaysInvalidJson {
            async fn complete(
                &self,
                _req: &llm::completion::CompletionRequest,
            ) -> Result<llm::completion::CompletionResponse, LLMError> {
                unimplemented!()
            }
        }

        #[async_trait::async_trait]
        impl llm::embedding::EmbeddingProvider for AlwaysInvalidJson {
            async fn embed(&self, _input: Vec<String>) -> Result<Vec<Vec<f32>>, LLMError> {
                unimplemented!()
            }
        }

        #[async_trait::async_trait]
        impl llm::stt::SpeechToTextProvider for AlwaysInvalidJson {
            async fn transcribe(&self, _audio: Vec<u8>) -> Result<String, LLMError> {
                unimplemented!()
            }
        }

        #[async_trait::async_trait]
        impl llm::tts::TextToSpeechProvider for AlwaysInvalidJson {}

        #[async_trait::async_trait]
        impl llm::models::ModelsProvider for AlwaysInvalidJson {}

        impl LLMProvider for AlwaysInvalidJson {}

        let service = ProductSchemaServiceImpl {
            llm: Box::new(AlwaysInvalidJson),
            repository: Box::new(MockShopsProductSchemaRepository::new()),
        };

        let original = sample_css_schema();
        let err = apply_schema_error_title();
        let result = service
            .fix_product_schema(&original, &err, "<html><body></body></html>")
            .await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ProductSchemaServiceError::JsonParsingTargetSchemaError(_)
        ));
    }

    #[tokio::test]
    async fn should_return_no_text_response_error_when_all_attempts_return_no_text_for_fix() {
        struct AlwaysNoText;

        #[async_trait::async_trait]
        impl llm::chat::ChatProvider for AlwaysNoText {
            async fn chat_with_tools(
                &self,
                _messages: &[ChatMessage],
                _tools: Option<&[llm::chat::Tool]>,
            ) -> Result<Box<dyn llm::chat::ChatResponse>, LLMError> {
                Ok(Box::new(FakeChatResponse(None)))
            }
        }

        #[async_trait::async_trait]
        impl llm::completion::CompletionProvider for AlwaysNoText {
            async fn complete(
                &self,
                _req: &llm::completion::CompletionRequest,
            ) -> Result<llm::completion::CompletionResponse, LLMError> {
                unimplemented!()
            }
        }

        #[async_trait::async_trait]
        impl llm::embedding::EmbeddingProvider for AlwaysNoText {
            async fn embed(&self, _input: Vec<String>) -> Result<Vec<Vec<f32>>, LLMError> {
                unimplemented!()
            }
        }

        #[async_trait::async_trait]
        impl llm::stt::SpeechToTextProvider for AlwaysNoText {
            async fn transcribe(&self, _audio: Vec<u8>) -> Result<String, LLMError> {
                unimplemented!()
            }
        }

        #[async_trait::async_trait]
        impl llm::tts::TextToSpeechProvider for AlwaysNoText {}

        #[async_trait::async_trait]
        impl llm::models::ModelsProvider for AlwaysNoText {}

        impl LLMProvider for AlwaysNoText {}

        let service = ProductSchemaServiceImpl {
            llm: Box::new(AlwaysNoText),
            repository: Box::new(MockShopsProductSchemaRepository::new()),
        };

        let original = sample_css_schema();
        let err = apply_schema_error_title();
        let result = service
            .fix_product_schema(&original, &err, "<html><body></body></html>")
            .await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ProductSchemaServiceError::NoTextResponse(_)
        ));
    }

    #[tokio::test]
    async fn should_propagate_llm_error_when_all_attempts_fail_for_fix() {
        struct AlwaysLlmError;

        #[async_trait::async_trait]
        impl llm::chat::ChatProvider for AlwaysLlmError {
            async fn chat_with_tools(
                &self,
                _messages: &[ChatMessage],
                _tools: Option<&[llm::chat::Tool]>,
            ) -> Result<Box<dyn llm::chat::ChatResponse>, LLMError> {
                Err(LLMError::ProviderError("simulated LLM failure".to_string()))
            }
        }

        #[async_trait::async_trait]
        impl llm::completion::CompletionProvider for AlwaysLlmError {
            async fn complete(
                &self,
                _req: &llm::completion::CompletionRequest,
            ) -> Result<llm::completion::CompletionResponse, LLMError> {
                unimplemented!()
            }
        }

        #[async_trait::async_trait]
        impl llm::embedding::EmbeddingProvider for AlwaysLlmError {
            async fn embed(&self, _input: Vec<String>) -> Result<Vec<Vec<f32>>, LLMError> {
                unimplemented!()
            }
        }

        #[async_trait::async_trait]
        impl llm::stt::SpeechToTextProvider for AlwaysLlmError {
            async fn transcribe(&self, _audio: Vec<u8>) -> Result<String, LLMError> {
                unimplemented!()
            }
        }

        #[async_trait::async_trait]
        impl llm::tts::TextToSpeechProvider for AlwaysLlmError {}

        #[async_trait::async_trait]
        impl llm::models::ModelsProvider for AlwaysLlmError {}

        impl LLMProvider for AlwaysLlmError {}

        let service = ProductSchemaServiceImpl {
            llm: Box::new(AlwaysLlmError),
            repository: Box::new(MockShopsProductSchemaRepository::new()),
        };

        let original = sample_css_schema();
        let err = apply_schema_error_title();
        let result = service
            .fix_product_schema(&original, &err, "<html><body></body></html>")
            .await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ProductSchemaServiceError::LLMError(_)
        ));
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

    #[async_trait::async_trait]
    impl llm::completion::CompletionProvider for MockLlmProvider {
        async fn complete(
            &self,
            _req: &llm::completion::CompletionRequest,
        ) -> Result<llm::completion::CompletionResponse, LLMError> {
            unimplemented!()
        }
    }

    #[async_trait::async_trait]
    impl llm::embedding::EmbeddingProvider for MockLlmProvider {
        async fn embed(&self, _input: Vec<String>) -> Result<Vec<Vec<f32>>, LLMError> {
            unimplemented!()
        }
    }

    #[async_trait::async_trait]
    impl llm::stt::SpeechToTextProvider for MockLlmProvider {
        async fn transcribe(&self, _audio: Vec<u8>) -> Result<String, LLMError> {
            unimplemented!()
        }
    }

    #[async_trait::async_trait]
    impl llm::tts::TextToSpeechProvider for MockLlmProvider {}

    #[async_trait::async_trait]
    impl llm::models::ModelsProvider for MockLlmProvider {}

    impl LLMProvider for MockLlmProvider {}

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

    #[async_trait::async_trait]
    impl llm::completion::CompletionProvider for MockLlmProviderReturning {
        async fn complete(
            &self,
            _req: &llm::completion::CompletionRequest,
        ) -> Result<llm::completion::CompletionResponse, LLMError> {
            unimplemented!()
        }
    }

    #[async_trait::async_trait]
    impl llm::embedding::EmbeddingProvider for MockLlmProviderReturning {
        async fn embed(&self, _input: Vec<String>) -> Result<Vec<Vec<f32>>, LLMError> {
            unimplemented!()
        }
    }

    #[async_trait::async_trait]
    impl llm::stt::SpeechToTextProvider for MockLlmProviderReturning {
        async fn transcribe(&self, _audio: Vec<u8>) -> Result<String, LLMError> {
            unimplemented!()
        }
    }

    #[async_trait::async_trait]
    impl llm::tts::TextToSpeechProvider for MockLlmProviderReturning {}

    #[async_trait::async_trait]
    impl llm::models::ModelsProvider for MockLlmProviderReturning {}

    impl LLMProvider for MockLlmProviderReturning {}
}
